//! `re` module: regular expressions over strings.
//!
//! A thin, pure-computation wrapper around the `regex` crate's **bytes** API, so
//! text need not be valid UTF-8 (a Luau string is a byte sequence). Patterns use
//! that crate's RE2/Perl-style syntax (no backreferences or look-around); the
//! pattern itself must be UTF-8. Numbers are *not* coerced to strings: a
//! non-string `pattern` or `text` raises.
//!
//! Every function has the pattern as its first argument. Patterns are compiled
//! once and cached (a small per-thread table), and `re.compile(pattern)` returns
//! a handle whose methods are the same functions without the pattern argument.
//!
//! - `re.is_match(p, text)` — `true` if `p` matches anywhere.
//! - `re.match(p, text)` — the capture table if `p` matches the *whole* text.
//! - `re.find(p, text)` — `match, start, end` (1-based inclusive, like
//!   `string.find`) of the first match, or `nil`.
//! - `re.find_all(p, text)` — every non-overlapping match, as strings.
//! - `re.count(p, text)` — how many non-overlapping matches.
//! - `re.captures(p, text)` / `re.captures_all(p, text)` — capture table(s):
//!   integer keys `0` (whole match), `1`, `2`, … and named groups under their
//!   name; groups that did not participate are omitted.
//! - `re.replace(p, text, rep)` / `re.replace_all(p, text, rep[, n])` — `rep` is
//!   a template (`$1`, `${name}`, `$$`; an unknown group raises) or a function
//!   taking the capture table and returning the replacement.
//! - `re.split(p, text[, n])` — pieces between matches (at most `n` pieces).
//! - `re.escape(text)` — `text` with every metacharacter escaped.

use std::cell::{OnceCell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use mlua::{Lua, LuaString, MultiValue, ObjectLike, Table, UserData, UserDataMethods, Value};
use regex::bytes::{Captures, Regex};

type Result<T> = mlua::Result<T>;

fn err(msg: String) -> mlua::Error {
    mlua::Error::runtime(msg)
}

/// A compiled pattern. Cheap to clone (everything is shared).
#[derive(Clone)]
struct Re {
    re: Regex,
    pattern: Rc<str>,
    /// `\A(?:pattern)\z`, compiled on first use by `match`.
    full: Rc<OnceCell<Regex>>,
}

thread_local! {
    static CACHE: RefCell<HashMap<String, Re>> = RefCell::new(HashMap::new());
}

/// Most patterns kept before the cache is dropped and refilled.
const CACHE_LIMIT: usize = 256;

fn compile(func: &str, pattern: &str) -> Result<Re> {
    if let Some(hit) = CACHE.with(|c| c.borrow().get(pattern).cloned()) {
        return Ok(hit);
    }
    let re = Regex::new(pattern)
        .map_err(|e| err(format!("re.{func}: invalid pattern {pattern:?}: {e}")))?;
    let re = Re {
        re,
        pattern: pattern.into(),
        full: Rc::default(),
    };
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        if c.len() >= CACHE_LIMIT {
            c.clear();
        }
        c.insert(pattern.to_string(), re.clone());
    });
    Ok(re)
}

/// A string argument, exactly: no number-to-string coercion.
fn str_arg(func: &str, what: &str, v: Option<&Value>) -> Result<LuaString> {
    match v {
        Some(Value::String(s)) => Ok(s.clone()),
        other => Err(err(format!(
            "re.{func}: {what} must be a string, got {}",
            other.map_or("nil", |v| v.type_name())
        ))),
    }
}

fn pattern_arg(func: &str, v: Option<&Value>) -> Result<String> {
    let s = str_arg(func, "pattern", v)?;
    s.to_str()
        .map(|s| s.to_string())
        .map_err(|_| err(format!("re.{func}: pattern must be valid UTF-8")))
}

/// Build a capture table for `caps`: integer keys `0` (whole match), `1`, `2`,
/// …, and named groups also under their name. Non-participating groups are
/// omitted. The same shape `fs.finder():grep()` reports in `captures`.
fn captures_to_table(lua: &Lua, re: &Regex, caps: &Captures) -> Result<Table> {
    let map = lua.create_table()?;
    for (i, group) in caps.iter().enumerate() {
        if let Some(m) = group {
            map.set(i as i64, lua.create_string(m.as_bytes())?)?;
        }
    }
    for name in re.capture_names().flatten() {
        if let Some(m) = caps.name(name) {
            map.set(name, lua.create_string(m.as_bytes())?)?;
        }
    }
    Ok(map)
}

/// Check every `$ref` in a replacement template names a real group, so a typo
/// raises instead of silently expanding to `""`.
fn check_template(func: &str, re: &Regex, rep: &[u8]) -> Result<()> {
    let names: Vec<&str> = re.capture_names().flatten().collect();
    let mut i = 0;
    while i < rep.len() {
        if rep[i] != b'$' {
            i += 1;
            continue;
        }
        i += 1;
        match rep.get(i) {
            Some(b'$') => i += 1,
            Some(b'{') => {
                let Some(len) = rep[i + 1..].iter().position(|&b| b == b'}') else {
                    return Err(err(format!("re.{func}: unterminated ${{ in replacement")));
                };
                check_ref(func, re, &names, &rep[i + 1..i + 1 + len])?;
                i += len + 2;
            }
            _ => {
                let len = rep[i..]
                    .iter()
                    .take_while(|b| b.is_ascii_alphanumeric() || **b == b'_')
                    .count();
                // A bare `$` followed by anything else is a literal `$`.
                if len > 0 {
                    check_ref(func, re, &names, &rep[i..i + len])?;
                }
                i += len;
            }
        }
    }
    Ok(())
}

fn check_ref(func: &str, re: &Regex, names: &[&str], name: &[u8]) -> Result<()> {
    let name = String::from_utf8_lossy(name);
    let ok = match name.parse::<usize>() {
        Ok(n) => n < re.captures_len(),
        Err(_) => names.contains(&&*name),
    };
    if ok {
        Ok(())
    } else {
        Err(err(format!("re.{func}: unknown group {name:?} in replacement")))
    }
}

impl Re {
    fn replace(
        &self,
        lua: &Lua,
        func: &str,
        text: &LuaString,
        rep: &Value,
        limit: usize,
    ) -> Result<LuaString> {
        let text = text.as_bytes();
        let template = match rep {
            Value::String(s) => {
                let b = s.as_bytes().to_vec();
                check_template(func, &self.re, &b)?;
                Some(b)
            }
            Value::Function(_) => None,
            other => {
                return Err(err(format!(
                    "re.{func}: replacement must be a string or function, got {}",
                    other.type_name()
                )));
            }
        };
        let mut out: Vec<u8> = Vec::with_capacity(text.len());
        let mut last = 0;
        for caps in self.re.captures_iter(&text).take(limit) {
            let m = caps.get(0).expect("group 0");
            out.extend_from_slice(&text[last..m.start()]);
            match (&template, rep) {
                (Some(t), _) => caps.expand(t, &mut out),
                (None, Value::Function(f)) => {
                    let r: Value = f.call(captures_to_table(lua, &self.re, &caps)?)?;
                    match r {
                        Value::String(s) => out.extend_from_slice(&s.as_bytes()),
                        other => {
                            return Err(err(format!(
                                "re.{func}: replacement function must return a string, got {}",
                                other.type_name()
                            )));
                        }
                    }
                }
                _ => unreachable!(),
            }
            last = m.end();
        }
        out.extend_from_slice(&text[last..]);
        lua.create_string(out)
    }
}

/// A limit argument: absent means unlimited; otherwise a positive integer.
fn limit_arg(func: &str, v: Option<&Value>) -> Result<usize> {
    match v {
        None | Some(Value::Nil) => Ok(usize::MAX),
        Some(Value::Integer(n)) if *n >= 1 => Ok(*n as usize),
        Some(Value::Number(n)) if *n >= 1.0 && n.fract() == 0.0 => Ok(*n as usize),
        Some(_) => Err(err(format!("re.{func}: n must be a positive integer"))),
    }
}

/// The methods, named as the module functions are. Each takes its arguments
/// after the pattern as a `MultiValue`.
impl UserData for Re {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("is_match", |_, re, args: MultiValue| {
            let text = str_arg("is_match", "text", args.front())?;
            Ok(re.re.is_match(&text.as_bytes()))
        });

        methods.add_method("match", |lua, re, args: MultiValue| {
            let text = str_arg("match", "text", args.front())?;
            let full = re.full.get_or_init(|| {
                Regex::new(&format!(r"\A(?:{})\z", re.pattern)).expect("wrapped pattern")
            });
            match full.captures(&text.as_bytes()) {
                Some(caps) => Ok(Some(captures_to_table(lua, full, &caps)?)),
                None => Ok(None),
            }
        });

        methods.add_method("find", |lua, re, args: MultiValue| {
            let text = str_arg("find", "text", args.front())?;
            let bytes = text.as_bytes();
            Ok(match re.re.find(&bytes) {
                Some(m) => MultiValue::from_iter([
                    Value::String(lua.create_string(m.as_bytes())?),
                    Value::Integer(m.start() as i64 + 1),
                    Value::Integer(m.end() as i64),
                ]),
                None => MultiValue::new(),
            })
        });

        methods.add_method("find_all", |lua, re, args: MultiValue| {
            let text = str_arg("find_all", "text", args.front())?;
            re.re
                .find_iter(&text.as_bytes())
                .map(|m| lua.create_string(m.as_bytes()))
                .collect::<Result<Vec<_>>>()
        });

        methods.add_method("count", |_, re, args: MultiValue| {
            let text = str_arg("count", "text", args.front())?;
            Ok(re.re.find_iter(&text.as_bytes()).count())
        });

        methods.add_method("captures", |lua, re, args: MultiValue| {
            let text = str_arg("captures", "text", args.front())?;
            match re.re.captures(&text.as_bytes()) {
                Some(caps) => Ok(Some(captures_to_table(lua, &re.re, &caps)?)),
                None => Ok(None),
            }
        });

        methods.add_method("captures_all", |lua, re, args: MultiValue| {
            let text = str_arg("captures_all", "text", args.front())?;
            re.re
                .captures_iter(&text.as_bytes())
                .map(|caps| captures_to_table(lua, &re.re, &caps))
                .collect::<Result<Vec<_>>>()
        });

        methods.add_method("replace", |lua, re, args: MultiValue| {
            let text = str_arg("replace", "text", args.front())?;
            let rep = args.get(1).cloned().unwrap_or(Value::Nil);
            re.replace(lua, "replace", &text, &rep, 1)
        });

        methods.add_method("replace_all", |lua, re, args: MultiValue| {
            let text = str_arg("replace_all", "text", args.front())?;
            let rep = args.get(1).cloned().unwrap_or(Value::Nil);
            let limit = limit_arg("replace_all", args.get(2))?;
            re.replace(lua, "replace_all", &text, &rep, limit)
        });

        methods.add_method("split", |lua, re, args: MultiValue| {
            let text = str_arg("split", "text", args.front())?;
            let limit = limit_arg("split", args.get(1))?;
            let bytes = text.as_bytes();
            let pieces: Vec<&[u8]> = if limit == usize::MAX {
                re.re.split(&bytes).collect()
            } else {
                re.re.splitn(&bytes, limit).collect()
            };
            pieces
                .into_iter()
                .map(|p| lua.create_string(p))
                .collect::<Result<Vec<_>>>()
        });

        methods.add_meta_method("__tostring", |_, re, ()| Ok(format!("re({:?})", &*re.pattern)));
    }
}

/// Register the `re` module on `lua`.
pub fn register(lua: &Lua) -> Result<()> {
    let module = lua.create_table()?;

    // re.<name>(pattern, ...) -> re.compile(pattern):<name>(...)
    for name in [
        "is_match",
        "match",
        "find",
        "find_all",
        "count",
        "captures",
        "captures_all",
        "replace",
        "replace_all",
        "split",
    ] {
        module.set(
            name,
            lua.create_function(move |lua, mut args: MultiValue| {
                let pattern = pattern_arg(name, args.front())?;
                args.pop_front();
                let re = compile(name, &pattern)?;
                lua.create_userdata(re)?.call_method::<MultiValue>(name, args)
            })?,
        )?;
    }

    // re.compile(pattern) -> Re
    module.set(
        "compile",
        lua.create_function(|_, pattern: Value| compile("compile", &pattern_arg("compile", Some(&pattern))?))?,
    )?;

    // re.escape(text) -> string
    module.set(
        "escape",
        lua.create_function(|lua, text: Value| {
            let text = str_arg("escape", "text", Some(&text))?;
            let s = text
                .to_str()
                .map_err(|_| err("re.escape: text must be valid UTF-8".into()))?;
            lua.create_string(regex::escape(&s))
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

    fn eval_err(src: &str) -> String {
        lua().load(src).eval::<Value>().expect_err("must raise").to_string()
    }

    #[test]
    fn is_match() {
        assert!(eval_bool(r#"return re.is_match([[\d+]], "abc123")"#));
        assert!(!eval_bool(r#"return re.is_match([[\d+]], "abc")"#));
    }

    #[test]
    fn find_returns_positions() {
        assert_eq!(eval_string(r#"return re.find([[\d+]], "a12b34")"#), "12");
        assert_eq!(
            eval_string(r#"local m, s, e = re.find([[\d+]], "a12b34") return m .. s .. e"#),
            "1223"
        );
        assert_eq!(
            lua().load(r#"return re.find("x", "abc")"#).eval::<Value>().unwrap(),
            Value::Nil
        );
    }

    #[test]
    fn find_all_and_count() {
        assert_eq!(eval_strings(r#"return re.find_all([[\d+]], "a12b34")"#), ["12", "34"]);
        assert!(eval_strings(r#"return re.find_all([[\d+]], "abc")"#).is_empty());
        assert_eq!(lua().load(r#"return re.count([[\d]], "a1b22")"#).eval::<i64>().unwrap(), 3);
    }

    #[test]
    fn captures_use_integer_keys() {
        let lua = lua();
        let m = lua
            .load(r#"return re.captures("(?<y>\\d{4})-(\\d{2})", "2026-06")"#)
            .eval::<Table>()
            .expect("eval table");
        assert_eq!(m.get::<String>(0).unwrap(), "2026-06");
        assert_eq!(m.get::<String>(1).unwrap(), "2026");
        assert_eq!(m.get::<String>(2).unwrap(), "06");
        assert_eq!(m.get::<String>("y").unwrap(), "2026");
        assert_eq!(
            lua.load(r#"local y, mo = unpack(re.captures("(\\d+)-(\\d+)", "1-2")) return y .. mo"#)
                .eval::<String>()
                .unwrap(),
            "12"
        );
    }

    #[test]
    fn captures_no_match_is_nil() {
        assert_eq!(
            lua().load(r#"return re.captures("\\d+", "abc")"#).eval::<Value>().unwrap(),
            Value::Nil
        );
    }

    #[test]
    fn captures_all() {
        let arr = lua()
            .load(r#"return re.captures_all("(\\d)(\\w)", "1a2b")"#)
            .eval::<Vec<Table>>()
            .expect("eval array");
        assert_eq!(arr.len(), 2);
    }

    #[test]
    fn match_is_anchored_over_the_whole_text() {
        assert!(eval_bool(r#"return re.match("a|ab", "ab") ~= nil"#));
        assert!(eval_bool(r#"return re.match("b", "ab") == nil"#));
        assert_eq!(eval_string(r#"return re.match("(a)b", "ab")[1]"#), "a");
    }

    #[test]
    fn replace_first_and_all_with_limit() {
        assert_eq!(eval_string(r#"return re.replace("\\d", "a1b2", "X")"#), "aXb2");
        assert_eq!(eval_string(r#"return re.replace_all("\\d", "a1b2", "X")"#), "aXbX");
        assert_eq!(eval_string(r#"return re.replace_all("\\d", "1234", "X", 2)"#), "XX34");
    }

    #[test]
    fn replace_with_group_reference_and_function() {
        assert_eq!(
            eval_string(r#"return re.replace_all("(\\w)(\\d)", "a1 b2", "$2$1")"#),
            "1a 2b"
        );
        assert_eq!(
            eval_string(r#"return re.replace_all("(?<n>\\d)", "a1b2", "${n}${n}")"#),
            "a11b22"
        );
        assert_eq!(
            eval_string(r#"return re.replace_all("\\d", "a1b2", function(m) return "<" .. m[0] .. ">" end)"#),
            "a<1>b<2>"
        );
        assert!(eval_err(r#"return re.replace("\\d", "a1", function() return 1 end)"#).contains("must return a string"));
    }

    #[test]
    fn unknown_group_in_replacement_raises() {
        for rep in ["$2", "${nope}", "$nope"] {
            let e = eval_err(&format!(r#"return re.replace("(a)", "a", "{rep}")"#));
            assert!(e.contains("unknown group"), "{rep}: {e}");
        }
        assert_eq!(eval_string(r#"return re.replace("a", "a", "$$ and $ ")"#), "$ and $ ");
    }

    #[test]
    fn split_edge_cases() {
        assert_eq!(eval_strings(r#"return re.split(",\\s*", "a, b,c")"#), ["a", "b", "c"]);
        assert_eq!(eval_strings(r#"return re.split(",", "")"#), [""]);
        assert_eq!(eval_strings(r#"return re.split(",", ",a,")"#), ["", "a", ""]);
        assert_eq!(eval_strings(r#"return re.split(",", "a,b,c", 2)"#), ["a", "b,c"]);
    }

    #[test]
    fn escape_and_compile() {
        assert_eq!(eval_string(r#"return re.escape("a.b*c")"#), r"a\.b\*c");
        assert!(eval_bool(r#"local r = re.compile("^a+$") return r:is_match("aaa") and not r:is_match("b")"#));
        assert_eq!(eval_string(r#"return re.compile("(\\d)"):replace_all("a1b2", "[$1]")"#), "a[1]b[2]");
    }

    #[test]
    fn non_utf8_text_and_no_number_coercion() {
        assert_eq!(
            lua().load(r#"return re.count("a", "a\xff\xfea")"#).eval::<i64>().unwrap(),
            2
        );
        assert!(eval_err(r#"return re.find("1", 123)"#).contains("text must be a string"));
        assert!(eval_err(r#"return re.find(1, "1")"#).contains("pattern must be a string"));
    }

    #[test]
    fn invalid_pattern_names_function_and_pattern() {
        let e = eval_err(r#"return re.is_match("(", "x")"#);
        assert!(e.contains("re.is_match") && e.contains("\"(\""), "{e}");
    }
}
