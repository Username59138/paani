//! Roblox `CFrame`: a rigid transform stored as a `glam::Mat4` whose columns
//! are the X, Y and Z axes and whose fourth column is the position.

use kahua_render::glam::{Mat3, Mat4, Vec3};
use mlua::{AnyUserData, Lua, MetaMethod, MultiValue, UserData, UserDataFields, UserDataMethods, Value};

use super::vector3::{LuaVector3, as_vector};

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LuaCFrame(pub Mat4);

fn cframe_arg(value: &Value) -> mlua::Result<Mat4> {
    match value {
        Value::UserData(ud) => ud.borrow::<LuaCFrame>().map(|c| c.0).map_err(|_| expected_cframe()),
        _ => Err(expected_cframe()),
    }
}

fn expected_cframe() -> mlua::Error {
    mlua::Error::runtime("expected a CFrame")
}

fn vector_arg(value: &Value) -> mlua::Result<Vec3> {
    as_vector(value).ok_or_else(|| mlua::Error::runtime("expected a Vector3"))
}

fn from_pos_rot(position: Vec3, rotation: Mat3) -> LuaCFrame {
    LuaCFrame(Mat4::from_cols(
        rotation.x_axis.extend(0.0),
        rotation.y_axis.extend(0.0),
        rotation.z_axis.extend(0.0),
        position.extend(1.0),
    ))
}

/// Rotation only, from the upper 3x3 of a CFrame.
fn rotation_of(m: &Mat4) -> Mat3 {
    Mat3::from_cols(m.x_axis.truncate(), m.y_axis.truncate(), m.z_axis.truncate())
}

/// Roblox `CFrame.Angles`: X then Y then Z, all intrinsic: Rx * Ry * Rz.
fn angles_xyz(rx: f32, ry: f32, rz: f32) -> Mat3 {
    Mat3::from_rotation_x(rx) * Mat3::from_rotation_y(ry) * Mat3::from_rotation_z(rz)
}

/// Roblox `CFrame.fromEulerAnglesYXZ` and `fromOrientation`: Ry * Rx * Rz.
fn angles_yxz(rx: f32, ry: f32, rz: f32) -> Mat3 {
    Mat3::from_rotation_y(ry) * Mat3::from_rotation_x(rx) * Mat3::from_rotation_z(rz)
}

/// The frame at `position` whose LookVector (-Z) points at `target`.
fn look_at(position: Vec3, target: Vec3) -> LuaCFrame {
    let z = (position - target).normalize_or_zero();
    let x = Vec3::Y.cross(z).normalize_or_zero();
    let y = z.cross(x);
    from_pos_rot(position, Mat3::from_cols(x, y, z))
}

/// The Roblox `Orientation` of a CFrame: YXZ Euler angles in degrees.
pub(crate) fn orientation_degrees(m: &Mat4) -> Vec3 {
    let (x, y, z) = to_angles_yxz(&rotation_of(m));
    let d = Vec3::new(x, y, z);
    Vec3::new(d.x.to_degrees(), d.y.to_degrees(), d.z.to_degrees())
}

/// The CFrame with its rotation replaced by YXZ angles given in degrees and its position kept.
pub(crate) fn with_orientation(m: &Mat4, degrees: Vec3) -> Mat4 {
    let r = Vec3::new(degrees.x.to_radians(), degrees.y.to_radians(), degrees.z.to_radians());
    let rotation = angles_yxz(r.x, r.y, r.z);
    from_pos_rot(m.w_axis.truncate(), rotation).0
}

/// Euler angles (x, y, z) such that `angles_xyz(x, y, z)` gives this rotation.
fn to_angles_xyz(r: &Mat3) -> (f32, f32, f32) {
    let y = r.z_axis.x.clamp(-1.0, 1.0).asin();
    let x = (-r.z_axis.y).atan2(r.z_axis.z);
    let z = (-r.y_axis.x).atan2(r.x_axis.x);
    (x, y, z)
}

/// Euler angles (x, y, z) such that `angles_yxz(x, y, z)` gives this rotation.
fn to_angles_yxz(r: &Mat3) -> (f32, f32, f32) {
    let x = (-r.z_axis.y).clamp(-1.0, 1.0).asin();
    let y = r.z_axis.x.atan2(r.z_axis.z);
    let z = r.x_axis.y.atan2(r.y_axis.y);
    (x, y, z)
}

/// Row-major rotation entries, as `GetComponents` returns them.
fn rotation_rows(r: &Mat3) -> [f32; 9] {
    [
        r.x_axis.x, r.y_axis.x, r.z_axis.x, //
        r.x_axis.y, r.y_axis.y, r.z_axis.y, //
        r.x_axis.z, r.y_axis.z, r.z_axis.z,
    ]
}

fn components(m: &Mat4) -> [f32; 12] {
    let p = m.w_axis.truncate();
    let r = rotation_rows(&rotation_of(m));
    [p.x, p.y, p.z, r[0], r[1], r[2], r[3], r[4], r[5], r[6], r[7], r[8]]
}

/// CFrame.new with 0, 1, 2, 3 or 12 numeric or Vector3 arguments.
fn construct(args: MultiValue) -> mlua::Result<LuaCFrame> {
    let args: Vec<Value> = args.into_iter().collect();
    let numbers = |values: &[Value]| -> mlua::Result<Vec<f32>> {
        values
            .iter()
            .map(|v| match v {
                Value::Number(n) => Ok(*n as f32),
                Value::Integer(i) => Ok(*i as f32),
                _ => Err(mlua::Error::runtime("expected a number")),
            })
            .collect()
    };
    match args.as_slice() {
        [] => Ok(LuaCFrame(Mat4::IDENTITY)),
        [pos] if as_vector(pos).is_some() => Ok(from_pos_rot(vector_arg(pos)?, Mat3::IDENTITY)),
        [pos, target] => Ok(look_at(vector_arg(pos)?, vector_arg(target)?)),
        [_, _, _] => {
            let n = numbers(&args)?;
            Ok(from_pos_rot(Vec3::new(n[0], n[1], n[2]), Mat3::IDENTITY))
        }
        args if args.len() == 12 => {
            let n = numbers(args)?;
            let rotation = Mat3::from_cols(
                Vec3::new(n[3], n[6], n[9]),
                Vec3::new(n[4], n[7], n[10]),
                Vec3::new(n[5], n[8], n[11]),
            );
            Ok(from_pos_rot(Vec3::new(n[0], n[1], n[2]), rotation))
        }
        _ => Err(mlua::Error::runtime("CFrame.new expects 0, 1, 2, 3 or 12 arguments")),
    }
}

impl UserData for LuaCFrame {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("Position", |_, c| Ok(LuaVector3(c.0.w_axis.truncate())));
        fields.add_field_method_get("X", |_, c| Ok(c.0.w_axis.x));
        fields.add_field_method_get("Y", |_, c| Ok(c.0.w_axis.y));
        fields.add_field_method_get("Z", |_, c| Ok(c.0.w_axis.z));
        fields.add_field_method_get("LookVector", |_, c| Ok(LuaVector3(-c.0.z_axis.truncate())));
        fields.add_field_method_get("RightVector", |_, c| Ok(LuaVector3(c.0.x_axis.truncate())));
        fields.add_field_method_get("UpVector", |_, c| Ok(LuaVector3(c.0.y_axis.truncate())));
        fields.add_field_method_get("Rotation", |_, c| Ok(LuaCFrame(Mat4::from_mat3(rotation_of(&c.0)))));
    }

    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("Inverse", |_, c, ()| Ok(LuaCFrame(c.0.inverse())));
        methods.add_method("Lerp", |_, c, (goal, alpha): (Value, f32)| {
            let goal = cframe_arg(&goal)?;
            let (_, ra, ta) = c.0.to_scale_rotation_translation();
            let (_, rb, tb) = goal.to_scale_rotation_translation();
            let rotation = Mat3::from_quat(ra.slerp(rb, alpha));
            Ok(from_pos_rot(ta.lerp(tb, alpha), rotation))
        });
        methods.add_method("ToWorldSpace", |_, c, other: Value| Ok(LuaCFrame(c.0 * cframe_arg(&other)?)));
        methods.add_method("ToObjectSpace", |_, c, other: Value| Ok(LuaCFrame(c.0.inverse() * cframe_arg(&other)?)));
        methods.add_method("PointToWorldSpace", |_, c, v: Value| Ok(LuaVector3(c.0.transform_point3(vector_arg(&v)?))));
        methods.add_method("PointToObjectSpace", |_, c, v: Value| {
            Ok(LuaVector3(c.0.inverse().transform_point3(vector_arg(&v)?)))
        });
        methods
            .add_method("VectorToWorldSpace", |_, c, v: Value| Ok(LuaVector3(c.0.transform_vector3(vector_arg(&v)?))));
        methods.add_method("VectorToObjectSpace", |_, c, v: Value| {
            Ok(LuaVector3(c.0.inverse().transform_vector3(vector_arg(&v)?)))
        });
        methods.add_method("GetComponents", |_, c, ()| {
            Ok(components(&c.0).into_iter().map(|v| Value::Number(f64::from(v))).collect::<MultiValue>())
        });
        methods.add_method("ToEulerAnglesXYZ", |_, c, ()| {
            let (x, y, z) = to_angles_xyz(&rotation_of(&c.0));
            Ok((x, y, z))
        });
        methods.add_method("ToEulerAnglesYXZ", |_, c, ()| {
            let (x, y, z) = to_angles_yxz(&rotation_of(&c.0));
            Ok((x, y, z))
        });
        methods.add_method("ToOrientation", |_, c, ()| {
            let (x, y, z) = to_angles_yxz(&rotation_of(&c.0));
            Ok((x, y, z))
        });

        methods.add_meta_function(MetaMethod::Mul, |lua, (a, b): (Value, Value)| {
            let frame = cframe_arg(&a)?;
            if let Some(point) = as_vector(&b) {
                return lua.create_userdata(LuaVector3(frame.transform_point3(point))).map(Value::UserData);
            }
            lua.create_userdata(LuaCFrame(frame * cframe_arg(&b)?)).map(Value::UserData)
        });
        methods.add_meta_function(MetaMethod::Add, |_, (a, b): (Value, Value)| {
            let shift = vector_arg(&b)?;
            let mut m = cframe_arg(&a)?;
            m.w_axis += shift.extend(0.0);
            Ok(LuaCFrame(m))
        });
        methods.add_meta_function(MetaMethod::Sub, |_, (a, b): (Value, Value)| {
            let shift = vector_arg(&b)?;
            let mut m = cframe_arg(&a)?;
            m.w_axis -= shift.extend(0.0);
            Ok(LuaCFrame(m))
        });
        methods.add_meta_function(MetaMethod::Eq, |_, (a, b): (AnyUserData, AnyUserData)| {
            Ok(matches!((a.borrow::<LuaCFrame>(), b.borrow::<LuaCFrame>()), (Ok(a), Ok(b)) if a.0 == b.0))
        });
        methods.add_meta_method(MetaMethod::ToString, |_, c, ()| {
            let parts: Vec<String> = components(&c.0).iter().map(|v| v.to_string()).collect();
            Ok(parts.join(", "))
        });
    }
}

pub(crate) fn install(lua: &Lua) -> mlua::Result<()> {
    let table = lua.create_table()?;
    table.set("new", lua.create_function(|_, args: MultiValue| construct(args))?)?;
    table.set("identity", LuaCFrame(Mat4::IDENTITY))?;
    table.set(
        "Angles",
        lua.create_function(|_, (x, y, z): (f32, f32, f32)| Ok(from_pos_rot(Vec3::ZERO, angles_xyz(x, y, z))))?,
    )?;
    table.set(
        "fromEulerAnglesXYZ",
        lua.create_function(|_, (x, y, z): (f32, f32, f32)| Ok(from_pos_rot(Vec3::ZERO, angles_xyz(x, y, z))))?,
    )?;
    let yxz = lua.create_function(|_, (x, y, z): (f32, f32, f32)| Ok(from_pos_rot(Vec3::ZERO, angles_yxz(x, y, z))))?;
    table.set("fromEulerAnglesYXZ", yxz.clone())?;
    table.set("fromOrientation", yxz)?;
    table.set(
        "lookAt",
        lua.create_function(|_, (pos, target): (Value, Value)| Ok(look_at(vector_arg(&pos)?, vector_arg(&target)?)))?,
    )?;
    lua.globals().set("CFrame", table)
}
