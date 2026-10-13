//! Roblox `Vector3`: a value type over `glam::Vec3`.

use kahua_render::glam::Vec3;
use mlua::{AnyUserData, Lua, MetaMethod, UserData, UserDataFields, UserDataMethods, Value};

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LuaVector3(pub Vec3);

/// The vector under a Lua value, if it is a Vector3.
pub(crate) fn as_vector(value: &Value) -> Option<Vec3> {
    match value {
        Value::UserData(ud) => ud.borrow::<LuaVector3>().ok().map(|v| v.0),
        _ => None,
    }
}

fn as_number(value: &Value) -> Option<f32> {
    match value {
        Value::Number(n) => Some(*n as f32),
        Value::Integer(i) => Some(*i as f32),
        _ => None,
    }
}

/// `a op b` for Vector3 with Vector3 or number operands. Scalars apply to
/// both sides of `*` and `/`, as in Roblox.
fn arith(a: &Value, b: &Value, op: fn(Vec3, Vec3) -> Vec3) -> mlua::Result<LuaVector3> {
    match (as_vector(a), as_vector(b)) {
        (Some(a), Some(b)) => Ok(LuaVector3(op(a, b))),
        (Some(a), None) => match as_number(b) {
            Some(n) => Ok(LuaVector3(op(a, Vec3::splat(n)))),
            None => Err(mlua::Error::runtime("attempt to perform arithmetic on a Vector3 and a non-number")),
        },
        (None, Some(b)) => match as_number(a) {
            Some(n) => Ok(LuaVector3(op(Vec3::splat(n), b))),
            None => Err(mlua::Error::runtime("attempt to perform arithmetic on a non-number and a Vector3")),
        },
        (None, None) => Err(mlua::Error::runtime("attempt to perform arithmetic on non-Vector3 values")),
    }
}

fn vector_arg(value: &Value) -> mlua::Result<Vec3> {
    as_vector(value).ok_or_else(|| mlua::Error::runtime("expected a Vector3"))
}

impl UserData for LuaVector3 {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("X", |_, v| Ok(v.0.x));
        fields.add_field_method_get("Y", |_, v| Ok(v.0.y));
        fields.add_field_method_get("Z", |_, v| Ok(v.0.z));
        fields.add_field_method_get("Magnitude", |_, v| Ok(v.0.length()));
        fields.add_field_method_get("Unit", |_, v| Ok(LuaVector3(v.0.normalize())));
    }

    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("Dot", |_, v, other: Value| Ok(v.0.dot(vector_arg(&other)?)));
        methods.add_method("Cross", |_, v, other: Value| Ok(LuaVector3(v.0.cross(vector_arg(&other)?))));
        methods.add_method("Lerp", |_, v, (goal, alpha): (Value, f32)| {
            Ok(LuaVector3(v.0.lerp(vector_arg(&goal)?, alpha)))
        });
        methods.add_method("FuzzyEq", |_, v, (other, epsilon): (Value, Option<f32>)| {
            let epsilon = epsilon.unwrap_or(1e-5);
            Ok((v.0 - vector_arg(&other)?).abs().max_element() <= epsilon)
        });
        methods.add_method("Abs", |_, v, ()| Ok(LuaVector3(v.0.abs())));
        methods.add_method("Floor", |_, v, ()| Ok(LuaVector3(v.0.floor())));
        methods.add_method("Ceil", |_, v, ()| Ok(LuaVector3(v.0.ceil())));
        methods.add_method("Min", |_, v, other: Value| Ok(LuaVector3(v.0.min(vector_arg(&other)?))));
        methods.add_method("Max", |_, v, other: Value| Ok(LuaVector3(v.0.max(vector_arg(&other)?))));

        methods.add_meta_function(MetaMethod::Add, |_, (a, b): (Value, Value)| arith(&a, &b, |a, b| a + b));
        methods.add_meta_function(MetaMethod::Sub, |_, (a, b): (Value, Value)| arith(&a, &b, |a, b| a - b));
        methods.add_meta_function(MetaMethod::Mul, |_, (a, b): (Value, Value)| arith(&a, &b, |a, b| a * b));
        methods.add_meta_function(MetaMethod::Div, |_, (a, b): (Value, Value)| arith(&a, &b, |a, b| a / b));
        methods.add_meta_function(MetaMethod::Unm, |_, a: Value| Ok(LuaVector3(-vector_arg(&a)?)));
        methods.add_meta_function(MetaMethod::Eq, |_, (a, b): (AnyUserData, AnyUserData)| {
            Ok(matches!((a.borrow::<LuaVector3>(), b.borrow::<LuaVector3>()), (Ok(a), Ok(b)) if a.0 == b.0))
        });
        methods.add_meta_method(MetaMethod::ToString, |_, v, ()| Ok(format!("{}, {}, {}", v.0.x, v.0.y, v.0.z)));
    }
}

pub(crate) fn install(lua: &Lua) -> mlua::Result<()> {
    let table = lua.create_table()?;
    table.set(
        "new",
        lua.create_function(|_, (x, y, z): (Option<f32>, Option<f32>, Option<f32>)| {
            Ok(LuaVector3(Vec3::new(x.unwrap_or(0.0), y.unwrap_or(0.0), z.unwrap_or(0.0))))
        })?,
    )?;
    table.set("zero", LuaVector3(Vec3::ZERO))?;
    table.set("one", LuaVector3(Vec3::ONE))?;
    table.set("xAxis", LuaVector3(Vec3::X))?;
    table.set("yAxis", LuaVector3(Vec3::Y))?;
    table.set("zAxis", LuaVector3(Vec3::Z))?;
    lua.globals().set("Vector3", table)
}
