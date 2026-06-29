//! `re` module: regular expressions over strings.
//!
//! A thin, pure-computation wrapper around the `regex` crate. Patterns
//! use that crate's syntax (a superset of POSIX, no backreferences or
//! look-around). Every function takes the pattern as its first argument
//! and compiles it on each call; an invalid pattern surfaces as a script
//! runtime error.
//!
//! - `re.is_match(pattern, text)` — `true` if `pattern` matches anywhere
//!   in `text`.
//! - `re.find(pattern, text)` — the first match as a string, or `nil` if
//!   none.
//! - `re.find_all(pattern, text)` — every non-overlapping match, as an
//!   array of strings (empty if none).
//! - `re.captures(pattern, text)` — the first match's capture groups as
//!   a table, or `nil` if none. Key `"0"` is the whole match; numbered keys
//!   (`"1"`, `"2"`, …) are positional groups; named groups also appear
//!   under their name. Groups that did not participate are omitted.
//! - `re.captures_all(pattern, text)` — an array of such tables, one per
//!   match.
//! - `re.replace(pattern, text, rep)` — replace the first match with
//!   `rep` (supports `$1` / `${name}` references), returning the new
//!   string.
//! - `re.replace_all(pattern, text, rep)` — replace every match.
//! - `re.split(pattern, text)` — split `text` on `pattern`, returning an
//!   array of the pieces between matches.

use mlua::{Lua, Table};
use regex::Regex;

/// Compile `pattern`, mapping a syntax error to a script runtime error.
fn compile(pattern: &str) -> mlua::Result<Regex> {
    Regex::new(pattern).map_err(|e| mlua::Error::runtime(format!("re: invalid pattern: {e}")))
}

/// Build a capture table for `caps`: `"0"` is the whole match, numbered
/// keys are positional groups, named groups also appear under their name.
/// Non-participating groups are omitted.
fn captures_to_table(lua: &Lua, re: &Regex, caps: &regex::Captures) -> mlua::Result<Table> {
    let map = lua.create_table()?;
    for (i, group) in caps.iter().enumerate() {
        if let Some(m) = group {
            map.set(i.to_string(), m.as_str())?;
        }
    }
    for name in re.capture_names().flatten() {
        if let Some(m) = caps.name(name) {
            map.set(name, m.as_str())?;
        }
    }
    Ok(map)
}

/// Register the `re` module on `lua`.
pub fn register(lua: &Lua) -> mlua::Result<()> {
    let module = lua.create_table()?;

    module.set(
        "is_match",
        lua.create_function(|_, (pattern, text): (String, String)| {
            Ok(compile(&pattern)?.is_match(&text))
        })?,
    )?;

    module.set(
        "find",
        lua.create_function(|_, (pattern, text): (String, String)| {
            Ok(compile(&pattern)?
                .find(&text)
                .map(|m| m.as_str().to_string()))
        })?,
    )?;

    module.set(
        "find_all",
        lua.create_function(|_, (pattern, text): (String, String)| {
            Ok(compile(&pattern)?
                .find_iter(&text)
                .map(|m| m.as_str().to_string())
                .collect::<Vec<String>>())
        })?,
    )?;

    module.set(
        "captures",
        lua.create_function(|lua, (pattern, text): (String, String)| {
            let re = compile(&pattern)?;
            match re.captures(&text) {
                Some(caps) => Ok(Some(captures_to_table(lua, &re, &caps)?)),
                None => Ok(None),
            }
        })?,
    )?;

    module.set(
        "captures_all",
        lua.create_function(|lua, (pattern, text): (String, String)| {
            let re = compile(&pattern)?;
            let mut out = Vec::new();
            for caps in re.captures_iter(&text) {
                out.push(captures_to_table(lua, &re, &caps)?);
            }
            Ok(out)
        })?,
    )?;

    module.set(
        "replace",
        lua.create_function(|_, (pattern, text, rep): (String, String, String)| {
            Ok(compile(&pattern)?.replace(&text, rep.as_str()).into_owned())
        })?,
    )?;

    module.set(
        "replace_all",
        lua.create_function(|_, (pattern, text, rep): (String, String, String)| {
            Ok(compile(&pattern)?
                .replace_all(&text, rep.as_str())
                .into_owned())
        })?,
    )?;

    module.set(
        "split",
        lua.create_function(|_, (pattern, text): (String, String)| {
            Ok(compile(&pattern)?
                .split(&text)
                .map(|s| s.to_string())
                .collect::<Vec<String>>())
        })?,
    )?;

    lua.globals().set("re", module)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mlua::{Table, Value};

    fn lua() -> Lua {
        let lua = Lua::new();
        register(&lua).expect("register");
        lua
    }

    fn eval_bool(src: &str) -> bool {
        lua().load(src).eval::<bool>().expect("eval bool")
    }

    fn eval_string(src: &str) -> String {
        lua().load(src).eval::<String>().expect("eval string")
    }

    fn eval_strings(src: &str) -> Vec<String> {
        lua().load(src).eval::<Vec<String>>().expect("eval array")
    }

    #[test]
    fn is_match() {
        assert!(eval_bool(r#"return re.is_match("\\d+", "abc123")"#));
        assert!(!eval_bool(r#"return re.is_match("\\d+", "abc")"#));
    }

    #[test]
    fn find() {
        assert_eq!(eval_string(r#"return re.find("\\d+", "a12b34")"#), "12");
        assert_eq!(
            lua()
                .load(r#"return re.find("\\d+", "abc")"#)
                .eval::<Value>()
                .expect("eval"),
            Value::Nil
        );
    }

    #[test]
    fn find_all() {
        assert_eq!(
            eval_strings(r#"return re.find_all("\\d+", "a12b34")"#),
            ["12", "34"]
        );
        assert!(eval_strings(r#"return re.find_all("\\d+", "abc")"#).is_empty());
    }

    #[test]
    fn captures() {
        let lua = lua();
        let m = lua
            .load(r#"return re.captures("(?<y>\\d{4})-(\\d{2})", "2026-06")"#)
            .eval::<Table>()
            .expect("eval table");
        assert_eq!(m.get::<String>("0").unwrap(), "2026-06");
        assert_eq!(m.get::<String>("1").unwrap(), "2026");
        assert_eq!(m.get::<String>("2").unwrap(), "06");
        assert_eq!(m.get::<String>("y").unwrap(), "2026");
    }

    #[test]
    fn captures_no_match_is_nil() {
        assert_eq!(
            lua()
                .load(r#"return re.captures("\\d+", "abc")"#)
                .eval::<Value>()
                .expect("eval"),
            Value::Nil
        );
    }

    #[test]
    fn captures_all() {
        let lua = lua();
        let arr = lua
            .load(r#"return re.captures_all("(\\d)(\\w)", "1a2b")"#)
            .eval::<Vec<Table>>()
            .expect("eval array");
        assert_eq!(arr.len(), 2);
    }

    #[test]
    fn replace_first_and_all() {
        assert_eq!(
            eval_string(r#"return re.replace("\\d", "a1b2", "X")"#),
            "aXb2"
        );
        assert_eq!(
            eval_string(r#"return re.replace_all("\\d", "a1b2", "X")"#),
            "aXbX"
        );
    }

    #[test]
    fn replace_with_group_reference() {
        assert_eq!(
            eval_string(r#"return re.replace_all("(\\w)(\\d)", "a1 b2", "$2$1")"#),
            "1a 2b"
        );
    }

    #[test]
    fn split() {
        assert_eq!(
            eval_strings(r#"return re.split(",\\s*", "a, b,c")"#),
            ["a", "b", "c"]
        );
    }

    #[test]
    fn invalid_pattern_errors() {
        assert!(
            lua()
                .load(r#"return re.is_match("(", "x")"#)
                .eval::<bool>()
                .is_err()
        );
    }
}
