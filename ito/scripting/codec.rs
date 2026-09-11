//! Shared plumbing of the `json`/`toml`/`yaml` codecs (not a Luau module).
//!
//! - [`to_data`] walks a Luau value into a [`yamlito::Yaml`] (the data model all
//!   three encoders start from), with precise errors: a key path, and a reason
//!   for every value that has no data form (mixed/sparse tables, functions,
//!   invalid UTF-8, cycles). Map keys come out sorted, so output is stable.
//! - The **array marker** is mlua's `array_metatable`: a table carrying it is
//!   an array even when empty. Parsers attach it to every array they produce;
//!   scripts get it from `table.array(t?)` ([`register_array`]). Without the
//!   marker an empty table is a map (`{}`).
//! - [`Options`] parses the `{pretty=, indent=}` argument of `to_string`.

use mlua::{Lua, LuaSerdeExt, Table, Value};
use yamlito::Yaml;

const MAX_DEPTH: usize = 128;
/// Integers a double holds exactly.
const EXACT: f64 = 9007199254740992.0;

/// Install `table.array(t?)`: mark `t` (or a fresh table) as an array.
/// Must run before `sandbox(true)`.
pub fn register_array(lua: &Lua) -> mlua::Result<()> {
    let table: Table = lua.globals().get("table")?;
    table.set(
        "array",
        lua.create_function(|lua, t: Option<Table>| {
            let t = match t {
                Some(t) => t,
                None => lua.create_table()?,
            };
            t.set_metatable(Some(lua.array_metatable()))?;
            Ok(t)
        })?,
    )
}

/// Convert a Luau value to the data model. `what` prefixes errors
/// (`"json serialize error"`).
pub fn to_data(lua: &Lua, value: &Value, what: &str) -> mlua::Result<Yaml> {
    let mut path = String::new();
    walk(lua, value, &mut path, 0).map_err(|(at, msg)| {
        let at = if at.is_empty() { String::new() } else { format!(" at `{at}`") };
        mlua::Error::runtime(format!("{what}{at}: {msg}"))
    })
}

/// `(path, message)`
type Fail = (String, String);

fn fail<T>(path: &str, msg: impl Into<String>) -> Result<T, Fail> {
    Err((path.to_string(), msg.into()))
}

fn walk(lua: &Lua, v: &Value, path: &mut String, depth: usize) -> Result<Yaml, Fail> {
    if depth > MAX_DEPTH {
        return fail(path, "nested too deeply (cyclic table?)");
    }
    match v {
        Value::Nil => Ok(Yaml::Null),
        Value::LightUserData(_) if *v == lua.null() => Ok(Yaml::Null),
        Value::Boolean(b) => Ok(Yaml::Bool(*b)),
        Value::Integer(i) => Ok(Yaml::Int(*i as i64)),
        Value::Number(n) => Ok(if n.fract() == 0.0 && n.abs() <= EXACT {
            Yaml::Int(*n as i64)
        } else {
            Yaml::Float(*n)
        }),
        Value::String(s) => match s.to_str() {
            Ok(s) => Ok(Yaml::Str(s.to_string())),
            Err(_) => fail(path, "string is not valid UTF-8"),
        },
        Value::Table(t) => table(lua, t, path, depth),
        other => fail(path, format!("a {} cannot be represented", other.type_name())),
    }
}

fn is_int_key(k: &Value) -> bool {
    match k {
        Value::Integer(_) => true,
        Value::Number(n) => n.fract() == 0.0,
        _ => false,
    }
}

fn table(lua: &Lua, t: &Table, path: &mut String, depth: usize) -> Result<Yaml, Fail> {
    let marked = t.metatable().is_some_and(|m| m == lua.array_metatable());
    let len = t.raw_len();
    let (mut keyed, mut ints, mut n) = (Vec::new(), 0usize, 0usize);
    for pair in t.pairs::<Value, Value>() {
        let Ok((k, v)) = pair else {
            return fail(path, "unreadable table");
        };
        n += 1;
        if let Value::String(s) = &k {
            let Ok(s) = s.to_str() else {
                return fail(path, "key is not valid UTF-8");
            };
            keyed.push((s.to_string(), v));
        } else if is_int_key(&k) {
            ints += 1;
        } else {
            return fail(path, format!("a {} cannot be a key", k.type_name()));
        }
    }
    let seq = if marked {
        if !keyed.is_empty() || ints != len {
            return fail(path, "array-marked table has keys that are not 1..n");
        }
        true
    } else if n == 0 {
        false
    } else if keyed.is_empty() {
        if ints != len {
            return fail(path, "table has integer keys that are not a sequence 1..n (sparse?)");
        }
        true
    } else if ints > 0 {
        return fail(
            path,
            format!("mixed table ({ints} array items and {} string keys)", keyed.len()),
        );
    } else {
        false
    };
    let base = path.len();
    Ok(if seq {
        let mut items = Vec::with_capacity(len);
        for i in 1..=len {
            path.push_str(&format!("[{i}]"));
            let v: Value = t.raw_get(i).map_err(|e| (path.clone(), e.to_string()))?;
            items.push(walk(lua, &v, path, depth + 1)?);
            path.truncate(base);
        }
        Yaml::Seq(items)
    } else {
        keyed.sort_by(|a, b| a.0.cmp(&b.0));
        let mut entries = Vec::with_capacity(keyed.len());
        for (k, v) in keyed {
            if base > 0 {
                path.push('.');
            }
            path.push_str(&k);
            let d = walk(lua, &v, path, depth + 1)?;
            entries.push((k, d));
            path.truncate(base);
        }
        Yaml::Map(entries)
    })
}

/// The `{pretty=, indent=}` argument of a `to_string`.
pub struct Options {
    /// `Some(width)` when pretty output was asked for.
    pub indent: Option<usize>,
}

impl Options {
    /// `indent` is accepted only when `allow_indent`; unknown keys raise.
    pub fn parse(opts: Option<Table>, who: &str, allow_indent: bool) -> mlua::Result<Self> {
        let (mut pretty, mut indent) = (false, None);
        if let Some(t) = opts {
            for pair in t.pairs::<String, Value>() {
                let (k, v) = pair?;
                match (k.as_str(), v) {
                    ("pretty", Value::Boolean(b)) => pretty = b,
                    ("indent", Value::Integer(n)) if allow_indent && n >= 0 => {
                        indent = Some(n as usize)
                    }
                    ("indent", Value::Number(n)) if allow_indent && n >= 0.0 && n.fract() == 0.0 => {
                        indent = Some(n as usize)
                    }
                    (k, _) => {
                        let want = if allow_indent {
                            "`pretty`: boolean, `indent`: integer >= 0"
                        } else {
                            "`pretty`: boolean"
                        };
                        return Err(mlua::Error::runtime(format!(
                            "{who}: bad option `{k}` (expected {want})"
                        )));
                    }
                }
            }
        }
        let on = pretty || indent.is_some();
        Ok(Options { indent: on.then(|| indent.unwrap_or(2)) })
    }
}
