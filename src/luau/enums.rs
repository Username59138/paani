//! `Enum.Material.Plastic` and friends, built from the reflection database.
//! Enum values are only names and numbers, so nothing is copied up front.

use kahua_io::reflect;
use mlua::{AnyUserData, Lua, MetaMethod, UserData, UserDataFields, UserDataMethods, Value};

/// The `Enum` global: indexing it gives an enum type.
#[derive(Clone, Copy)]
pub(crate) struct Enums;

/// One enum type, such as `Enum.Material`.
#[derive(Clone)]
pub(crate) struct EnumType {
    pub name: &'static str,
}

/// One enum value, such as `Enum.Material.Plastic`.
#[derive(Clone)]
pub(crate) struct EnumItem {
    pub enum_name: &'static str,
    pub name: String,
    pub value: u32,
}

fn enum_descriptor(name: &str) -> Option<&'static str> {
    reflect::db().enums.get_key_value(name).map(|(k, _)| *k)
}

/// The enum item with this value, if the enum defines one.
pub(crate) fn item_by_value(enum_name: &'static str, value: u32) -> Option<EnumItem> {
    let descriptor = reflect::db().enums.get(enum_name)?;
    let (name, _) = descriptor.items.iter().find(|(_, v)| **v == value)?;
    Some(EnumItem { enum_name, name: (*name).to_string(), value })
}

fn item_by_name(enum_name: &'static str, name: &str) -> Option<EnumItem> {
    let descriptor = reflect::db().enums.get(enum_name)?;
    let value = *descriptor.items.get(name)?;
    Some(EnumItem { enum_name, name: name.to_string(), value })
}

impl UserData for Enums {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_meta_method(MetaMethod::Index, |lua, _, key: String| index_enums(lua, &key));
        methods.add_meta_method(MetaMethod::ToString, |_, _, ()| Ok("Enum".to_string()));
    }
}

impl UserData for EnumType {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("Name", |_, e| Ok(e.name));
    }

    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("GetEnumItems", |lua, e, ()| {
            let descriptor = reflect::db().enums.get(e.name);
            let mut items: Vec<(u32, &str)> =
                descriptor.map(|d| d.items.iter().map(|(n, v)| (*v, *n)).collect()).unwrap_or_default();
            items.sort_unstable();
            let table = lua.create_table()?;
            for (index, (value, name)) in items.into_iter().enumerate() {
                let item = EnumItem { enum_name: e.name, name: name.to_string(), value };
                table.set(index + 1, lua.create_userdata(item)?)?;
            }
            Ok(table)
        });
        methods.add_meta_method(MetaMethod::Index, |lua, e, key: String| index_enum_type(lua, e.name, &key));
        methods.add_meta_method(MetaMethod::ToString, |_, e, ()| Ok(e.name.to_string()));
    }
}

impl UserData for EnumItem {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("Name", |_, e| Ok(e.name.clone()));
        fields.add_field_method_get("Value", |_, e| Ok(e.value));
        fields.add_field_method_get("EnumType", |lua, e| lua.create_userdata(EnumType { name: e.enum_name }));
    }

    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_meta_method(MetaMethod::Eq, |_, e, other: AnyUserData| {
            Ok(other.borrow::<EnumItem>().is_ok_and(|o| o.enum_name == e.enum_name && o.value == e.value))
        });
        methods.add_meta_method(MetaMethod::ToString, |_, e, ()| Ok(e.name.clone()));
    }
}

pub(crate) fn install(lua: &Lua) -> mlua::Result<()> {
    let enums = lua.create_userdata(Enums)?;
    lua.globals().set("Enum", enums)
}

/// `Enum.<name>` and `Enum.<type>.<item>` lookups, wired through the metatable.
pub(crate) fn index_enums(lua: &Lua, key: &str) -> mlua::Result<Value> {
    match enum_descriptor(key) {
        Some(name) => lua.create_userdata(EnumType { name }).map(Value::UserData),
        None => Err(mlua::Error::runtime(format!("{key} is not a valid member of Enums"))),
    }
}

pub(crate) fn index_enum_type(lua: &Lua, enum_name: &'static str, key: &str) -> mlua::Result<Value> {
    match item_by_name(enum_name, key) {
        Some(item) => lua.create_userdata(item).map(Value::UserData),
        None => Err(mlua::Error::runtime(format!("{key} is not a valid member of Enum {enum_name}"))),
    }
}
