//! `json` module: convert between JSON text and native Luau values.
//!
//! - `json.parse(text, name?)` → value. `null` becomes the global `null`
//!   sentinel; arrays carry the array marker (`table.array`), so an empty
//!   array survives a round trip. `name` (a file name, say) is added to
//!   parse errors.
//! - `json.to_string(value, opts?)` → text. Compact by default;
//!   `{pretty = true}` indents by 2 and ends with a newline, `{indent = n}`
//!   picks the width (and implies `pretty`). Object keys are emitted sorted.
//! - `json.parse_lines(text, name?)` / `json.to_string_lines(array)` — JSON
//!   Lines (one compact value per line, blank lines skipped on read).
//!
//! Lossy cases: Luau numbers are doubles, so integers above 2^53 lose
//! precision on both sides, and `1.0` writes as `1`. Writing raises for
//! NaN/inf, invalid UTF-8, functions/userdata, mixed or sparse tables and
//! cycles (see [`crate::scripting::codec`]).

use super::codec::{self, Options};
use mlua::{Lua, LuaSerdeExt, Table, Value};
use serde::Serialize;
use yamlito::Yaml;

fn parse_err(name: &Option<String>, e: impl std::fmt::Display) -> mlua::Error {
    match name {
        Some(n) => mlua::Error::runtime(format!("json parse error in {n}: {e}")),
        None => mlua::Error::runtime(format!("json parse error: {e}")),
    }
}

/// The data model as a JSON value; NaN/inf have no JSON form.
fn to_json(y: Yaml, path: &str) -> Result<serde_json::Value, String> {
    use serde_json::Value as J;
    Ok(match y {
        Yaml::Null => J::Null,
        Yaml::Bool(b) => J::Bool(b),
        Yaml::Int(i) => J::from(i),
        Yaml::Float(f) => match serde_json::Number::from_f64(f) {
            Some(n) => J::Number(n),
            None => return Err(format!("json serialize error at `{path}`: {f} has no JSON form")),
        },
        Yaml::Str(s) => J::String(s),
        Yaml::Seq(items) => J::Array(
            items
                .into_iter()
                .enumerate()
                .map(|(i, v)| to_json(v, &format!("{path}[{}]", i + 1)))
                .collect::<Result<_, _>>()?,
        ),
        Yaml::Map(entries) => J::Object(
            entries
                .into_iter()
                .map(|(k, v)| {
                    let p = if path.is_empty() { k.clone() } else { format!("{path}.{k}") };
                    to_json(v, &p).map(|v| (k, v))
                })
                .collect::<Result<_, _>>()?,
        ),
    })
}

fn encode(lua: &Lua, value: &Value) -> mlua::Result<serde_json::Value> {
    let data = codec::to_data(lua, value, "json serialize error")?;
    to_json(data, "").map_err(mlua::Error::runtime)
}

fn pretty(v: &serde_json::Value, indent: usize) -> mlua::Result<String> {
    let pad = " ".repeat(indent);
    let mut buf = Vec::new();
    let fmt = serde_json::ser::PrettyFormatter::with_indent(pad.as_bytes());
    v.serialize(&mut serde_json::Serializer::with_formatter(&mut buf, fmt))
        .map_err(|e| mlua::Error::runtime(format!("json serialize error: {e}")))?;
    let mut s = String::from_utf8(buf).map_err(mlua::Error::external)?;
    s.push('\n');
    Ok(s)
}

/// Register the `json` module on `lua`.
pub fn register(lua: &Lua) -> mlua::Result<()> {
    let module = lua.create_table()?;

    module.set(
        "parse",
        lua.create_function(|lua, (text, name): (String, Option<String>)| {
            let value: serde_json::Value =
                serde_json::from_str(&text).map_err(|e| parse_err(&name, e))?;
            lua.to_value(&value)
        })?,
    )?;

    module.set(
        "to_string",
        lua.create_function(|lua, (value, opts): (Value, Option<Table>)| {
            let opts = Options::parse(opts, "json.to_string", true)?;
            let v = encode(lua, &value)?;
            match opts.indent {
                Some(n) => pretty(&v, n),
                None => Ok(v.to_string()),
            }
        })?,
    )?;

    module.set(
        "parse_lines",
        lua.create_function(|lua, (text, name): (String, Option<String>)| {
            let mut out = Vec::new();
            for (i, line) in text.lines().enumerate() {
                if line.trim().is_empty() {
                    continue;
                }
                let v: serde_json::Value = serde_json::from_str(line)
                    .map_err(|e| parse_err(&name, format!("line {}: {e}", i + 1)))?;
                out.push(v);
            }
            lua.to_value(&out)
        })?,
    )?;

    module.set(
        "to_string_lines",
        lua.create_function(|lua, values: Table| {
            let mut out = String::new();
            for v in values.sequence_values::<Value>() {
                out.push_str(&encode(lua, &v?)?.to_string());
                out.push('\n');
            }
            Ok(out)
        })?,
    )?;

    lua.globals().set("json", module)?;
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
    fn null_round_trips() {
        let out: String = ev(r#"return json.to_string({a = null})"#);
        assert_eq!(out, r#"{"a":null}"#);
        assert!(ev::<bool>(r#"return json.parse('{"a":null}').a == null"#));
    }

    #[test]
    fn empty_array_round_trips_and_marker() {
        assert_eq!(ev::<String>("return json.to_string(json.parse('{\"a\":[]}'))"), r#"{"a":[]}"#);
        assert_eq!(ev::<String>("return json.to_string({a = table.array()})"), r#"{"a":[]}"#);
        assert_eq!(ev::<String>("return json.to_string({a = {}})"), r#"{"a":{}}"#);
    }

    #[test]
    fn pretty_and_indent() {
        assert_eq!(
            ev::<String>("return json.to_string({a = {1}}, {pretty = true})"),
            "{\n  \"a\": [\n    1\n  ]\n}\n"
        );
        assert_eq!(ev::<String>("return json.to_string({a = 1}, {indent = 4})"), "{\n    \"a\": 1\n}\n");
        assert!(err("return json.to_string({}, {prety = true})").contains("bad option `prety`"));
    }

    #[test]
    fn bad_values_raise_with_path() {
        assert!(err("return json.to_string({a = {1, x = 2}})").contains("mixed table"));
        assert!(err("return json.to_string({a = {[1] = 1, [3] = 3}})").contains("sparse"));
        assert!(err("return json.to_string({a = {0/0}})").contains("`a[1]`"));
        assert!(err("return json.to_string({f = print})").contains("function"));
        assert!(err(r#"return json.to_string({s = "\255"})"#).contains("UTF-8"));
    }

    #[test]
    fn parse_error_names_the_source() {
        assert!(err(r#"return json.parse("{", "a.json")"#).contains("in a.json"));
    }

    #[test]
    fn lines() {
        assert_eq!(ev::<i64>("return #json.parse_lines('1\\n\\n{\"a\":2}\\n')"), 2);
        assert_eq!(ev::<String>("return json.to_string_lines({1, {a = 2}})"), "1\n{\"a\":2}\n");
    }
}
