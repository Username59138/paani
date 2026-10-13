//! An Instance seen from Luau: a userdata holding the DOM referent. Lookups go
//! through the methods, then the Luau-implemented methods, then the Name,
//! ClassName and Parent fields, then stored properties, then children.

use kahua_io::rbx_dom_weak::types::{Attributes, CFrame, Color3, Matrix3, Ref, Variant, Vector3};
use kahua_io::rbx_dom_weak::{Instance, InstanceBuilder, Ustr, WeakDom};
use kahua_io::reflect;
use kahua_render::glam::{Mat4, Vec3};
use kahua_render::scene::{cframe_to_mat4, mat4_to_cframe};
use mlua::{FromLua, Lua, MetaMethod, Table, UserData, UserDataMethods, Value};

use super::cframe::{LuaCFrame, orientation_degrees, with_orientation};
use super::color3::{LuaBrickColor, LuaColor3};
use super::convert::{to_lua, to_variant};
use super::script::full_name;
use super::vector3::{LuaVector3, as_vector};
use super::{Context, context};
use crate::rig;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct InstanceRef(pub Ref);

/// Services `GetService` creates when the place does not have them.
const CREATABLE_SERVICES: &[&str] = &[
    "Players",
    "RunService",
    "ReplicatedStorage",
    "ServerStorage",
    "ServerScriptService",
    "Lighting",
    "StarterGui",
    "StarterPlayer",
    "TweenService",
    "Debris",
    "UserInputService",
    "SoundService",
];

fn destroyed() -> mlua::Error {
    mlua::Error::runtime("Instance no longer exists")
}

fn not_a_member(class: &str, full: &str, key: &str) -> mlua::Error {
    mlua::Error::runtime(format!("{key} is not a valid member of {class} \"{full}\""))
}

fn parent_of(dom: &WeakDom, r: Ref) -> Option<Ref> {
    let parent = dom.get_by_ref(r)?.parent();
    parent.is_some().then_some(parent)
}

/// Parent, grandparent and so on up to the DataModel.
fn ancestors(dom: &WeakDom, r: Ref) -> Vec<Ref> {
    let mut out = Vec::new();
    let mut current = parent_of(dom, r);
    while let Some(p) = current {
        out.push(p);
        current = parent_of(dom, p);
    }
    out
}

/// Depth-first, parents before children, not including `r` itself.
fn descendants(dom: &WeakDom, r: Ref, out: &mut Vec<Ref>) {
    let Some(inst) = dom.get_by_ref(r) else {
        return;
    };
    for &child in inst.children() {
        out.push(child);
        descendants(dom, child, out);
    }
}

fn find_child(dom: &WeakDom, parent: Ref, recursive: bool, matches: &dyn Fn(&Instance) -> bool) -> Option<Ref> {
    let inst = dom.get_by_ref(parent)?;
    let direct = inst.children().iter().copied().find(|&c| dom.get_by_ref(c).is_some_and(matches));
    if direct.is_some() || !recursive {
        return direct;
    }
    inst.children().iter().find_map(|&c| find_child(dom, c, true, matches))
}

/// Runs `f` on the DOM. Only DOM reads happen inside; nothing calls back into Lua.
fn dom_ref<R>(lua: &Lua, f: impl FnOnce(&WeakDom) -> R) -> R {
    let ctx = context(lua);
    f(&ctx.dom.borrow())
}

fn instance_arg(value: &Value) -> Option<Ref> {
    match value {
        Value::UserData(ud) => ud.borrow::<InstanceRef>().ok().map(|r| r.0),
        _ => None,
    }
}

impl FromLua for InstanceRef {
    fn from_lua(value: Value, _: &Lua) -> mlua::Result<Self> {
        instance_arg(&value)
            .map(InstanceRef)
            .ok_or_else(|| mlua::Error::runtime(format!("expected an Instance, got {}", value.type_name())))
    }
}

/// What a `key` on an instance means, decided while the DOM is borrowed.
enum Member {
    Text(String),
    Parent(Option<Ref>),
    Position(Vec3),
    Orientation(Vec3),
    Stored(Variant, Option<&'static str>),
    Child(Ref),
    Missing,
}

fn lookup(dom: &WeakDom, r: Ref, key: &str) -> Member {
    let Some(inst) = dom.get_by_ref(r) else {
        return Member::Missing;
    };
    let class = inst.class.as_str();
    match key {
        "Name" => return Member::Text(inst.name.clone()),
        "ClassName" => return Member::Text(class.to_string()),
        "Parent" => return Member::Parent(parent_of(dom, r)),
        _ => {}
    }
    if reflect::is_base_part(class) && matches!(key, "Position" | "Orientation") {
        let cframe = stored_cframe(dom, r).unwrap_or(Mat4::IDENTITY);
        return match key {
            "Position" => Member::Position(cframe.w_axis.truncate()),
            _ => Member::Orientation(orientation_degrees(&cframe)),
        };
    }
    if class == "Players" && key == "LocalPlayer" {
        let first = inst.children().iter().copied().find(|&c| dom.get_by_ref(c).is_some_and(|i| i.class == "Player"));
        return first.map_or(Member::Missing, Member::Child);
    }
    let stored = inst.properties.get(&Ustr::from(key)).cloned();
    if let Some(value) = stored.or_else(|| reflect::default_value(class, key).cloned()) {
        return Member::Stored(value, reflect::enum_name(class, key));
    }
    match inst.children().iter().copied().find(|&c| dom.get_by_ref(c).is_some_and(|i| i.name == key)) {
        Some(child) => Member::Child(child),
        None => Member::Missing,
    }
}

/// The CFrame an instance has now: its stored value, else the class default.
fn stored_cframe(dom: &WeakDom, r: Ref) -> Option<Mat4> {
    let inst = dom.get_by_ref(r)?;
    let cframe: CFrame = match inst.properties.get(&Ustr::from("CFrame")) {
        Some(Variant::CFrame(cf)) => *cf,
        _ => match reflect::default_value(&inst.class, "CFrame") {
            Some(Variant::CFrame(cf)) => *cf,
            _ => return None,
        },
    };
    Some(cframe_to_mat4(&cframe))
}

fn get(lua: &Lua, r: Ref, key: &str) -> mlua::Result<Value> {
    let ctx = context(lua);
    if let Some(methods) = ctx.instance_methods.borrow().as_ref() {
        let method: Value = methods.get(key)?;
        if !method.is_nil() {
            return Ok(method);
        }
    }
    let member = {
        let dom = ctx.dom.borrow();
        if dom.get_by_ref(r).is_none() {
            return Err(destroyed());
        }
        lookup(&dom, r, key)
    };
    match member {
        Member::Text(text) => Ok(Value::String(lua.create_string(text)?)),
        Member::Parent(None) => Ok(Value::Nil),
        Member::Parent(Some(p)) => Ok(Value::UserData(lua.create_userdata(InstanceRef(p))?)),
        Member::Position(v) => Ok(Value::UserData(lua.create_userdata(LuaVector3(v))?)),
        Member::Orientation(v) => Ok(Value::UserData(lua.create_userdata(LuaVector3(v))?)),
        Member::Stored(value, enum_name) => to_lua(lua, &value, enum_name),
        Member::Child(child) => Ok(Value::UserData(lua.create_userdata(InstanceRef(child))?)),
        Member::Missing => {
            let dom = ctx.dom.borrow();
            let class = dom.get_by_ref(r).map(|i| i.class.to_string()).unwrap_or_default();
            Err(not_a_member(&class, &full_name(&dom, r), key))
        }
    }
}

fn set(lua: &Lua, r: Ref, key: &str, value: Value) -> mlua::Result<()> {
    let ctx = context(lua);
    let mut dom = ctx.dom.borrow_mut();
    let Some(inst) = dom.get_by_ref(r) else {
        return Err(destroyed());
    };
    let class = inst.class.to_string();
    match key {
        "Name" => {
            let name = match &value {
                Value::String(s) => s.to_str()?.to_string(),
                _ => return Err(mlua::Error::runtime("Unable to assign property Name. string expected")),
            };
            dom.get_by_ref_mut(r).expect("checked above").name = name;
        }
        "Parent" => set_parent(&mut dom, r, &value)?,
        "ClassName" => {
            return Err(mlua::Error::runtime("Unable to assign property ClassName. Property is read only"));
        }
        "Position" | "Orientation" if reflect::is_base_part(&class) => {
            let mut cframe = stored_cframe(&dom, r).unwrap_or(Mat4::IDENTITY);
            let vector = as_vector(&value)
                .ok_or_else(|| mlua::Error::runtime(format!("Unable to assign property {key}. Vector3 expected")))?;
            cframe = if key == "Position" {
                let mut m = cframe;
                m.w_axis = vector.extend(1.0);
                m
            } else {
                with_orientation(&cframe, vector)
            };
            store(&mut dom, r, "CFrame", Variant::CFrame(mat4_to_cframe(&cframe)));
        }
        _ => {
            let Some(descriptor) = reflect::property(&class, key) else {
                return Err(not_a_member(&class, &full_name(&dom, r), key));
            };
            let variant = to_variant(&value, &descriptor.data_type, key)?;
            if let Variant::BrickColor(brick) = &variant
                && key == "BrickColor"
            {
                let c = brick.to_color3uint8();
                let color = Color3::new(f32::from(c.r) / 255.0, f32::from(c.g) / 255.0, f32::from(c.b) / 255.0);
                store(&mut dom, r, "Color", Variant::Color3(color));
            }
            store(&mut dom, r, key, variant);
        }
    }
    ctx.dirty.set(true);
    Ok(())
}

fn store(dom: &mut WeakDom, r: Ref, key: &str, value: Variant) {
    if let Some(inst) = dom.get_by_ref_mut(r) {
        inst.properties.insert(Ustr::from(key), value);
    }
}

fn set_parent(dom: &mut WeakDom, r: Ref, value: &Value) -> mlua::Result<()> {
    let new_parent = match value {
        Value::Nil => Ref::none(),
        other => instance_arg(other).ok_or_else(|| mlua::Error::runtime("Parent must be an Instance or nil"))?,
    };
    if r == dom.root_ref() {
        return Err(mlua::Error::runtime("Cannot change the Parent of the DataModel"));
    }
    if new_parent.is_some() {
        if dom.get_by_ref(new_parent).is_none() {
            return Err(destroyed());
        }
        if new_parent == r || ancestors(dom, new_parent).contains(&r) {
            return Err(mlua::Error::runtime("Attempt to set Parent to a descendant or to itself"));
        }
    }
    dom.transfer_within(r, new_parent);
    Ok(())
}

/// Attribute values are inferred from the Lua type: numbers are Float64 and
/// tables and functions are rejected.
fn attribute_variant(value: &Value) -> mlua::Result<Variant> {
    Ok(match value {
        Value::Boolean(b) => Variant::Bool(*b),
        Value::Number(n) => Variant::Float64(*n),
        Value::Integer(i) => Variant::Float64(*i as f64),
        Value::String(s) => Variant::String(s.to_str()?.to_string()),
        Value::UserData(ud) => {
            if let Ok(v) = ud.borrow::<LuaVector3>() {
                Variant::Vector3(Vector3::new(v.0.x, v.0.y, v.0.z))
            } else if let Ok(c) = ud.borrow::<LuaColor3>() {
                Variant::Color3(Color3::new(c.0.x, c.0.y, c.0.z))
            } else if let Ok(c) = ud.borrow::<LuaCFrame>() {
                Variant::CFrame(mat4_to_cframe(&c.0))
            } else if let Ok(b) = ud.borrow::<LuaBrickColor>() {
                Variant::BrickColor(b.0)
            } else {
                return Err(mlua::Error::runtime("Unsupported attribute value"));
            }
        }
        _ => return Err(mlua::Error::runtime("Unsupported attribute value")),
    })
}

/// The DataModel's service of this class: an existing child, or one created
/// when `create` is set and the class is in `CREATABLE_SERVICES`.
fn service(dom: &mut WeakDom, class: &str, create: bool) -> Option<Ref> {
    if let Some(existing) = rig::service(dom, class) {
        return Some(existing);
    }
    if !(create && CREATABLE_SERVICES.contains(&class)) {
        return None;
    }
    let root = dom.root_ref();
    Some(dom.insert(root, InstanceBuilder::new(class).with_name(class)))
}

/// Drops cached `require` results for destroyed ModuleScripts so a new
/// instance with the same referent cannot see a stale value.
fn forget_modules(ctx: &Context, gone: &[Ref]) {
    ctx.modules.borrow_mut().retain(|r, _| !gone.contains(r));
}

fn child_list(lua: &Lua, refs: Vec<Ref>) -> mlua::Result<Table> {
    let table = lua.create_table()?;
    for (index, r) in refs.into_iter().enumerate() {
        table.set(index + 1, lua.create_userdata(InstanceRef(r))?)?;
    }
    Ok(table)
}

/// Looks up or creates the service `name` under the DataModel.
fn service_named(lua: &Lua, name: &str, create: bool) -> mlua::Result<Option<Ref>> {
    let ctx = context(lua);
    let mut dom = ctx.dom.borrow_mut();
    let existed = rig::service(&dom, name).is_some();
    let found = service(&mut dom, name, create);
    if !existed && found.is_some() {
        ctx.dirty.set(true);
    }
    Ok(found)
}

impl UserData for InstanceRef {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("FindFirstChild", |lua, this, (name, recursive): (String, Option<bool>)| {
            let found = dom_ref(lua, |dom| find_child(dom, this.0, recursive.unwrap_or(false), &|i| i.name == name));
            Ok(found.map(InstanceRef))
        });
        methods.add_method("FindFirstChildOfClass", |lua, this, class: String| {
            let found = dom_ref(lua, |dom| find_child(dom, this.0, false, &|i| i.class.as_str() == class));
            Ok(found.map(InstanceRef))
        });
        methods.add_method("FindFirstChildWhichIsA", |lua, this, class: String| {
            let found =
                dom_ref(lua, |dom| find_child(dom, this.0, false, &|i| reflect::is_a(i.class.as_str(), &class)));
            Ok(found.map(InstanceRef))
        });
        methods.add_method("FindFirstAncestor", |lua, this, name: String| {
            let found = dom_ref(lua, |dom| {
                ancestors(dom, this.0).into_iter().find(|&a| dom.get_by_ref(a).is_some_and(|i| i.name == name))
            });
            Ok(found.map(InstanceRef))
        });
        methods.add_method("FindFirstAncestorOfClass", |lua, this, class: String| {
            let found = dom_ref(lua, |dom| {
                ancestors(dom, this.0)
                    .into_iter()
                    .find(|&a| dom.get_by_ref(a).is_some_and(|i| i.class.as_str() == class))
            });
            Ok(found.map(InstanceRef))
        });
        methods.add_method("GetChildren", |lua, this, ()| {
            let refs = dom_ref(lua, |dom| dom.get_by_ref(this.0).map(|i| i.children().to_vec()).unwrap_or_default());
            child_list(lua, refs)
        });
        methods.add_method("GetDescendants", |lua, this, ()| {
            let mut refs = Vec::new();
            dom_ref(lua, |dom| descendants(dom, this.0, &mut refs));
            child_list(lua, refs)
        });
        methods.add_method("IsA", |lua, this, class: String| {
            Ok(dom_ref(lua, |dom| dom.get_by_ref(this.0).is_some_and(|i| reflect::is_a(i.class.as_str(), &class))))
        });
        methods.add_method("IsDescendantOf", |lua, this, other: InstanceRef| {
            Ok(dom_ref(lua, |dom| ancestors(dom, this.0).contains(&other.0)))
        });
        methods.add_method("IsAncestorOf", |lua, this, other: InstanceRef| {
            Ok(dom_ref(lua, |dom| ancestors(dom, other.0).contains(&this.0)))
        });
        methods.add_method("Destroy", |lua, this, ()| {
            let ctx = context(lua);
            let mut dom = ctx.dom.borrow_mut();
            if this.0 == dom.root_ref() {
                return Err(mlua::Error::runtime("Cannot destroy the DataModel"));
            }
            if dom.get_by_ref(this.0).is_some() {
                let mut gone = vec![this.0];
                descendants(&dom, this.0, &mut gone);
                dom.destroy(this.0);
                forget_modules(&ctx, &gone);
                ctx.dirty.set(true);
            }
            Ok(())
        });
        methods.add_method("ClearAllChildren", |lua, this, ()| {
            let ctx = context(lua);
            let mut dom = ctx.dom.borrow_mut();
            let children = dom.get_by_ref(this.0).map(|i| i.children().to_vec()).unwrap_or_default();
            let mut gone = Vec::new();
            for child in children {
                gone.push(child);
                descendants(&dom, child, &mut gone);
                dom.destroy(child);
            }
            forget_modules(&ctx, &gone);
            ctx.dirty.set(true);
            Ok(())
        });
        methods.add_method("Clone", |lua, this, ()| {
            let ctx = context(lua);
            let mut dom = ctx.dom.borrow_mut();
            if dom.get_by_ref(this.0).is_none() {
                return Err(destroyed());
            }
            let clone = dom.clone_within(this.0);
            ctx.dirty.set(true);
            Ok(InstanceRef(clone))
        });
        methods.add_method("GetFullName", |lua, this, ()| Ok(dom_ref(lua, |dom| full_name(dom, this.0))));
        methods.add_method("GetService", |lua, _, name: String| {
            service_named(lua, &name, true)?
                .map(InstanceRef)
                .ok_or_else(|| mlua::Error::runtime(format!("\"{name}\" is not a valid service")))
        });
        methods
            .add_method("FindService", |lua, _, name: String| Ok(service_named(lua, &name, false)?.map(InstanceRef)));
        methods.add_method("GetAttribute", |lua, this, name: String| {
            let value = dom_ref(lua, |dom| match dom.get_by_ref(this.0)?.properties.get(&Ustr::from("Attributes")) {
                Some(Variant::Attributes(attributes)) => attributes.get(name.as_str()).cloned(),
                _ => None,
            });
            match value {
                Some(v) => to_lua(lua, &v, None),
                None => Ok(Value::Nil),
            }
        });
        methods.add_method("SetAttribute", |lua, this, (name, value): (String, Value)| {
            let variant = if value.is_nil() { None } else { Some(attribute_variant(&value)?) };
            let ctx = context(lua);
            let mut dom = ctx.dom.borrow_mut();
            let Some(inst) = dom.get_by_ref_mut(this.0) else {
                return Err(destroyed());
            };
            let entry = inst
                .properties
                .entry(Ustr::from("Attributes"))
                .or_insert_with(|| Variant::Attributes(Attributes::default()));
            if let Variant::Attributes(attributes) = entry {
                match variant {
                    Some(v) => {
                        attributes.insert(name, v);
                    }
                    None => {
                        attributes.remove(name.as_str());
                    }
                }
            }
            ctx.dirty.set(true);
            Ok(())
        });

        methods.add_meta_method(MetaMethod::Index, |lua, this, key: String| get(lua, this.0, &key));
        methods.add_meta_method(MetaMethod::NewIndex, |lua, this, (key, value): (String, Value)| {
            set(lua, this.0, &key, value)
        });
        methods.add_meta_method(MetaMethod::Eq, |_, this, other: Value| Ok(instance_arg(&other) == Some(this.0)));
        methods.add_meta_method(MetaMethod::ToString, |lua, this, ()| {
            Ok(dom_ref(lua, |dom| dom.get_by_ref(this.0).map(|i| i.name.clone()).unwrap_or_default()))
        });
    }
}

/// Starting values a new BasePart needs so reads work even where the
/// reflection database has no default: the Roblox part size and an identity CFrame.
fn base_part_defaults(mut builder: InstanceBuilder, class: &str) -> InstanceBuilder {
    if reflect::default_value(class, "Size").is_none() {
        builder = builder.with_property("Size", Vector3::new(4.0, 1.0, 2.0));
    }
    if reflect::default_value(class, "CFrame").is_none() {
        builder = builder.with_property("CFrame", CFrame::new(Vector3::new(0.0, 0.0, 0.0), Matrix3::identity()));
    }
    builder
}

/// The Instance table: `Instance.new(class, parent?)`.
pub(crate) fn install(lua: &Lua) -> mlua::Result<()> {
    let table = lua.create_table()?;
    table.set(
        "new",
        lua.create_function(|lua, (class, parent): (String, Option<InstanceRef>)| {
            if reflect::class(&class).is_none() {
                return Err(mlua::Error::runtime(format!("Unable to create an Instance of type \"{class}\"")));
            }
            let ctx = context(lua);
            let mut dom = ctx.dom.borrow_mut();
            let parent_ref = match parent {
                Some(p) if dom.get_by_ref(p.0).is_some() => p.0,
                Some(_) => return Err(destroyed()),
                None => Ref::none(),
            };
            let mut builder = InstanceBuilder::new(class.as_str()).with_name(class.clone());
            if reflect::is_base_part(&class) {
                builder = base_part_defaults(builder, &class);
            }
            let created = dom.insert(parent_ref, builder);
            ctx.dirty.set(true);
            Ok(InstanceRef(created))
        })?,
    )?;
    lua.globals().set("Instance", table)
}
