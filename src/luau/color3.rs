//! Roblox `Color3` (components 0..1) and `BrickColor` (a palette index with a name).

use kahua_io::rbx_types::{BrickColor, Color3uint8};
use kahua_render::glam::Vec3;
use mlua::{AnyUserData, Lua, MetaMethod, UserData, UserDataFields, UserDataMethods, Value};

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LuaColor3(pub Vec3);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LuaBrickColor(pub BrickColor);

/// Roblox's default when `BrickColor.new` gets a name or number it does not know.
const DEFAULT_BRICK_COLOR: BrickColor = BrickColor::MediumStoneGrey;

fn hsv_to_rgb(h: f32, s: f32, v: f32) -> Vec3 {
    let h = h.rem_euclid(1.0) * 6.0;
    let sector = h.floor();
    let f = h - sector;
    let p = v * (1.0 - s);
    let q = v * (1.0 - s * f);
    let t = v * (1.0 - s * (1.0 - f));
    match sector as i32 % 6 {
        0 => Vec3::new(v, t, p),
        1 => Vec3::new(q, v, p),
        2 => Vec3::new(p, v, t),
        3 => Vec3::new(p, q, v),
        4 => Vec3::new(t, p, v),
        _ => Vec3::new(v, p, q),
    }
}

fn rgb_to_hsv(c: Vec3) -> (f32, f32, f32) {
    let max = c.max_element();
    let min = c.min_element();
    let delta = max - min;
    let v = max;
    let s = if max > 0.0 { delta / max } else { 0.0 };
    let h = if delta == 0.0 {
        0.0
    } else if max == c.x {
        ((c.y - c.z) / delta).rem_euclid(6.0)
    } else if max == c.y {
        (c.z - c.x) / delta + 2.0
    } else {
        (c.x - c.y) / delta + 4.0
    };
    (h / 6.0, s, v)
}

fn parse_hex(text: &str) -> mlua::Result<Vec3> {
    let digits = text.trim_start_matches('#');
    let value = u32::from_str_radix(digits, 16)
        .ok()
        .filter(|_| digits.len() == 6)
        .ok_or_else(|| mlua::Error::runtime(format!("invalid hex color \"{text}\"")))?;
    Ok(Vec3::new(
        ((value >> 16) & 0xff) as f32 / 255.0,
        ((value >> 8) & 0xff) as f32 / 255.0,
        (value & 0xff) as f32 / 255.0,
    ))
}

fn color_arg(value: &Value) -> mlua::Result<Vec3> {
    match value {
        Value::UserData(ud) => ud.borrow::<LuaColor3>().map(|c| c.0).map_err(|_| expected_color()),
        _ => Err(expected_color()),
    }
}

fn expected_color() -> mlua::Error {
    mlua::Error::runtime("expected a Color3")
}

/// Channel 0..1 as a byte, rounded the way Roblox does.
fn to_byte(channel: f32) -> u32 {
    (channel.clamp(0.0, 1.0) * 255.0).round() as u32
}

impl UserData for LuaColor3 {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("R", |_, c| Ok(c.0.x));
        fields.add_field_method_get("G", |_, c| Ok(c.0.y));
        fields.add_field_method_get("B", |_, c| Ok(c.0.z));
    }

    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("Lerp", |lua, c, (goal, alpha): (Value, f32)| {
            let goal = color_arg(&goal)?;
            lua.create_userdata(LuaColor3(c.0.lerp(goal, alpha)))
        });
        methods.add_method("ToHSV", |_, c, ()| {
            let (h, s, v) = rgb_to_hsv(c.0);
            Ok((h, s, v))
        });
        methods.add_method("ToHex", |_, c, ()| {
            let packed = (to_byte(c.0.x) << 16) | (to_byte(c.0.y) << 8) | to_byte(c.0.z);
            Ok(format!("{packed:06x}"))
        });
        methods.add_meta_method(MetaMethod::Eq, |_, c, other: Value| Ok(color_arg(&other).is_ok_and(|o| o == c.0)));
        methods.add_meta_method(MetaMethod::ToString, |_, c, ()| Ok(format!("{}, {}, {}", c.0.x, c.0.y, c.0.z)));
    }
}

impl UserData for LuaBrickColor {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("Name", |_, b| Ok(b.0.to_string()));
        fields.add_field_method_get("Number", |_, b| Ok(b.0 as u16));
        fields.add_field_method_get("Color", |lua, b| {
            let c = b.0.to_color3uint8();
            lua.create_userdata(LuaColor3(color_from_byte(c)))
        });
    }

    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_meta_method(MetaMethod::Eq, |_, b, other: AnyUserData| {
            Ok(other.borrow::<LuaBrickColor>().is_ok_and(|o| o.0 == b.0))
        });
        methods.add_meta_method(MetaMethod::ToString, |_, b, ()| Ok(b.0.to_string()));
    }
}

fn color_from_byte(c: Color3uint8) -> Vec3 {
    Vec3::new(f32::from(c.r) / 255.0, f32::from(c.g) / 255.0, f32::from(c.b) / 255.0)
}

fn brick_color_from_value(value: &Value) -> BrickColor {
    match value {
        Value::String(s) => s.to_str().ok().and_then(|s| BrickColor::from_name(&s)).unwrap_or(DEFAULT_BRICK_COLOR),
        Value::Number(n) => BrickColor::from_number(*n as u16).unwrap_or(DEFAULT_BRICK_COLOR),
        Value::Integer(i) => BrickColor::from_number(*i as u16).unwrap_or(DEFAULT_BRICK_COLOR),
        _ => DEFAULT_BRICK_COLOR,
    }
}

/// Every BrickColor with a palette number, for `BrickColor.random`.
fn palette() -> Vec<BrickColor> {
    (0..=u16::MAX).filter_map(BrickColor::from_number).collect()
}

pub(crate) fn install(lua: &Lua) -> mlua::Result<()> {
    let color = lua.create_table()?;
    color.set(
        "new",
        lua.create_function(|lua, (r, g, b): (Option<f32>, Option<f32>, Option<f32>)| {
            lua.create_userdata(LuaColor3(Vec3::new(r.unwrap_or(0.0), g.unwrap_or(0.0), b.unwrap_or(0.0))))
        })?,
    )?;
    color.set(
        "fromRGB",
        lua.create_function(|lua, (r, g, b): (Option<f32>, Option<f32>, Option<f32>)| {
            let channel = |v: Option<f32>| v.unwrap_or(0.0) / 255.0;
            lua.create_userdata(LuaColor3(Vec3::new(channel(r), channel(g), channel(b))))
        })?,
    )?;
    color.set(
        "fromHSV",
        lua.create_function(|lua, (h, s, v): (Option<f32>, Option<f32>, Option<f32>)| {
            lua.create_userdata(LuaColor3(hsv_to_rgb(h.unwrap_or(0.0), s.unwrap_or(0.0), v.unwrap_or(0.0))))
        })?,
    )?;
    color
        .set("fromHex", lua.create_function(|lua, text: String| lua.create_userdata(LuaColor3(parse_hex(&text)?)))?)?;
    lua.globals().set("Color3", color)?;

    let brick = lua.create_table()?;
    brick.set(
        "new",
        lua.create_function(|lua, value: Value| lua.create_userdata(LuaBrickColor(brick_color_from_value(&value))))?,
    )?;
    brick.set(
        "random",
        lua.create_function(|lua, ()| {
            let colors = palette();
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.subsec_nanos() as usize);
            let pick = colors[nanos % colors.len()];
            lua.create_userdata(LuaBrickColor(pick))
        })?,
    )?;
    lua.globals().set("BrickColor", brick)
}
