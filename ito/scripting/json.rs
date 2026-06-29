//! `json` module: convert between JSON text and native Lua values.
//!
//! - `json.parse(text)` deserializes via serde_json into a
//!   `serde_json::Value`, then converts to a Lua value (tables, strings,
//!   numbers, bools, nil), so scripts can inspect JSON content with
//!   ordinary Lua indexing and iteration.
//! - `json.to_string(value)` / `json.to_string_pretty(value)` serialize a
//!   Lua value back to JSON text (compact / 2-space indented).
//! - `json.null` — the same null sentinel as `ito.null` (identical value),
//!   exposed here because that is where a caller of `json.parse` looks for
//!   it.

use mlua::{Lua, LuaSerdeExt, Value};

/// Register the `json` module on `lua`.
pub fn register(lua: &Lua) -> mlua::Result<()> {
    let module = lua.create_table()?;

    module.set(
        "parse",
        lua.create_function(|lua, text: String| {
            let value: serde_json::Value = serde_json::from_str(&text)
                .map_err(|e| mlua::Error::runtime(format!("json parse error: {e}")))?;
            lua.to_value(&value)
        })?,
    )?;

    module.set(
        "to_string",
        lua.create_function(|lua, value: Value| {
            let v = to_json_value(lua, value)?;
            serde_json::to_string(&v)
                .map_err(|e| mlua::Error::runtime(format!("json serialize error: {e}")))
        })?,
    )?;

    module.set(
        "to_string_pretty",
        lua.create_function(|lua, value: Value| {
            let v = to_json_value(lua, value)?;
            serde_json::to_string_pretty(&v)
                .map_err(|e| mlua::Error::runtime(format!("json serialize error: {e}")))
        })?,
    )?;

    // Same sentinel as `ito.null` — mlua's null is process-wide, so the two
    // are the identical value, not two nulls that compare equal.
    module.set("null", lua.null())?;

    lua.globals().set("json", module)?;
    Ok(())
}

fn to_json_value(lua: &Lua, value: Value) -> mlua::Result<serde_json::Value> {
    lua.from_value(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_null_is_the_same_value_as_ito_null() {
        let lua = Lua::new();
        register(&lua).unwrap();
        crate::scripting::ito::register(&lua, "0.0.0").unwrap();

        // Identity, not merely equality: the same sentinel under both names,
        // so `ito.is_null` and a round-trip accept either spelling.
        let same: bool = lua.load("return json.null == ito.null").eval().unwrap();
        assert!(same);
        let is_null: bool = lua.load("return ito.is_null(json.null)").eval().unwrap();
        assert!(is_null);

        let out: String = lua
            .load(r#"return json.to_string({a = json.null})"#)
            .eval()
            .unwrap();
        assert_eq!(out, r#"{"a":null}"#);
    }
}
