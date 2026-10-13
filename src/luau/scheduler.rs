//! Script time and the thread queue. Threads yield `(marker, seconds)` to
//! sleep; the scheduler wakes them in `step`. Rust callbacks cannot yield, so
//! all sleeping happens in Luau and only the bookkeeping lives here.

use std::time::Instant;

use mlua::thread::ThreadStatus;
use mlua::{Lua, MultiValue, Table, Thread, Value};

use super::{Context, OutputKind, context};

/// Shortest wait `wait` can ask for: one frame at 60 Hz.
const MIN_WAIT: f64 = 1.0 / 60.0;
/// Absorbs float error in accumulated `dt` so a wake time reached exactly is due.
const EPSILON: f64 = 1e-9;

struct Task {
    id: u64,
    thread: Thread,
    wake_at: f64,
    /// Script time when the thread went to sleep; `wait` reports elapsed time from here.
    since: f64,
    /// Values to resume with. `None` means a wait, which resumes with (elapsed, time).
    args: Option<MultiValue>,
}

#[derive(Default)]
pub(crate) struct Scheduler {
    now: f64,
    next_id: u64,
    tasks: Vec<Task>,
}

impl Scheduler {
    pub fn clear(&mut self) {
        self.tasks.clear();
    }

    fn push(&mut self, thread: Thread, wake_at: f64, args: Option<MultiValue>) {
        self.next_id += 1;
        self.tasks.push(Task { id: self.next_id, thread, wake_at, since: self.now, args });
    }

    /// Ids of the tasks due now, in wake order then schedule order. A snapshot:
    /// tasks scheduled while these run wait for the next step.
    fn due(&self) -> Vec<u64> {
        let mut due: Vec<(f64, u64)> =
            self.tasks.iter().filter(|t| t.wake_at <= self.now + EPSILON).map(|t| (t.wake_at, t.id)).collect();
        due.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        due.into_iter().map(|(_, id)| id).collect()
    }

    /// Removes a task by id. Returns None if it was cancelled meanwhile.
    fn take(&mut self, id: u64) -> Option<Task> {
        let index = self.tasks.iter().position(|t| t.id == id)?;
        Some(self.tasks.swap_remove(index))
    }

    fn cancel(&mut self, thread: &Thread) {
        self.tasks.retain(|t| t.thread != *thread);
    }
}

pub(crate) fn step(ctx: &Context, dt: f64) {
    let due = {
        let mut scheduler = ctx.scheduler.borrow_mut();
        scheduler.now += dt;
        scheduler.due()
    };
    for id in due {
        let Some(task) = ctx.scheduler.borrow_mut().take(id) else {
            continue;
        };
        let args = match task.args {
            Some(args) => args,
            None => {
                let now = ctx.scheduler.borrow().now;
                MultiValue::from_vec(vec![Value::Number(now - task.since), Value::Number(now)])
            }
        };
        run_thread(ctx, &task.thread, args);
    }
}

/// Resumes `thread` and handles what it did: finished, failed, or yielded to
/// sleep. The borrow of the scheduler is never held across the resume.
pub(crate) fn run_thread(ctx: &Context, thread: &Thread, args: MultiValue) {
    let previous = ctx.resume_started.replace(Some(Instant::now()));
    let result = thread.resume::<MultiValue>(args);
    ctx.resume_started.set(previous);
    match result {
        Err(err) => ctx.emit(OutputKind::Error, error_text(&err)),
        Ok(values) if thread.status() == ThreadStatus::Resumable => sleep_thread(ctx, thread, values),
        Ok(_) => {}
    }
}

/// A yield of `(marker, seconds)` puts the thread to sleep. Any other yield has
/// no one to resume it, so the thread is dropped.
fn sleep_thread(ctx: &Context, thread: &Thread, values: MultiValue) {
    let marker = ctx.wait_marker.borrow().clone();
    let mut values = values.into_iter();
    let is_wait = matches!(values.next(), Some(Value::Table(t)) if Some(&t) == marker.as_ref());
    if !is_wait {
        return;
    }
    let seconds = match values.next() {
        Some(Value::Number(n)) => n,
        Some(Value::Integer(i)) => i as f64,
        _ => 0.0,
    };
    let mut scheduler = ctx.scheduler.borrow_mut();
    let wake_at = scheduler.now + seconds.max(MIN_WAIT);
    scheduler.push(thread.clone(), wake_at, None);
}

/// Luau's message for a failed script: the error and, from mlua, the stack
/// trace. The Display form would add a "runtime error:" prefix, so use the raw message.
fn error_text(err: &mlua::Error) -> String {
    match err {
        mlua::Error::RuntimeError(message) => message.clone(),
        other => other.to_string(),
    }
}

/// Starts `thread` at once (`task.spawn`) or queues it to start after `delay` seconds.
fn queue(ctx: &Context, thread: Thread, delay: Option<f64>, args: MultiValue) {
    let mut scheduler = ctx.scheduler.borrow_mut();
    let wake_at = scheduler.now + delay.unwrap_or(0.0).max(0.0);
    scheduler.push(thread, wake_at, Some(args));
}

fn to_thread(lua: &Lua, callee: Value) -> mlua::Result<Thread> {
    match callee {
        Value::Thread(thread) => Ok(thread),
        Value::Function(f) => lua.create_thread(f),
        other => Err(mlua::Error::runtime(format!("expected a function or thread, got {}", other.type_name()))),
    }
}

/// Registers the Rust side of the task library in `natives`; the prelude wraps it.
pub(crate) fn install_natives(lua: &Lua, natives: &Table) -> mlua::Result<()> {
    natives.set(
        "spawn",
        lua.create_function(|lua, mut args: MultiValue| {
            let thread = to_thread(lua, args.pop_front().unwrap_or(Value::Nil))?;
            run_thread(&context(lua), &thread, args);
            Ok(thread)
        })?,
    )?;
    natives.set(
        "defer",
        lua.create_function(|lua, mut args: MultiValue| {
            let thread = to_thread(lua, args.pop_front().unwrap_or(Value::Nil))?;
            queue(&context(lua), thread.clone(), None, args);
            Ok(thread)
        })?,
    )?;
    natives.set(
        "delay",
        lua.create_function(|lua, mut args: MultiValue| {
            let seconds = match args.pop_front() {
                Some(Value::Number(n)) => n,
                Some(Value::Integer(i)) => i as f64,
                _ => 0.0,
            };
            let thread = to_thread(lua, args.pop_front().unwrap_or(Value::Nil))?;
            queue(&context(lua), thread.clone(), Some(seconds), args);
            Ok(thread)
        })?,
    )?;
    natives.set(
        "cancel",
        lua.create_function(|lua, thread: Thread| {
            context(lua).scheduler.borrow_mut().cancel(&thread);
            Ok(())
        })?,
    )?;
    natives.set(
        "sleep",
        lua.create_function(|lua, (thread, seconds): (Thread, Option<f64>)| {
            let ctx = context(lua);
            let mut scheduler = ctx.scheduler.borrow_mut();
            let wake_at = scheduler.now + seconds.unwrap_or(0.0).max(MIN_WAIT);
            scheduler.push(thread, wake_at, None);
            Ok(())
        })?,
    )?;
    natives.set("clock", lua.create_function(|lua, ()| Ok(context(lua).scheduler.borrow().now))?)?;
    natives.set(
        "unixClock",
        lua.create_function(|lua, ()| {
            let ctx = context(lua);
            let now = ctx.scheduler.borrow().now;
            Ok(ctx.start_unix + now)
        })?,
    )?;
    Ok(())
}
