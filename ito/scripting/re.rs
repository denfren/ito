//! `re` module: regular expressions over strings.
//!
//! A thin, pure-computation wrapper around the `regex` crate. Patterns
//! use that crate's syntax (a superset of POSIX, no backreferences or
//! look-around). Every function takes the pattern as its first argument
//! and compiles it on each call; an invalid pattern surfaces as a script
//! runtime error.
//!
//! - `re::is_match(pattern, text)` — `true` if `pattern` matches anywhere
//!   in `text`.
//! - `re::find(pattern, text)` — the first match as a string, or `()` if
//!   none.
//! - `re::find_all(pattern, text)` — every non-overlapping match, as an
//!   array of strings (empty if none).
//! - `re::captures(pattern, text)` — the first match's capture groups as
//!   a map, or `()` if none. Key `"0"` is the whole match; numbered keys
//!   (`"1"`, `"2"`, …) are positional groups; named groups also appear
//!   under their name. Groups that did not participate are omitted.
//! - `re::captures_all(pattern, text)` — an array of such maps, one per
//!   match.
//! - `re::replace(pattern, text, rep)` — replace the first match with
//!   `rep` (supports `$1` / `${name}` references), returning the new
//!   string.
//! - `re::replace_all(pattern, text, rep)` — replace every match.
//! - `re::split(pattern, text)` — split `text` on `pattern`, returning an
//!   array of the pieces between matches.

use regex::Regex;
use rhai::{Array, Dynamic, Engine, EvalAltResult, ImmutableString, Map, Module};

type Result<T> = std::result::Result<T, Box<EvalAltResult>>;

/// Compile `pattern`, mapping a syntax error to a script runtime error.
fn compile(pattern: &str) -> Result<Regex> {
    Regex::new(pattern)
        .map_err(|e| -> Box<EvalAltResult> { format!("re: invalid pattern: {e}").into() })
}

/// Build a capture map for `caps`: `"0"` is the whole match, numbered
/// keys are positional groups, named groups also appear under their name.
/// Non-participating groups are omitted.
fn captures_to_map(re: &Regex, caps: &regex::Captures) -> Map {
    let mut map = Map::new();
    for (i, group) in caps.iter().enumerate() {
        if let Some(m) = group {
            map.insert(i.to_string().into(), m.as_str().into());
        }
    }
    for name in re.capture_names().flatten() {
        if let Some(m) = caps.name(name) {
            map.insert(name.into(), m.as_str().into());
        }
    }
    map
}

/// Register the `re` module on `engine`.
pub fn register(engine: &mut Engine) {
    let mut module = Module::new();

    module.set_native_fn(
        "is_match",
        |pattern: ImmutableString, text: ImmutableString| -> Result<bool> {
            Ok(compile(&pattern)?.is_match(&text))
        },
    );

    module.set_native_fn(
        "find",
        |pattern: ImmutableString, text: ImmutableString| -> Result<Dynamic> {
            Ok(match compile(&pattern)?.find(&text) {
                Some(m) => m.as_str().into(),
                None => Dynamic::UNIT,
            })
        },
    );

    module.set_native_fn(
        "find_all",
        |pattern: ImmutableString, text: ImmutableString| -> Result<Array> {
            Ok(compile(&pattern)?
                .find_iter(&text)
                .map(|m| m.as_str().into())
                .collect())
        },
    );

    module.set_native_fn(
        "captures",
        |pattern: ImmutableString, text: ImmutableString| -> Result<Dynamic> {
            let re = compile(&pattern)?;
            Ok(match re.captures(&text) {
                Some(caps) => captures_to_map(&re, &caps).into(),
                None => Dynamic::UNIT,
            })
        },
    );

    module.set_native_fn(
        "captures_all",
        |pattern: ImmutableString, text: ImmutableString| -> Result<Array> {
            let re = compile(&pattern)?;
            Ok(re
                .captures_iter(&text)
                .map(|caps| captures_to_map(&re, &caps).into())
                .collect())
        },
    );

    module.set_native_fn(
        "replace",
        |pattern: ImmutableString,
         text: ImmutableString,
         rep: ImmutableString|
         -> Result<ImmutableString> {
            Ok(compile(&pattern)?
                .replace(&text, rep.as_str())
                .into_owned()
                .into())
        },
    );

    module.set_native_fn(
        "replace_all",
        |pattern: ImmutableString,
         text: ImmutableString,
         rep: ImmutableString|
         -> Result<ImmutableString> {
            Ok(compile(&pattern)?
                .replace_all(&text, rep.as_str())
                .into_owned()
                .into())
        },
    );

    module.set_native_fn(
        "split",
        |pattern: ImmutableString, text: ImmutableString| -> Result<Array> {
            Ok(compile(&pattern)?.split(&text).map(|s| s.into()).collect())
        },
    );

    engine.register_static_module("re", module.into());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> Engine {
        let mut e = Engine::new();
        register(&mut e);
        e
    }

    fn eval_bool(src: &str) -> bool {
        engine().eval::<bool>(src).expect("eval bool")
    }

    fn eval_string(src: &str) -> String {
        engine().eval::<String>(src).expect("eval string")
    }

    fn eval_strings(src: &str) -> Vec<String> {
        engine()
            .eval::<Array>(src)
            .expect("eval array")
            .into_iter()
            .map(|d| d.into_string().expect("string elem"))
            .collect()
    }

    #[test]
    fn is_match() {
        assert!(eval_bool(r#"re::is_match("\\d+", "abc123")"#));
        assert!(!eval_bool(r#"re::is_match("\\d+", "abc")"#));
    }

    #[test]
    fn find() {
        assert_eq!(eval_string(r#"re::find("\\d+", "a12b34")"#), "12");
        assert!(
            engine()
                .eval::<Dynamic>(r#"re::find("\\d+", "abc")"#)
                .expect("eval")
                .is_unit()
        );
    }

    #[test]
    fn find_all() {
        assert_eq!(
            eval_strings(r#"re::find_all("\\d+", "a12b34")"#),
            ["12", "34"]
        );
        assert!(eval_strings(r#"re::find_all("\\d+", "abc")"#).is_empty());
    }

    #[test]
    fn captures() {
        let m = engine()
            .eval::<Map>(r#"re::captures("(?<y>\\d{4})-(\\d{2})", "2026-06")"#)
            .expect("eval map");
        assert_eq!(m["0"].clone().into_string().unwrap(), "2026-06");
        assert_eq!(m["1"].clone().into_string().unwrap(), "2026");
        assert_eq!(m["2"].clone().into_string().unwrap(), "06");
        assert_eq!(m["y"].clone().into_string().unwrap(), "2026");
    }

    #[test]
    fn captures_no_match_is_unit() {
        assert!(
            engine()
                .eval::<Dynamic>(r#"re::captures("\\d+", "abc")"#)
                .expect("eval")
                .is_unit()
        );
    }

    #[test]
    fn captures_all() {
        let arr = engine()
            .eval::<Array>(r#"re::captures_all("(\\d)(\\w)", "1a2b")"#)
            .expect("eval array");
        assert_eq!(arr.len(), 2);
    }

    #[test]
    fn replace_first_and_all() {
        assert_eq!(eval_string(r#"re::replace("\\d", "a1b2", "X")"#), "aXb2");
        assert_eq!(
            eval_string(r#"re::replace_all("\\d", "a1b2", "X")"#),
            "aXbX"
        );
    }

    #[test]
    fn replace_with_group_reference() {
        assert_eq!(
            eval_string(r#"re::replace_all("(\\w)(\\d)", "a1 b2", "$2$1")"#),
            "1a 2b"
        );
    }

    #[test]
    fn split() {
        assert_eq!(
            eval_strings(r#"re::split(",\\s*", "a, b,c")"#),
            ["a", "b", "c"]
        );
    }

    #[test]
    fn invalid_pattern_errors() {
        assert!(engine().eval::<bool>(r#"re::is_match("(", "x")"#).is_err());
    }
}
