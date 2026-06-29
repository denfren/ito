//! `path` module: pure lexical path manipulation.
//!
//! These are total string functions — no disk access, no sandbox
//! resolution. They operate on script-visible paths, so their results
//! feed straight back into the disk-touching `fs.` functions
//! (`fs.read(path.join(dir, name))`). Mirrors the `std::fs` /
//! `std::path` split: `fs.` is I/O, `path.` is string math.

use std::path::{Path, PathBuf};

use mlua::{Lua, Table, Value};

/// One element of a parsed `capture` pattern.
enum Elem {
    /// A literal segment that must match exactly.
    Literal(String),
    /// `{name}` — captures exactly one segment.
    One(String),
    /// `{name?}` — captures zero or one segment.
    OptOne(String),
    /// `{name*}` — captures one-or-more segments (joined with `/`).
    Many(String),
    /// `{name*?}` — captures zero-or-more segments (joined with `/`).
    OptMany(String),
}

impl Elem {
    /// `true` for the variable-width captures (`*` / `*?`), which need a
    /// following literal (or end-of-pattern) to bound their greed.
    fn is_variable_width(&self) -> bool {
        matches!(self, Elem::Many(_) | Elem::OptMany(_))
    }
}

/// Split a path/pattern into its non-empty segments. Leading/trailing
/// slashes and empty segments are ignored, so `/a/b/` == `a/b`.
fn segments(s: &str) -> Vec<&str> {
    s.split('/').filter(|seg| !seg.is_empty()).collect()
}

/// Parse a capture pattern into elements, rejecting ambiguous shapes:
/// a variable-width capture (`{x*}` / `{x*?}`) must be followed by a
/// literal segment or be the last element, otherwise there is no
/// boundary to split on.
fn parse_pattern(pattern: &str) -> mlua::Result<Vec<Elem>> {
    let mut elems = Vec::new();
    for seg in segments(pattern) {
        let elem = if let Some(inner) = seg.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
            if inner.is_empty() {
                return Err(mlua::Error::runtime(format!(
                    "empty capture name in pattern: {pattern}"
                )));
            }
            match inner.strip_suffix("*?") {
                Some(name) => Elem::OptMany(name.to_string()),
                None => match inner.strip_suffix('*') {
                    Some(name) => Elem::Many(name.to_string()),
                    None => match inner.strip_suffix('?') {
                        Some(name) => Elem::OptOne(name.to_string()),
                        None => Elem::One(inner.to_string()),
                    },
                },
            }
        } else if seg.contains('{') || seg.contains('}') {
            return Err(mlua::Error::runtime(format!(
                "malformed capture segment '{seg}' in pattern: {pattern}"
            )));
        } else {
            Elem::Literal(seg.to_string())
        };
        elems.push(elem);
    }

    // A variable-width capture must be bounded by a following literal
    // (or be last) — never followed by another capture.
    for i in 0..elems.len() {
        if elems[i].is_variable_width() {
            match elems.get(i + 1) {
                None | Some(Elem::Literal(_)) => {}
                Some(_) => {
                    return Err(mlua::Error::runtime(format!(
                        "ambiguous pattern: a multi-segment capture must be \
                         followed by a literal segment or end of pattern: {pattern}"
                    )));
                }
            }
        }
    }
    Ok(elems)
}

/// Match `path_segs` against `elems`, recording captures into `out`.
/// Returns `true` on a full match. Variable-width captures are greedy
/// but bounded by the next literal (or end of input).
fn match_segments(elems: &[Elem], path_segs: &[&str], out: &mut Vec<(String, String)>) -> bool {
    match elems.split_first() {
        // No pattern left: match iff no path left.
        None => path_segs.is_empty(),
        Some((head, rest)) => match head {
            Elem::Literal(lit) => match path_segs.split_first() {
                Some((seg, tail)) if seg == lit => match_segments(rest, tail, out),
                _ => false,
            },
            Elem::One(name) => match path_segs.split_first() {
                Some((seg, tail)) => {
                    out.push((name.clone(), (*seg).to_string()));
                    match_segments(rest, tail, out)
                }
                None => false,
            },
            Elem::OptOne(name) => {
                // Try consuming one segment; fall back to consuming none.
                if let Some((seg, tail)) = path_segs.split_first() {
                    let mark = out.len();
                    out.push((name.clone(), (*seg).to_string()));
                    if match_segments(rest, tail, out) {
                        return true;
                    }
                    out.truncate(mark);
                }
                match_segments(rest, path_segs, out)
            }
            Elem::Many(name) | Elem::OptMany(name) => {
                let min = if matches!(head, Elem::Many(_)) { 1 } else { 0 };
                // Greedy: take as many as possible, back off until the
                // remainder matches. The parser guarantees `rest` starts
                // with a literal or is empty, so this is unambiguous.
                let max = path_segs.len();
                for take in (min..=max).rev() {
                    let captured = path_segs[..take].join("/");
                    let mark = out.len();
                    out.push((name.clone(), captured));
                    if match_segments(rest, &path_segs[take..], out) {
                        return true;
                    }
                    out.truncate(mark);
                }
                false
            }
        },
    }
}

// Scalar path-component extractors, shared by the string and the
// element-wise array overloads.
fn parent_of(p: &str) -> String {
    Path::new(p)
        .parent()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default()
}
fn file_name_of(p: &str) -> String {
    Path::new(p)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}
fn extension_of(p: &str) -> String {
    Path::new(p)
        .extension()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}
fn stem_of(p: &str) -> String {
    Path::new(p)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Coerce a `join`/`product` argument into a flat list of string
/// segments: a string is a single segment, an array (table) must be all
/// strings. Rejects nested arrays and non-string elements.
fn as_segments(arg: &Value) -> mlua::Result<Vec<String>> {
    match arg {
        Value::String(s) => Ok(vec![s.to_str()?.to_owned()]),
        Value::Table(arr) => {
            let mut out = Vec::with_capacity(arr.raw_len());
            for pair in arr.clone().sequence_values::<Value>() {
                let elem = pair?;
                match elem {
                    Value::String(s) => out.push(s.to_str()?.to_owned()),
                    other => {
                        return Err(mlua::Error::runtime(format!(
                            "path: array elements must be strings, got {}",
                            other.type_name()
                        )));
                    }
                }
            }
            Ok(out)
        }
        other => Err(mlua::Error::runtime(format!(
            "path: expected string or array, got {}",
            other.type_name()
        ))),
    }
}

/// Join an ordered list of segments into one path string.
fn join_segments(segs: &[String]) -> String {
    let mut p = PathBuf::new();
    for seg in segs {
        p.push(seg);
    }
    p.to_string_lossy().into_owned()
}

/// `join` over any number of arguments (each a string or string array):
/// flatten all in order, then join into a single path.
fn join_args(args: &[Value]) -> mlua::Result<String> {
    let mut segs = Vec::new();
    for arg in args {
        segs.extend(as_segments(arg)?);
    }
    Ok(join_segments(&segs))
}

/// Borrow each element of an array (table) as a string, rejecting
/// non-strings. Backing for the element-wise array overloads (flat
/// arrays only).
fn string_elems(arr: &Table) -> mlua::Result<Vec<String>> {
    let mut out = Vec::with_capacity(arr.raw_len());
    for pair in arr.clone().sequence_values::<Value>() {
        match pair? {
            Value::String(s) => out.push(s.to_str()?.to_owned()),
            other => {
                return Err(mlua::Error::runtime(format!(
                    "path: array elements must be strings, got {}",
                    other.type_name()
                )));
            }
        }
    }
    Ok(out)
}

/// Match one path against pre-parsed pattern `elems`, returning a table of
/// captures or `nil` on no match. Shared by the scalar and array `capture`.
fn capture_one(lua: &Lua, elems: &[Elem], path: &str) -> mlua::Result<Value> {
    let path_segs = segments(path);
    let mut captured = Vec::new();
    if match_segments(elems, &path_segs, &mut captured) {
        let map = lua.create_table()?;
        for (k, v) in captured {
            map.set(k, v)?;
        }
        Ok(Value::Table(map))
    } else {
        Ok(Value::Nil)
    }
}

/// `path` expressed relative to `base_segs`, or `nil` when it does not lie
/// under it.
///
/// The comparison is whole-segment, which is the whole point of having this
/// here rather than as a `string.sub` at each call site: a lexical prefix test
/// calls `/repo-old` a child of `/repo`, and a trailing slash on either side
/// shifts every offset by one.
fn relative_one(lua: &Lua, base: &str, base_segs: &[&str], path: &str) -> mlua::Result<Value> {
    // An absolute path is not under a relative base, nor the reverse:
    // `segments` drops the leading slash, so without this `"a/b"` would
    // report as `"b"` under `"/a"` -- two paths that name the same file
    // only by coincidence of the cwd.
    if path.starts_with('/') != base.starts_with('/') {
        return Ok(Value::Nil);
    }
    let segs = segments(path);
    if segs.len() < base_segs.len() || &segs[..base_segs.len()] != base_segs {
        return Ok(Value::Nil);
    }
    let rest = &segs[base_segs.len()..];
    let out = match rest.is_empty() {
        true => ".".to_string(),
        false => rest.join("/"),
    };
    Ok(Value::String(lua.create_string(out)?))
}

/// Register the `path` module on `lua`.
pub fn register(lua: &Lua) -> mlua::Result<()> {
    let module = lua.create_table()?;

    // path.join(parts)  -> String  (one array of string segments)
    // path.join(a, b)   -> String  (each of a/b a string or string array)
    // Flatten all arguments in order and join into a single path.
    module.set(
        "join",
        lua.create_function(|_, args: mlua::Variadic<Value>| join_args(&args))?,
    )?;

    // path.product(a, b) -> [String]  (each of a/b a string or string array)
    // Cartesian product of the two axes, each combination joined into a
    // path. An empty axis yields an empty result.
    module.set(
        "product",
        lua.create_function(|_, (a, b): (Value, Value)| {
            let left = as_segments(&a)?;
            let right = as_segments(&b)?;
            let mut out = Vec::with_capacity(left.len() * right.len());
            for l in &left {
                for r in &right {
                    out.push(join_segments(&[l.clone(), r.clone()]));
                }
            }
            Ok(out)
        })?,
    )?;

    // path.relative_to(path, base) -> String | nil
    // path.relative_to(paths, base) -> [String | nil]   (element-wise)
    // `path` expressed relative to `base`, or nil when it does not lie
    // under it. Purely lexical, like everything here: segments are
    // compared whole, so `/repo-old` is not under `/repo` and a trailing
    // slash changes nothing. `path == base` gives "." -- the one form
    // that is still usable as a path, matching what the traversals report
    // for the directory they started in.
    module.set(
        "relative_to",
        lua.create_function(|lua, (subject, base): (Value, String)| {
            let base_segs = segments(&base);
            match subject {
                Value::String(s) => relative_one(lua, &base, &base_segs, s.to_str()?.as_ref()),
                Value::Table(ref arr) => {
                    let paths = string_elems(arr)?;
                    let mut out = Vec::with_capacity(paths.len());
                    for p in &paths {
                        out.push(relative_one(lua, &base, &base_segs, p)?);
                    }
                    Ok(Value::Table(lua.create_sequence_from(out)?))
                }
                other => Err(mlua::Error::runtime(format!(
                    "path: expected string or array, got {}",
                    other.type_name()
                ))),
            }
        })?,
    )?;

    // The single-path component extractors, each with an element-wise
    // array overload (array of paths -> array of results) so consumers
    // can map a whole list without looping in Lua.
    //   path.parent     parent directory ("" when none)
    //   path.file_name  final component ("" if none)
    //   path.extension  extension without the dot ("" if none)
    //   path.stem       file name without its extension
    // A single function takes a Value: a string returns a string, a
    // table (array) returns a table of results.
    macro_rules! component_fn {
        ($name:literal, $f:expr) => {
            module.set(
                $name,
                lua.create_function(|lua, arg: Value| -> mlua::Result<Value> {
                    match arg {
                        Value::String(s) => {
                            Ok(Value::String(lua.create_string($f(s.to_str()?.as_ref()))?))
                        }
                        Value::Table(ref arr) => {
                            let out: Vec<String> =
                                string_elems(arr)?.iter().map(|p| $f(p.as_str())).collect();
                            Ok(Value::Table(lua.create_sequence_from(out)?))
                        }
                        other => Err(mlua::Error::runtime(format!(
                            "path: expected string or array, got {}",
                            other.type_name()
                        ))),
                    }
                })?,
            )?;
        };
    }
    component_fn!("parent", parent_of);
    component_fn!("file_name", file_name_of);
    component_fn!("extension", extension_of);
    component_fn!("stem", stem_of);

    // path.capture(path, pattern) -> table | nil
    // path.capture(paths, pattern) -> [table | nil]   (element-wise)
    // Match `path` against a segment pattern with named captures and
    // return a table of the captures, or `nil` on no match. Placeholders:
    //   {name}    one segment (required)
    //   {name?}   zero or one segment
    //   {name*}   one-or-more segments, joined with "/"
    //   {name*?}  zero-or-more segments, joined with "/"
    // Optionals that match nothing are simply absent from the table. A
    // variable-width capture must be followed by a literal or end the
    // pattern (e.g. "/AWS/{account}/{rest*}/resources/{tail*}"). When the
    // first argument is an array of paths, the pattern is parsed once and
    // matched against each path, returning one result per input.
    module.set(
        "capture",
        lua.create_function(|lua, (subject, pattern): (Value, String)| {
            let elems = parse_pattern(&pattern)?;
            match subject {
                Value::String(s) => capture_one(lua, &elems, s.to_str()?.as_ref()),
                Value::Table(ref arr) => {
                    let paths = string_elems(arr)?;
                    let mut out = Vec::with_capacity(paths.len());
                    for p in &paths {
                        out.push(capture_one(lua, &elems, p)?);
                    }
                    Ok(Value::Table(lua.create_sequence_from(out)?))
                }
                other => Err(mlua::Error::runtime(format!(
                    "path: expected string or array, got {}",
                    other.type_name()
                ))),
            }
        })?,
    )?;

    lua.globals().set("path", module)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mlua::Value;

    fn capture(path: &str, pattern: &str) -> Option<Vec<(String, String)>> {
        let elems = parse_pattern(pattern).expect("pattern parses");
        let segs = segments(path);
        let mut out = Vec::new();
        if match_segments(&elems, &segs, &mut out) {
            Some(out)
        } else {
            None
        }
    }

    fn pairs(p: &[(&str, &str)]) -> Vec<(String, String)> {
        p.iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn relative_to(path: &str, base: &str) -> Option<String> {
        let lua = Lua::new();
        let segs = segments(base);
        match relative_one(&lua, base, &segs, path).expect("relative_to succeeds") {
            Value::String(s) => Some(s.to_str().expect("utf-8").to_owned()),
            Value::Nil => None,
            other => panic!("unexpected {}", other.type_name()),
        }
    }

    #[test]
    fn relative_to_strips_the_base() {
        assert_eq!(
            relative_to("/repo/src/main.rs", "/repo"),
            Some("src/main.rs".to_string())
        );
    }

    #[test]
    fn relative_to_ignores_trailing_slashes() {
        assert_eq!(relative_to("/repo/src/", "/repo/"), Some("src".to_string()));
    }

    #[test]
    fn relative_to_matches_whole_segments_only() {
        // The bug a hand-rolled `string.sub` prefix strip has: `/repo-old`
        // shares a string prefix with `/repo` but is not under it.
        assert_eq!(relative_to("/repo-old/src", "/repo"), None);
    }

    #[test]
    fn relative_to_self_is_dot() {
        assert_eq!(relative_to("/repo", "/repo"), Some(".".to_string()));
    }

    #[test]
    fn relative_to_rejects_a_path_outside_the_base() {
        assert_eq!(relative_to("/other/src", "/repo"), None);
        assert_eq!(relative_to("/repo", "/repo/src"), None);
    }

    #[test]
    fn relative_to_will_not_mix_absolute_and_relative() {
        assert_eq!(relative_to("repo/src", "/repo"), None);
        assert_eq!(relative_to("/repo/src", "repo"), None);
        assert_eq!(relative_to("repo/src", "repo"), Some("src".to_string()));
    }

    #[test]
    fn literals_and_one() {
        assert_eq!(
            capture(
                "/AWS/123/web/resources",
                "/AWS/{account}/{project}/resources"
            ),
            Some(pairs(&[("account", "123"), ("project", "web")]))
        );
    }

    #[test]
    fn one_segment_count_must_match() {
        assert_eq!(
            capture("/AWS/123/web/resources", "/AWS/{account}/resources"),
            None
        );
    }

    #[test]
    fn trailing_many() {
        assert_eq!(
            capture("/AWS/123/web/prod/x", "/AWS/{account}/{rest*}"),
            Some(pairs(&[("account", "123"), ("rest", "web/prod/x")]))
        );
    }

    #[test]
    fn many_bounded_by_literal_then_many() {
        assert_eq!(
            capture(
                "/AWS/123/web/prod/resources/a/b/c",
                "/AWS/{account}/{rest*}/resources/{tail*}"
            ),
            Some(pairs(&[
                ("account", "123"),
                ("rest", "web/prod"),
                ("tail", "a/b/c"),
            ]))
        );
    }

    #[test]
    fn many_requires_at_least_one() {
        // {rest*} needs >=1 segment before the literal `resources`.
        assert_eq!(
            capture("/AWS/resources/x", "/AWS/{rest*}/resources/{tail*}"),
            None
        );
    }

    #[test]
    fn opt_many_allows_zero() {
        assert_eq!(
            capture("/AWS/resources/x", "/AWS/{rest*?}/resources/{tail*}"),
            Some(pairs(&[("rest", ""), ("tail", "x")]))
        );
    }

    #[test]
    fn opt_one_present_and_absent() {
        assert_eq!(
            capture("/a/b/c", "/a/{mid?}/c"),
            Some(pairs(&[("mid", "b")]))
        );
        // absent optional => key omitted
        assert_eq!(capture("/a/c", "/a/{mid?}/c"), Some(pairs(&[])));
    }

    #[test]
    fn no_match_returns_none() {
        assert_eq!(capture("/x/y", "/a/{b}"), None);
    }

    #[test]
    fn trailing_slashes_ignored() {
        assert_eq!(
            capture("/AWS/123/", "/AWS/{account}"),
            Some(pairs(&[("account", "123")]))
        );
    }

    #[test]
    fn ambiguous_adjacent_variable_width_rejected() {
        assert!(parse_pattern("/x/{a*}/{b*}").is_err());
        assert!(parse_pattern("/x/{a*}/{b?}").is_err());
        assert!(parse_pattern("/x/{a*}/{b}").is_err());
        assert!(parse_pattern("/x/{a*}/lit/{b*}").is_ok());
        assert!(parse_pattern("/x/{a}/{b*}").is_ok());
    }

    fn lua() -> Lua {
        let lua = Lua::new();
        register(&lua).expect("register");
        lua
    }

    fn eval_string(src: &str) -> String {
        lua().load(src).eval::<String>().expect("eval string")
    }

    fn eval_strings(src: &str) -> Vec<String> {
        lua().load(src).eval::<Vec<String>>().expect("eval array")
    }

    #[test]
    fn join_variants() {
        assert_eq!(eval_string(r#"return path.join({"/a","b","c"})"#), "/a/b/c");
        assert_eq!(eval_string(r#"return path.join("/a","b")"#), "/a/b");
        assert_eq!(eval_string(r#"return path.join("/a",{"b","c"})"#), "/a/b/c");
        assert_eq!(eval_string(r#"return path.join({"/a","b"},"c")"#), "/a/b/c");
        assert_eq!(
            eval_string(r#"return path.join({"/a","b"},{"c","d"})"#),
            "/a/b/c/d"
        );
    }

    #[test]
    fn product_variants() {
        assert_eq!(
            eval_strings(r#"return path.product({"/a","/b"},{"x","y"})"#),
            ["/a/x", "/a/y", "/b/x", "/b/y"]
        );
        assert_eq!(
            eval_strings(r#"return path.product("/a",{"x","y"})"#),
            ["/a/x", "/a/y"]
        );
        assert!(eval_strings(r#"return path.product({},{"x"})"#).is_empty());
    }

    #[test]
    fn elementwise_overloads() {
        assert_eq!(
            eval_strings(r#"return path.parent({"/a/b","/c/d.txt"})"#),
            ["/a", "/c"]
        );
        assert_eq!(
            eval_strings(r#"return path.file_name({"/a/b","/c/d.txt"})"#),
            ["b", "d.txt"]
        );
        assert_eq!(
            eval_strings(r#"return path.extension({"/c/d.txt","/x"})"#),
            ["txt", ""]
        );
        assert_eq!(eval_strings(r#"return path.stem({"/c/d.txt"})"#), ["d"]);
    }

    #[test]
    fn capture_elementwise() {
        // One entry per input; nil for the non-matching path.
        let lua = lua();
        let out = lua
            .load(r#"return path.capture({"/AWS/1/web","/x/y"}, "/AWS/{acct}/{proj}")"#)
            .eval::<Table>()
            .expect("eval");
        // A sequence with a trailing nil: length is 1 by Lua semantics, so
        // fetch positionally.
        assert!(matches!(out.get::<Value>(1).unwrap(), Value::Table(_)));
        assert!(matches!(out.get::<Value>(2).unwrap(), Value::Nil));
    }

    #[test]
    fn array_elements_must_be_strings() {
        assert!(
            lua()
                .load(r#"return path.parent({{"x"}})"#)
                .eval::<Table>()
                .is_err()
        );
        assert!(
            lua()
                .load(r#"return path.join({1,2})"#)
                .eval::<String>()
                .is_err()
        );
    }
}
