//! Roblox events. Signals and connections are Luau objects (prelude.luau);
//! this side decides which members are events and fires them, always with
//! the DOM unborrowed, because handlers run right away and use it too.

use kahua_io::rbx_dom_weak::WeakDom;
use kahua_io::rbx_dom_weak::types::Ref;
use kahua_io::reflect;
use mlua::{IntoLuaMulti, Lua, Table, Value};

use super::instance::InstanceRef;
use super::{OutputKind, context};
use crate::rig;

/// Whether `key` is an event of `class`.
pub(crate) fn is_event(class: &str, key: &str) -> bool {
    match key {
        "Changed" | "ChildAdded" | "ChildRemoved" | "DescendantAdded" | "DescendantRemoving" | "AncestryChanged"
        | "Destroying" | "AttributeChanged" => true,
        "Touched" | "TouchEnded" => reflect::is_base_part(class),
        "Died" | "HealthChanged" => class == "Humanoid",
        "PlayerAdded" | "PlayerRemoving" => class == "Players",
        "CharacterAdded" | "CharacterRemoving" => class == "Player",
        "Heartbeat" | "Stepped" | "RenderStepped" | "PreSimulation" | "PostSimulation" | "PreRender" => {
            class == "RunService"
        }
        "Event" => class == "BindableEvent",
        _ => false,
    }
}

/// Signals are stored by instance and event name.
fn key(r: Ref, event: &str) -> String {
    format!("{r}:{event}")
}

fn signals(lua: &Lua) -> Option<Table> {
    context(lua).signals.borrow().clone()
}

/// The signal for `event` on `r`, created on first use.
pub(crate) fn signal(lua: &Lua, r: Ref, event: &str) -> mlua::Result<Value> {
    let signals = signals(lua).expect("the prelude installs the signals");
    signals.get::<mlua::Function>("get")?.call(key(r, event))
}

/// Runs the handlers connected to `event` on `r`.
pub(crate) fn fire(lua: &Lua, r: Ref, event: &str, args: impl IntoLuaMulti) {
    let Some(signals) = signals(lua) else { return };
    let result = signals.get::<mlua::Function>("fire").and_then(|f| f.call::<()>((key(r, event), args)));
    if let Err(err) = result {
        context(lua).emit(OutputKind::Error, err.to_string());
    }
}

/// Forgets the signals of destroyed instances, disconnecting their handlers.
pub(crate) fn forget(lua: &Lua, gone: &[Ref]) {
    let Some(signals) = signals(lua) else { return };
    let Ok(forget) = signals.get::<mlua::Function>("forget") else { return };
    for &r in gone {
        let _ = forget.call::<()>(format!("{r}:"));
    }
}

/// What a property write changed, recorded while the DOM was borrowed.
pub(crate) enum Change {
    Property,
    Parent { old: Option<Ref> },
    Health { old: f32 },
}

/// Fires the events for a property write: Changed, GetPropertyChangedSignal,
/// and the Parent and Humanoid health events.
pub(crate) fn changed(lua: &Lua, r: Ref, property: &str, change: Change) {
    fire(lua, r, "Changed", property.to_string());
    fire(lua, r, &format!("prop:{property}"), ());
    match change {
        Change::Property => {}
        Change::Parent { old } => parented(lua, r, old),
        Change::Health { old } => {
            let health = rig::float(&context(lua).dom.borrow(), Some(r), "Health", 0.0);
            fire(lua, r, "HealthChanged", health);
            if old > 0.0 && health <= 0.0 {
                fire(lua, r, "Died", ());
            }
        }
    }
}

/// The tree events after `r` moved from `old` to its current parent.
pub(crate) fn parented(lua: &Lua, r: Ref, old: Option<Ref>) {
    let ctx = context(lua);
    let (new, old_chain, new_chain) = {
        let dom = ctx.dom.borrow();
        let new = dom.get_by_ref(r).map(|i| i.parent()).filter(|p| p.is_some());
        (new, chain(&dom, old), chain(&dom, new))
    };
    let inst = InstanceRef(r);
    if let Some(old) = old {
        fire(lua, old, "ChildRemoved", inst);
    }
    for ancestor in old_chain {
        fire(lua, ancestor, "DescendantRemoving", inst);
    }
    if let Some(new) = new {
        fire(lua, new, "ChildAdded", inst);
    }
    for ancestor in new_chain {
        fire(lua, ancestor, "DescendantAdded", inst);
    }
    fire(lua, r, "AncestryChanged", (inst, new.map(InstanceRef)));
}

/// `start` and its ancestors, nearest first.
fn chain(dom: &WeakDom, start: Option<Ref>) -> Vec<Ref> {
    let mut out = Vec::new();
    let mut current = start;
    while let Some(r) = current.filter(|r| r.is_some()) {
        let Some(inst) = dom.get_by_ref(r) else { break };
        out.push(r);
        current = Some(inst.parent());
    }
    out
}

/// Registers `signalKey(instance, event)` for the prelude's Luau-side events.
pub(crate) fn install_natives(lua: &Lua, natives: &Table) -> mlua::Result<()> {
    natives.set("signalKey", lua.create_function(|_, (inst, event): (InstanceRef, String)| Ok(key(inst.0, &event)))?)
}
