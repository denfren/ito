//! `toml` module: convert between TOML text and native Luau values.
//!
//! - `toml.parse(text, name?)` → table. Datetimes become their TOML text
//!   (strings); arrays carry the array marker (`table.array`). `name` is
//!   added to parse errors.
//! - `toml.to_string(value, opts?)` → text. The top-level value must be a
//!   table with string keys (TOML has no top-level array/scalar form).
//!   `{pretty = true}` selects the crate's pretty layout (multi-line arrays).
//!   Keys are emitted sorted.
//!
//! TOML has no null: writing `null` raises. Luau numbers are doubles, so
//! integers above 2^53 lose precision and `1.0` writes as `1`. There is no
//! way to write a datetime (a string stays a string).

use super::codec::{self, Options};
use mlua::{Lua, LuaSerdeExt, Table, Value};
use yamlito::Yaml;

fn from_toml(v: toml::Value) -> Yaml {
    match v {
        toml::Value::String(s) => Yaml::Str(s),
        toml::Value::Integer(i) => Yaml::Int(i),
        toml::Value::Float(f) => Yaml::Float(f),
        toml::Value::Boolean(b) => Yaml::Bool(b),
        toml::Value::Datetime(d) => Yaml::Str(d.to_string()),
        toml::Value::Array(a) => Yaml::Seq(a.into_iter().map(from_toml).collect()),
        toml::Value::Table(t) => Yaml::Map(t.into_iter().map(|(k, v)| (k, from_toml(v))).collect()),
    }
}

fn to_toml(y: Yaml, path: &str) -> Result<toml::Value, String> {
    use toml::Value as T;
    Ok(match y {
        Yaml::Null => {
            let at = if path.is_empty() { String::new() } else { format!(" at `{path}`") };
            return Err(format!("toml serialize error{at}: TOML has no null"));
        }
        Yaml::Bool(b) => T::Boolean(b),
        Yaml::Int(i) => T::Integer(i),
        Yaml::Float(f) => T::Float(f),
        Yaml::Str(s) => T::String(s),
        Yaml::Seq(items) => T::Array(
            items
                .into_iter()
                .enumerate()
                .map(|(i, v)| to_toml(v, &format!("{path}[{}]", i + 1)))
                .collect::<Result<_, _>>()?,
        ),
        Yaml::Map(entries) => T::Table(
            entries
                .into_iter()
                .map(|(k, v)| {
                    let p = if path.is_empty() { k.clone() } else { format!("{path}.{k}") };
                    to_toml(v, &p).map(|v| (k, v))
                })
                .collect::<Result<_, _>>()?,
        ),
    })
}

/// Register the `toml` module on `lua`.
pub fn register(lua: &Lua) -> mlua::Result<()> {
    let module = lua.create_table()?;

    module.set(
        "parse",
        lua.create_function(|lua, (text, name): (String, Option<String>)| {
            // the crate's message already starts with "TOML parse error"
            let value: toml::Table = toml::from_str(&text).map_err(|e| match &name {
                Some(n) => mlua::Error::runtime(format!("in {n}: {e}")),
                None => mlua::Error::runtime(e.to_string()),
            })?;
            lua.to_value(&from_toml(toml::Value::Table(value)))
        })?,
    )?;

    module.set(
        "to_string",
        lua.create_function(|lua, (value, opts): (Value, Option<Table>)| {
            let opts = Options::parse(opts, "toml.to_string", false)?;
            let data = codec::to_data(lua, &value, "toml serialize error")?;
            if !matches!(data, Yaml::Map(_)) {
                return Err(mlua::Error::runtime(
                    "toml serialize error: the top-level value must be a table with string keys",
                ));
            }
            let v = to_toml(data, "").map_err(mlua::Error::runtime)?;
            let out = match opts.indent {
                Some(_) => toml::to_string_pretty(&v),
                None => toml::to_string(&v),
            };
            out.map_err(|e| mlua::Error::runtime(format!("toml serialize error: {e}")))
        })?,
    )?;

    lua.globals().set("toml", module)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lua() -> Lua {
        let lua = Lua::new();
        register(&lua).unwrap();
        crate::scripting::null::register(&lua).unwrap();
        codec::register_array(&lua).unwrap();
        lua
    }

    fn ev<T: mlua::FromLua>(src: &str) -> T {
        lua().load(src).eval().unwrap()
    }

    fn err(src: &str) -> String {
        lua().load(src).eval::<Value>().unwrap_err().to_string()
    }

    #[test]
    fn round_trip_with_empty_array() {
        let out: String = ev(r#"return toml.to_string(toml.parse("a = []\n[t]\nb = 1\n"))"#);
        assert_eq!(out, "a = []\n\n[t]\nb = 1\n");
    }

    #[test]
    fn datetime_is_a_string() {
        assert_eq!(
            ev::<String>(r#"return toml.parse("d = 1979-05-27T07:32:00Z").d"#),
            "1979-05-27T07:32:00Z"
        );
    }

    #[test]
    fn null_and_non_table_raise() {
        assert!(err("return toml.to_string({a = {b = null}})").contains("`a.b`: TOML has no null"));
        assert!(err("return toml.to_string(1)").contains("top-level"));
        assert!(err("return toml.to_string({1, x = 2})").contains("mixed"));
    }

    #[test]
    fn parse_error_not_doubled_and_named() {
        let e = err(r#"return toml.parse("a = ", "x.toml")"#);
        assert!(e.contains("in x.toml"));
        assert!(!e.contains("toml parse error"));
    }

    #[test]
    fn pretty_option() {
        let s: String = ev("return toml.to_string({a = {1, 2}}, {pretty = true})");
        assert!(s.contains("[\n"));
    }
}
