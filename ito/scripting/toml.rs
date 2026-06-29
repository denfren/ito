//! `toml` module: convert between TOML text and native Lua values.
//!
//! - `toml.parse(text)` deserializes via the `toml` crate into a
//!   `toml::Value`, then converts to a Lua value (tables, strings,
//!   numbers, bools; datetimes become strings), so scripts can inspect
//!   TOML content with ordinary Lua indexing and iteration.
//! - `toml.to_string(value)` / `toml.to_string_pretty(value)` serialize a
//!   Lua value back to TOML text. The top-level value must be a table
//!   with string keys (TOML has no top-level array/scalar form).

use mlua::{Lua, LuaSerdeExt, Value};

/// Register the `toml` module on `lua`.
pub fn register(lua: &Lua) -> mlua::Result<()> {
    let module = lua.create_table()?;

    module.set(
        "parse",
        lua.create_function(|lua, text: String| {
            let value: toml::Value = toml::from_str(&text)
                .map_err(|e| mlua::Error::runtime(format!("toml parse error: {e}")))?;
            lua.to_value(&value)
        })?,
    )?;

    module.set(
        "to_string",
        lua.create_function(|lua, value: Value| {
            let v = to_toml_value(lua, value)?;
            toml::to_string(&v)
                .map_err(|e| mlua::Error::runtime(format!("toml serialize error: {e}")))
        })?,
    )?;

    module.set(
        "to_string_pretty",
        lua.create_function(|lua, value: Value| {
            let v = to_toml_value(lua, value)?;
            toml::to_string_pretty(&v)
                .map_err(|e| mlua::Error::runtime(format!("toml serialize error: {e}")))
        })?,
    )?;

    lua.globals().set("toml", module)?;
    Ok(())
}

fn to_toml_value(lua: &Lua, value: Value) -> mlua::Result<toml::Value> {
    lua.from_value(value)
}
