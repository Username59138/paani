//! Global functions and the environment every script sees: print, warn,
//! typeof, game, workspace, and the datatype and Instance tables.

use mlua::{AnyUserData, Function, Lua, MultiValue, Value};

use super::cframe::LuaCFrame;
use super::color3::{LuaBrickColor, LuaColor3};
use super::enums::{EnumItem, EnumType, Enums};
use super::instance::InstanceRef;
use super::vector3::LuaVector3;
use super::{OutputKind, context};
use crate::rig;

pub(crate) fn install(lua: &Lua) -> mlua::Result<()> {
    let globals = lua.globals();
    globals.set("print", lua.create_function(|lua, args: MultiValue| log(lua, OutputKind::Print, args))?)?;
    globals.set("warn", lua.create_function(|lua, args: MultiValue| log(lua, OutputKind::Warn, args))?)?;
    globals.set("typeof", lua.create_function(|_, value: Value| Ok(type_name(&value)))?)?;

    let root = context(lua).dom.borrow().root_ref();
    let game = lua.create_userdata(InstanceRef(root))?;
    globals.set("game", game)?;
    let workspace = rig::service(&context(lua).dom.borrow(), "Workspace");
    if let Some(workspace) = workspace {
        let value = lua.create_userdata(InstanceRef(workspace))?;
        globals.set("workspace", value.clone())?;
        globals.set("Workspace", value)?;
    }

    super::vector3::install(lua)?;
    super::cframe::install(lua)?;
    super::color3::install(lua)?;
    super::enums::install(lua)?;
    super::instance::install(lua)
}

/// Joins the arguments with spaces, each through `tostring`, and records the line.
fn log(lua: &Lua, kind: OutputKind, args: MultiValue) -> mlua::Result<()> {
    let tostring: Function = lua.globals().get("tostring")?;
    let mut parts = Vec::with_capacity(args.len());
    for value in args {
        parts.push(tostring.call::<String>(value)?);
    }
    context(lua).emit(kind, parts.join(" "));
    Ok(())
}

/// Roblox type names for the userdata this runtime defines; other values
/// report their Luau type.
fn type_name(value: &Value) -> String {
    let name = match value {
        Value::UserData(ud) => userdata_type(ud),
        _ => None,
    };
    name.map_or_else(|| value.type_name().to_string(), str::to_string)
}

fn userdata_type(ud: &AnyUserData) -> Option<&'static str> {
    if ud.is::<InstanceRef>() {
        Some("Instance")
    } else if ud.is::<LuaVector3>() {
        Some("Vector3")
    } else if ud.is::<LuaCFrame>() {
        Some("CFrame")
    } else if ud.is::<LuaColor3>() {
        Some("Color3")
    } else if ud.is::<LuaBrickColor>() {
        Some("BrickColor")
    } else if ud.is::<EnumItem>() {
        Some("EnumItem")
    } else if ud.is::<EnumType>() {
        Some("Enum")
    } else if ud.is::<Enums>() {
        Some("Enums")
    } else {
        None
    }
}
