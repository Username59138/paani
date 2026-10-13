//! Runs Roblox scripts (Script, LocalScript, ModuleScript) in Luau against the
//! place's DOM. The app owns the DOM; `ScriptHost` mutates it through the
//! shared handle and reports what changed so the renderer and physics can
//! catch up.

mod cframe;
mod color3;
mod convert;
mod enums;
mod globals;
mod instance;
mod scheduler;
mod script;
#[cfg(test)]
mod tests;
mod vector3;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use kahua_io::rbx_dom_weak::WeakDom;
use kahua_io::rbx_dom_weak::types::Ref;
use mlua::{Lua, RegistryKey, Table, VmState};

use scheduler::Scheduler;

pub type SharedDom = Rc<RefCell<WeakDom>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputKind {
    Print,
    Warn,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputLine {
    pub kind: OutputKind,
    pub text: String,
}

pub struct Options {
    /// A script that runs this long without yielding is stopped with an error.
    pub timeout: Duration,
}

impl Default for Options {
    fn default() -> Self {
        Self { timeout: Duration::from_secs(10) }
    }
}

/// State shared by the Rust callbacks. Lua callbacks reach it through the
/// Lua app data, so no callback has to capture anything.
pub(crate) struct Context {
    pub dom: SharedDom,
    pub dirty: Cell<bool>,
    pub output: RefCell<Vec<OutputLine>>,
    pub timeout: Duration,
    /// When the thread being resumed started running; read by the interrupt.
    pub resume_started: Cell<Option<Instant>>,
    pub scheduler: RefCell<Scheduler>,
    /// Yield marker that tells the scheduler a thread is waiting (see prelude.luau).
    pub wait_marker: RefCell<Option<Table>>,
    /// Methods implemented in Luau, such as WaitForChild.
    pub instance_methods: RefCell<Option<Table>>,
    /// Module results by ModuleScript, kept alive for the host's lifetime.
    pub modules: RefCell<HashMap<Ref, RegistryKey>>,
    /// Unix time at `ScriptHost::new`, so `tick()` reads as a wall clock.
    pub start_unix: f64,
}

impl Context {
    pub fn emit(&self, kind: OutputKind, text: String) {
        match kind {
            OutputKind::Print => println!("{text}"),
            OutputKind::Warn | OutputKind::Error => eprintln!("{text}"),
        }
        self.output.borrow_mut().push(OutputLine { kind, text });
    }
}

pub(crate) fn context(lua: &Lua) -> Rc<Context> {
    let ctx = lua.app_data_ref::<Rc<Context>>().expect("ScriptHost installs its context");
    Rc::clone(&ctx)
}

pub struct ScriptHost {
    lua: Lua,
    ctx: Rc<Context>,
}

impl ScriptHost {
    pub fn new(dom: SharedDom, options: Options) -> mlua::Result<Self> {
        let lua = Lua::new();
        let start_unix =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64());
        let ctx = Rc::new(Context {
            dom,
            dirty: Cell::new(false),
            output: RefCell::new(Vec::new()),
            timeout: options.timeout,
            resume_started: Cell::new(None),
            scheduler: RefCell::new(Scheduler::default()),
            wait_marker: RefCell::new(None),
            instance_methods: RefCell::new(None),
            modules: RefCell::new(HashMap::new()),
            start_unix,
        });
        lua.set_app_data(Rc::clone(&ctx));
        lua.set_interrupt(|lua| {
            let ctx = context(lua);
            if let Some(started) = ctx.resume_started.get()
                && started.elapsed() > ctx.timeout
            {
                return Err(mlua::Error::runtime("Script timeout: exhausted allowed execution time"));
            }
            Ok(VmState::Continue)
        });
        let mut host = Self { lua, ctx };
        host.install()?;
        Ok(host)
    }

    fn install(&mut self) -> mlua::Result<()> {
        let lua = &self.lua;
        let natives = lua.create_table()?;
        scheduler::install_natives(lua, &natives)?;
        script::install_natives(lua, &natives)?;
        globals::install(lua)?;
        let exports: Table = lua.load(include_str!("prelude.luau")).set_name("=Paani.prelude").call(natives)?;
        *self.ctx.wait_marker.borrow_mut() = Some(exports.get("waitMarker")?);
        *self.ctx.instance_methods.borrow_mut() = Some(exports.get("instanceMethods")?);
        Ok(())
    }

    /// Starts the place's scripts (rules in `script.rs`). Call once, after the
    /// character was spawned.
    pub fn start(&mut self) {
        script::start(&self.lua);
    }

    /// Advances script time by `dt` seconds and resumes due threads.
    pub fn step(&mut self, dt: f64) {
        scheduler::step(&self.ctx, dt);
    }

    /// True if scripts changed the DOM since the last call.
    pub fn take_dirty(&mut self) -> bool {
        self.ctx.dirty.replace(false)
    }

    /// Output since the last call. Print/Warn/Error are also written to stdout/stderr.
    pub fn take_output(&mut self) -> Vec<OutputLine> {
        std::mem::take(&mut *self.ctx.output.borrow_mut())
    }
}

impl Drop for ScriptHost {
    fn drop(&mut self) {
        // The context holds Lua threads and registry keys, which hold the Lua
        // state. Clearing them breaks that cycle so the state can be freed.
        self.ctx.scheduler.borrow_mut().clear();
        self.ctx.modules.borrow_mut().clear();
        self.ctx.wait_marker.borrow_mut().take();
        self.ctx.instance_methods.borrow_mut().take();
        self.lua.remove_app_data::<Rc<Context>>();
    }
}
