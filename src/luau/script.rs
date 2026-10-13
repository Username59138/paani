//! Which scripts run when the place starts, and how a script's thread and
//! environment are built. ModuleScripts run only through `require`.

use kahua_io::rbx_dom_weak::Instance;
use kahua_io::rbx_dom_weak::types::{Ref, Variant};
use kahua_io::rbx_dom_weak::{InstanceBuilder, Ustr, WeakDom};
use mlua::{Function, Lua, MultiValue, Table, Value};

use super::instance::InstanceRef;
use super::scheduler::run_thread;
use super::{Context, OutputKind, context};
use crate::rig;

/// Roblox's RunContext enum value for Client.
const RUN_CONTEXT_CLIENT: u32 = 2;

/// Runs the place's scripts. Server scripts run in DOM order; then the local
/// scripts are cloned into the player and run. Errors are reported per script.
pub(crate) fn start(lua: &Lua) {
    let ctx = context(lua);
    let server_scripts = collect_server_scripts(&ctx.dom.borrow());
    for script in server_scripts {
        run_script(lua, &ctx, script);
    }
    let locals = local_scripts(&mut ctx.dom.borrow_mut());
    for script in locals {
        run_script(lua, &ctx, script);
    }
}

/// Script instances under Workspace or ServerScriptService that are enabled.
/// A Script with RunContext Client also runs here: this client is the only
/// one, so running it in place matches a LocalScript in the same spot.
fn collect_server_scripts(dom: &WeakDom) -> Vec<Ref> {
    let root = dom.root_ref();
    let mut found = Vec::new();
    let Some(root_inst) = dom.get_by_ref(root) else {
        return found;
    };
    for &top in root_inst.children() {
        let Some(top_inst) = dom.get_by_ref(top) else {
            continue;
        };
        if matches!(top_inst.class.as_str(), "Workspace" | "ServerScriptService") {
            collect_scripts(dom, top, "Script", &mut found);
        }
    }
    found.retain(|&r| is_enabled(dom, r));
    found
}

/// Pre-order (depth-first) walk collecting instances of `class`.
fn collect_scripts(dom: &WeakDom, parent: Ref, class: &str, out: &mut Vec<Ref>) {
    let Some(inst) = dom.get_by_ref(parent) else {
        return;
    };
    for &child in inst.children() {
        let Some(child_inst) = dom.get_by_ref(child) else {
            continue;
        };
        if child_inst.class == class {
            out.push(child);
        }
        collect_scripts(dom, child, class, out);
    }
}

/// Clones the StarterPlayer local scripts into the player and character, then
/// returns the LocalScripts that should run for this client.
fn local_scripts(dom: &mut WeakDom) -> Vec<Ref> {
    let Some(players) = rig::service(dom, "Players") else {
        return Vec::new();
    };
    let Some(player) = rig::child_of_class(dom, players, "Player") else {
        return Vec::new();
    };
    let starter = rig::service(dom, "StarterPlayer");
    let player_scripts = match rig::child_of_class(dom, player, "PlayerScripts") {
        Some(folder) => folder,
        None => dom.insert(player, InstanceBuilder::new("PlayerScripts").with_name("PlayerScripts")),
    };
    let mut to_run = Vec::new();
    if let Some(source) = starter.and_then(|sp| rig::child_of_class(dom, sp, "StarterPlayerScripts")) {
        for clone in clone_children(dom, source, player_scripts) {
            if is_script_class(dom, clone, "LocalScript") || is_script_class(dom, clone, "Script") {
                to_run.push(clone);
            }
        }
    }
    let character = dom.get_by_ref(player).and_then(|p| prop_ref(p, "Character"));
    let Some(character) = character.filter(|&c| dom.get_by_ref(c).is_some()) else {
        return to_run;
    };
    if let Some(source) = starter.and_then(|sp| rig::child_of_class(dom, sp, "StarterCharacterScripts")) {
        clone_children(dom, source, character);
    }
    let mut in_character = Vec::new();
    collect_scripts(dom, character, "LocalScript", &mut in_character);
    to_run.extend(in_character);
    to_run.retain(|&r| is_enabled(dom, r));
    to_run
}

fn is_script_class(dom: &WeakDom, r: Ref, class: &str) -> bool {
    dom.get_by_ref(r).is_some_and(|i| i.class == class)
}

/// Clones every child of `source` and parents the clones under `target`.
fn clone_children(dom: &mut WeakDom, source: Ref, target: Ref) -> Vec<Ref> {
    let children: Vec<Ref> = dom.get_by_ref(source).map(|i| i.children().to_vec()).unwrap_or_default();
    children
        .into_iter()
        .map(|child| {
            let clone = dom.clone_within(child);
            dom.transfer_within(clone, target);
            clone
        })
        .collect()
}

fn prop_ref(inst: &Instance, key: &str) -> Option<Ref> {
    match inst.properties.get(&Ustr::from(key)) {
        Some(Variant::Ref(r)) => Some(*r),
        _ => None,
    }
}

fn is_enabled(dom: &WeakDom, script: Ref) -> bool {
    !rig::boolean(dom, Some(script), "Disabled", false) && rig::boolean(dom, Some(script), "Enabled", true)
}

/// Runs one script on its own thread. Load errors are reported, not raised.
fn run_script(lua: &Lua, ctx: &Context, script: Ref) {
    let (full_name, source) = {
        let dom = ctx.dom.borrow();
        let Some(inst) = dom.get_by_ref(script) else {
            return;
        };
        let source = string_prop(inst, "Source");
        (full_name(&dom, script), source)
    };
    let env = match environment(lua, script) {
        Ok(env) => env,
        Err(err) => return ctx.emit(OutputKind::Error, err.to_string()),
    };
    let func = match lua.load(source).set_name(format!("={full_name}")).set_environment(env).into_function() {
        Ok(func) => func,
        Err(err) => return ctx.emit(OutputKind::Error, err.to_string()),
    };
    match lua.create_thread(func) {
        Ok(thread) => run_thread(ctx, &thread, MultiValue::new()),
        Err(err) => ctx.emit(OutputKind::Error, err.to_string()),
    }
}

/// `Workspace.Part.Script`-style name, from the first child under the DataModel down.
pub(crate) fn full_name(dom: &WeakDom, r: Ref) -> String {
    let mut names = Vec::new();
    let mut current = r;
    while let Some(inst) = dom.get_by_ref(current) {
        if current == dom.root_ref() {
            break;
        }
        names.push(inst.name.clone());
        current = inst.parent();
    }
    names.reverse();
    names.join(".")
}

/// A fresh environment for one script: `script` set, everything else read from the globals.
fn environment(lua: &Lua, script: Ref) -> mlua::Result<Table> {
    let env = lua.create_table()?;
    env.set("script", lua.create_userdata(InstanceRef(script))?)?;
    let meta = lua.create_table()?;
    meta.set("__index", lua.globals())?;
    env.set_metatable(Some(meta))?;
    Ok(env)
}

/// Registers the module helpers used by the prelude's `require`.
pub(crate) fn install_natives(lua: &Lua, natives: &Table) -> mlua::Result<()> {
    natives.set(
        "requireCached",
        lua.create_function(|lua, module: InstanceRef| {
            let ctx = context(lua);
            let modules = ctx.modules.borrow();
            match modules.get(&module.0) {
                Some(key) => Ok((true, lua.registry_value::<Value>(key)?)),
                None => Ok((false, Value::Nil)),
            }
        })?,
    )?;
    natives.set(
        "requireChunk",
        lua.create_function(|lua, module: InstanceRef| -> mlua::Result<Function> {
            let ctx = context(lua);
            let (full_name, source) = {
                let dom = ctx.dom.borrow();
                let Some(inst) = dom.get_by_ref(module.0) else {
                    return Err(mlua::Error::runtime("Requested module was not a ModuleScript"));
                };
                if inst.class != "ModuleScript" {
                    return Err(mlua::Error::runtime("Requested module was not a ModuleScript"));
                }
                let source = string_prop(inst, "Source");
                (full_name(&dom, module.0), source)
            };
            let env = environment(lua, module.0)?;
            lua.load(source).set_name(format!("={full_name}")).set_environment(env).into_function()
        })?,
    )?;
    natives.set(
        "requireStore",
        lua.create_function(|lua, (module, value): (InstanceRef, Value)| {
            let key = lua.create_registry_value(value)?;
            context(lua).modules.borrow_mut().insert(module.0, key);
            Ok(())
        })?,
    )?;
    Ok(())
}

/// A string property, or empty when it is missing.
fn string_prop(inst: &Instance, key: &str) -> String {
    match inst.properties.get(&Ustr::from(key)) {
        Some(Variant::String(s)) => s.clone(),
        _ => String::new(),
    }
}
