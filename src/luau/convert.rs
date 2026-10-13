//! Conversions between DOM property values (`rbx_types::Variant`) and Luau values.

use kahua_io::rbx_dom_weak::types::{Color3, Color3uint8, Content, Enum, Ref, Variant, Vector3};
use kahua_io::rbx_reflection::DataType;
use kahua_io::rbx_types::VariantType;
use kahua_render::glam::Vec3;
use kahua_render::scene::{cframe_to_mat4, mat4_to_cframe};
use mlua::{Lua, Value};

use super::cframe::LuaCFrame;
use super::color3::{LuaBrickColor, LuaColor3};
use super::enums::{self, EnumItem};
use super::instance::InstanceRef;
use super::vector3::{LuaVector3, as_vector};

/// The Luau value for a stored property. `enum_name` names the enum for an
/// enum property, so the value can be shown as an EnumItem.
pub(crate) fn to_lua(lua: &Lua, value: &Variant, enum_name: Option<&'static str>) -> mlua::Result<Value> {
    Ok(match value {
        Variant::Bool(b) => Value::Boolean(*b),
        Variant::Int32(i) => Value::Number(f64::from(*i)),
        Variant::Int64(i) => Value::Number(*i as f64),
        Variant::Float32(f) => Value::Number(f64::from(*f)),
        Variant::Float64(f) => Value::Number(*f),
        Variant::String(s) => Value::String(lua.create_string(s)?),
        Variant::Vector3(v) => Value::UserData(lua.create_userdata(LuaVector3(Vec3::new(v.x, v.y, v.z)))?),
        Variant::CFrame(cf) => Value::UserData(lua.create_userdata(LuaCFrame(cframe_to_mat4(cf)))?),
        Variant::Color3(c) => Value::UserData(lua.create_userdata(LuaColor3(Vec3::new(c.r, c.g, c.b)))?),
        Variant::Color3uint8(c) => Value::UserData(lua.create_userdata(LuaColor3(color_from_bytes(c)))?),
        Variant::BrickColor(b) => Value::UserData(lua.create_userdata(LuaBrickColor(*b))?),
        Variant::Enum(e) => match enum_name.and_then(|name| enums::item_by_value(name, e.to_u32())) {
            Some(item) => Value::UserData(lua.create_userdata(item)?),
            None => Value::Number(f64::from(e.to_u32())),
        },
        Variant::Ref(r) if r.is_none() => Value::Nil,
        Variant::Ref(r) => Value::UserData(lua.create_userdata(InstanceRef(*r))?),
        Variant::Content(c) => Value::String(lua.create_string(c.as_uri().unwrap_or(""))?),
        _ => Value::Nil,
    })
}

fn color_from_bytes(c: &Color3uint8) -> Vec3 {
    Vec3::new(f32::from(c.r) / 255.0, f32::from(c.g) / 255.0, f32::from(c.b) / 255.0)
}

/// A Luau value as a property of `data_type`. `key` only feeds the error message.
pub(crate) fn to_variant(value: &Value, data_type: &DataType, key: &str) -> mlua::Result<Variant> {
    let mismatch = |expected: &str| {
        mlua::Error::runtime(format!("Unable to assign property {key}. {expected} expected, got {}", value.type_name()))
    };
    match data_type {
        DataType::Enum(_) => match value {
            Value::UserData(ud) => match ud.borrow::<EnumItem>() {
                Ok(item) => Ok(Variant::Enum(Enum::from_u32(item.value))),
                Err(_) => Err(mismatch("EnumItem")),
            },
            Value::Number(n) => Ok(Variant::Enum(Enum::from_u32(*n as u32))),
            Value::Integer(i) => Ok(Variant::Enum(Enum::from_u32(*i as u32))),
            _ => Err(mismatch("EnumItem")),
        },
        DataType::Value(vt) => value_variant(value, *vt, &mismatch),
        _ => Err(mlua::Error::runtime(format!("Unable to assign property {key}"))),
    }
}

fn value_variant(value: &Value, vt: VariantType, mismatch: &impl Fn(&str) -> mlua::Error) -> mlua::Result<Variant> {
    let number = || match value {
        Value::Number(n) => Some(*n),
        Value::Integer(i) => Some(*i as f64),
        _ => None,
    };
    let vec = || as_vector(value).ok_or_else(|| mismatch("Vector3"));
    Ok(match vt {
        VariantType::Bool => match value {
            Value::Boolean(b) => Variant::Bool(*b),
            _ => return Err(mismatch("boolean")),
        },
        VariantType::String => match value {
            Value::String(s) => Variant::String(s.to_str()?.to_string()),
            _ => return Err(mismatch("string")),
        },
        VariantType::Float32 => Variant::Float32(number().ok_or_else(|| mismatch("number"))? as f32),
        VariantType::Float64 => Variant::Float64(number().ok_or_else(|| mismatch("number"))?),
        VariantType::Int32 => Variant::Int32(number().ok_or_else(|| mismatch("number"))? as i32),
        VariantType::Int64 => Variant::Int64(number().ok_or_else(|| mismatch("number"))? as i64),
        VariantType::Vector3 => {
            let v = vec()?;
            Variant::Vector3(Vector3::new(v.x, v.y, v.z))
        }
        VariantType::CFrame => match value {
            Value::UserData(ud) => match ud.borrow::<LuaCFrame>() {
                Ok(c) => Variant::CFrame(mat4_to_cframe(&c.0)),
                Err(_) => return Err(mismatch("CFrame")),
            },
            _ => return Err(mismatch("CFrame")),
        },
        VariantType::Color3 | VariantType::Color3uint8 => match value {
            Value::UserData(ud) => match ud.borrow::<LuaColor3>() {
                Ok(c) if vt == VariantType::Color3 => Variant::Color3(Color3::new(c.0.x, c.0.y, c.0.z)),
                Ok(c) => Variant::Color3uint8(Color3uint8 {
                    r: (c.0.x * 255.0).round() as u8,
                    g: (c.0.y * 255.0).round() as u8,
                    b: (c.0.z * 255.0).round() as u8,
                }),
                Err(_) => return Err(mismatch("Color3")),
            },
            _ => return Err(mismatch("Color3")),
        },
        VariantType::BrickColor => match value {
            Value::UserData(ud) => match ud.borrow::<LuaBrickColor>() {
                Ok(b) => Variant::BrickColor(b.0),
                Err(_) => return Err(mismatch("BrickColor")),
            },
            _ => return Err(mismatch("BrickColor")),
        },
        VariantType::Ref => match value {
            Value::Nil => Variant::Ref(Ref::none()),
            Value::UserData(ud) => match ud.borrow::<InstanceRef>() {
                Ok(r) => Variant::Ref(r.0),
                Err(_) => return Err(mismatch("Instance")),
            },
            _ => return Err(mismatch("Instance")),
        },
        VariantType::Content => match value {
            Value::String(s) => Variant::Content(Content::from_uri(s.to_str()?.to_string())),
            _ => return Err(mismatch("string")),
        },
        _ => return Err(mlua::Error::runtime(format!("Unable to assign a value of this type to {vt:?}"))),
    })
}
