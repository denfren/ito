//! `path` module: pure lexical path manipulation.
//!
//! These are total string functions — no disk access, no sandbox
//! resolution. They operate on script-visible paths, so their results
//! feed straight back into the disk-touching `fs.` functions
//! (`fs.read(path.join(dir, name))`). Mirrors the `std::fs` /
//! `std::path` split: `fs.` is I/O, `path.` is string math.
//!
//! Arguments are strings exactly (no coercion) and must be valid UTF-8; every
//! error is prefixed with the function (`path.join: …`). The batch forms take an
//! array of paths and return one result per input — `false` marks "no result"
//! (no match / not under the base), so the array has no holes and `#out == #in`.

use std::collections::HashSet;
use std::fmt::Display;
use std::path::{Path, PathBuf};

use mlua::{Lua, Table, Value, Variadic};

type R<T> = mlua::Result<T>;

fn err(func: &str, msg: impl Display) -> mlua::Error {
    mlua::Error::runtime(format!("path.{func}: {msg}"))
}

/// A string argument, exactly: no coercion, UTF-8 only.
fn str_arg(func: &str, what: &str, v: &Value) -> R<String> {
    match v {
        Value::String(s) => s
            .to_str()
            .map(|s| s.to_string())
            .map_err(|_| err(func, format!("{what} must be valid UTF-8"))),
        other => Err(err(
            func,
            format!("{what} must be a string, got {}", other.type_name()),
        )),
    }
}

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
    /// `true` for the variable-width captures (`*` / `*?`).
    fn is_variable_width(&self) -> bool {
        matches!(self, Elem::Many(_) | Elem::OptMany(_))
    }

    fn name(&self) -> Option<&str> {
        match self {
            Elem::Literal(_) => None,
            Elem::One(n) | Elem::OptOne(n) | Elem::Many(n) | Elem::OptMany(n) => Some(n),
        }
    }
}

/// Split a path/pattern into its meaningful segments. Leading/trailing
/// slashes, empty segments and `.` are ignored, so `/a/b/` == `a/./b`.
fn segments(s: &str) -> Vec<&str> {
    s.split('/')
        .filter(|seg| !seg.is_empty() && *seg != ".")
        .collect()
}

/// Parse a capture pattern into elements. Rejected: empty or duplicate capture
/// names, a segment mixing literal text with braces, and two variable-width
/// captures (`{x*}` / `{x*?}`) directly adjacent, which have no boundary to
/// split on. Any other order is well defined: a variable-width capture is
/// greedy and backs off until the rest of the pattern matches.
fn parse_pattern(func: &str, pattern: &str) -> R<Vec<Elem>> {
    let mut elems = Vec::new();
    for seg in segments(pattern) {
        let elem = if let Some(inner) = seg.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
            let (name, make): (&str, fn(String) -> Elem) = if let Some(n) = inner.strip_suffix("*?")
            {
                (n, Elem::OptMany)
            } else if let Some(n) = inner.strip_suffix('*') {
                (n, Elem::Many)
            } else if let Some(n) = inner.strip_suffix('?') {
                (n, Elem::OptOne)
            } else {
                (inner, Elem::One)
            };
            if name.is_empty() {
                return Err(err(func, format!("empty capture name in pattern: {pattern}")));
            }
            make(name.to_string())
        } else if seg.contains('{') || seg.contains('}') {
            return Err(err(
                func,
                format!("malformed capture segment '{seg}' in pattern: {pattern}"),
            ));
        } else {
            Elem::Literal(seg.to_string())
        };
        elems.push(elem);
    }

    let mut seen = HashSet::new();
    for e in &elems {
        if let Some(n) = e.name()
            && !seen.insert(n)
        {
            return Err(err(
                func,
                format!("duplicate capture name '{n}' in pattern: {pattern}"),
            ));
        }
    }
    for pair in elems.windows(2) {
        if pair[0].is_variable_width() && pair[1].is_variable_width() {
            return Err(err(
                func,
                format!(
                    "ambiguous pattern: two multi-segment captures must be separated \
                     by a literal or a single-segment capture: {pattern}"
                ),
            ));
        }
    }
    Ok(elems)
}

/// Match `path_segs` against `elems`, recording captures into `out`.
/// Returns `true` on a full match. Variable-width captures are greedy and
/// back off until the remainder matches. A capture that matched nothing is
/// absent from `out` (optional captures never map to `""`).
fn match_segments(elems: &[Elem], path_segs: &[&str], out: &mut Vec<(String, String)>) -> bool {
    match elems.split_first() {
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
                for take in (min..=path_segs.len()).rev() {
                    let mark = out.len();
                    if take > 0 {
                        out.push((name.clone(), path_segs[..take].join("/")));
                    }
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
// element-wise array overloads. They follow `std::path`: see the doc chapter
// for the dotfile / trailing-dot / root cases.
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

/// Lexically clean a path: drop `.` and empty segments, resolve `..` against
/// the segment before it. A leading `..` of a relative path is kept; one at the
/// root of an absolute path is dropped. The empty result is `"."`.
fn normalize_of(p: &str) -> String {
    let abs = p.starts_with('/');
    let mut out: Vec<&str> = Vec::new();
    for seg in p.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                if out.last().is_some_and(|l| *l != "..") {
                    out.pop();
                } else if !abs {
                    out.push("..");
                }
            }
            s => out.push(s),
        }
    }
    let body = out.join("/");
    if abs {
        format!("/{body}")
    } else if body.is_empty() {
        ".".to_string()
    } else {
        body
    }
}

/// Coerce a `join`/`product` argument into a flat list of string
/// segments: a string is a single segment, an array (table) must be all
/// strings. Rejects nested arrays and non-string elements.
fn as_segments(func: &str, what: &str, arg: &Value) -> R<Vec<String>> {
    match arg {
        Value::String(_) => Ok(vec![str_arg(func, what, arg)?]),
        Value::Table(arr) => string_elems(func, what, arr),
        other => Err(err(
            func,
            format!(
                "{what} must be a string or array, got {}",
                other.type_name()
            ),
        )),
    }
}

/// Join an ordered list of segments into one path string. An empty segment is
/// skipped (it would add a trailing slash); an absolute segment replaces
/// everything before it, as `std::path::PathBuf::push` does.
fn join_segments(segs: &[String]) -> String {
    let mut p = PathBuf::new();
    for seg in segs.iter().filter(|s| !s.is_empty()) {
        p.push(seg);
    }
    p.to_string_lossy().into_owned()
}

/// Each element of a flat array as a string, rejecting non-strings.
fn string_elems(func: &str, what: &str, arr: &Table) -> R<Vec<String>> {
    let mut out = Vec::with_capacity(arr.raw_len());
    for elem in arr.clone().sequence_values::<Value>() {
        let elem = elem?;
        match &elem {
            Value::String(_) => out.push(str_arg(func, what, &elem)?),
            other => {
                return Err(err(
                    func,
                    format!(
                        "{what}: array elements must be strings, got {}",
                        other.type_name()
                    ),
                ));
            }
        }
    }
    Ok(out)
}

/// Apply `one` to a string subject, or element-wise to an array of them. In the
/// array form `nil` results become `false`, so the result array has no holes.
fn map_subject(
    lua: &Lua,
    func: &str,
    subject: &Value,
    one: impl Fn(&str) -> R<Value>,
) -> R<Value> {
    match subject {
        Value::String(_) => one(&str_arg(func, "path", subject)?),
        Value::Table(arr) => {
            let paths = string_elems(func, "paths", arr)?;
            let mut out = Vec::with_capacity(paths.len());
            for p in &paths {
                out.push(match one(p)? {
                    Value::Nil => Value::Boolean(false),
                    v => v,
                });
            }
            Ok(Value::Table(lua.create_sequence_from(out)?))
        }
        other => Err(err(
            func,
            format!(
                "path must be a string or array, got {}",
                other.type_name()
            ),
        )),
    }
}

/// `path` expressed relative to `base_segs`, or `None` when it does not lie
/// under it (unless `up`, which reaches it through `..` segments).
///
/// The comparison is whole-segment, which is the whole point of having this
/// here rather than as a `string.sub` at each call site: a lexical prefix test
/// calls `/repo-old` a child of `/repo`, and a trailing slash on either side
/// shifts every offset by one.
fn relative_one(base: &str, base_segs: &[&str], path: &str, up: bool) -> Option<String> {
    // An absolute path is not under a relative base, nor the reverse:
    // `segments` drops the leading slash, so without this `"a/b"` would
    // report as `"b"` under `"/a"`.
    if path.starts_with('/') != base.starts_with('/') {
        return None;
    }
    let segs = segments(path);
    let common = segs
        .iter()
        .zip(base_segs)
        .take_while(|(a, b)| a == b)
        .count();
    if common < base_segs.len() && !up {
        return None;
    }
    let mut parts: Vec<&str> = vec![".."; base_segs.len() - common];
    parts.extend(&segs[common..]);
    Some(if parts.is_empty() {
        ".".to_string()
    } else {
        parts.join("/")
    })
}

fn opt(v: Option<String>, lua: &Lua) -> R<Value> {
    Ok(match v {
        Some(s) => Value::String(lua.create_string(s)?),
        None => Value::Nil,
    })
}

/// Register the `path` module on `lua`.
pub fn register(lua: &Lua) -> R<()> {
    let module = lua.create_table()?;

    // path.join(parts)  -> String  (one array of string segments)
    // path.join(a, b)   -> String  (each of a/b a string or string array)
    module.set(
        "join",
        lua.create_function(|_, args: Variadic<Value>| {
            let mut segs = Vec::new();
            for (i, arg) in args.iter().enumerate() {
                segs.extend(as_segments("join", &format!("argument {}", i + 1), arg)?);
            }
            Ok(join_segments(&segs))
        })?,
    )?;

    // path.product(a, b, ...) -> [String]
    // Cartesian product of the axes (each a string or string array), each
    // combination joined into a path, left-major. An empty axis yields an
    // empty result.
    module.set(
        "product",
        lua.create_function(|_, args: Variadic<Value>| {
            if args.len() < 2 {
                return Err(err("product", "needs at least two axes"));
            }
            let mut acc: Vec<String> = vec![String::new()];
            for (i, arg) in args.iter().enumerate() {
                let axis = as_segments("product", &format!("argument {}", i + 1), arg)?;
                acc = acc
                    .iter()
                    .flat_map(|l| axis.iter().map(move |r| join_segments(&[l.clone(), r.clone()])))
                    .collect();
            }
            Ok(acc)
        })?,
    )?;

    // path.relative_to(path, base[, opts]) -> String | nil
    // path.relative_to(paths, base[, opts]) -> [String | false]   (element-wise)
    // `opts.up = true` also reaches paths outside `base`, through `..`.
    module.set(
        "relative_to",
        lua.create_function(|lua, (subject, base, opts): (Value, Value, Option<Table>)| {
            let base = str_arg("relative_to", "base", &base)?;
            let up = match &opts {
                Some(t) => match t.get::<Value>("up")? {
                    Value::Nil => false,
                    Value::Boolean(b) => b,
                    _ => return Err(err("relative_to", "opts.up must be a boolean")),
                },
                None => false,
            };
            let base_segs = segments(&base);
            map_subject(lua, "relative_to", &subject, |p| {
                opt(relative_one(&base, &base_segs, p, up), lua)
            })
        })?,
    )?;

    // The single-path component extractors, each with an element-wise
    // array overload (array of paths -> array of results).
    macro_rules! component_fn {
        ($name:literal, $f:expr) => {
            module.set(
                $name,
                lua.create_function(|lua, arg: Value| -> R<Value> {
                    map_subject(lua, $name, &arg, |p| {
                        Ok(Value::String(lua.create_string($f(p))?))
                    })
                })?,
            )?;
        };
    }
    component_fn!("parent", parent_of);
    component_fn!("file_name", file_name_of);
    component_fn!("extension", extension_of);
    component_fn!("stem", stem_of);
    component_fn!("normalize", normalize_of);

    // path.is_absolute(path) -> bool
    module.set(
        "is_absolute",
        lua.create_function(|_, p: Value| {
            Ok(Path::new(&str_arg("is_absolute", "path", &p)?).is_absolute())
        })?,
    )?;

    // path.split(path) -> parent, file_name
    module.set(
        "split",
        lua.create_function(|_, p: Value| {
            let p = str_arg("split", "path", &p)?;
            Ok((parent_of(&p), file_name_of(&p)))
        })?,
    )?;

    // path.with_extension(path, ext) -> String   ("" removes it)
    module.set(
        "with_extension",
        lua.create_function(|_, (p, ext): (Value, Value)| {
            let p = str_arg("with_extension", "path", &p)?;
            let ext = str_arg("with_extension", "ext", &ext)?;
            Ok(Path::new(&p)
                .with_extension(ext.trim_start_matches('.'))
                .to_string_lossy()
                .into_owned())
        })?,
    )?;

    // path.with_file_name(path, name) -> String
    module.set(
        "with_file_name",
        lua.create_function(|_, (p, name): (Value, Value)| {
            let p = str_arg("with_file_name", "path", &p)?;
            let name = str_arg("with_file_name", "name", &name)?;
            Ok(Path::new(&p)
                .with_file_name(name)
                .to_string_lossy()
                .into_owned())
        })?,
    )?;

    // path.capture(path, pattern) -> table | nil
    // path.capture(paths, pattern) -> [table | false]   (element-wise)
    // Match `path` against a segment pattern with named captures and
    // return a table of the captures, or `nil` on no match. Placeholders:
    //   {name}    one segment (required)
    //   {name?}   zero or one segment
    //   {name*}   one-or-more segments, joined with "/"
    //   {name*?}  zero-or-more segments, joined with "/"
    // An optional capture that matches nothing is absent from the table. The
    // pattern is parsed once for an array of paths.
    module.set(
        "capture",
        lua.create_function(|lua, (subject, pattern): (Value, Value)| {
            let pattern = str_arg("capture", "pattern", &pattern)?;
            let elems = parse_pattern("capture", &pattern)?;
            map_subject(lua, "capture", &subject, |p| {
                let path_segs = segments(p);
                let mut captured = Vec::new();
                if match_segments(&elems, &path_segs, &mut captured) {
                    let map = lua.create_table()?;
                    for (k, v) in captured {
                        map.set(k, v)?;
                    }
                    Ok(Value::Table(map))
                } else {
                    Ok(Value::Nil)
                }
            })
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
        let elems = parse_pattern("capture", pattern).expect("pattern parses");
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
        relative_one(base, &segments(base), path, false)
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
    fn relative_to_handles_dot_segments_and_up() {
        assert_eq!(relative_to("a/b", "."), Some("a/b".to_string()));
        assert_eq!(relative_to("./a/./b", "a"), Some("b".to_string()));
        let up = |p, b| relative_one(b, &segments(b), p, true);
        assert_eq!(up("/a/x/y", "/a/b/c"), Some("../../x/y".to_string()));
        assert_eq!(up("/a/b", "/a/b/c"), Some("..".to_string()));
        assert_eq!(up("/a/b/c", "/a/b/c"), Some(".".to_string()));
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
        assert_eq!(
            capture("/AWS/resources/x", "/AWS/{rest*}/resources/{tail*}"),
            None
        );
    }

    #[test]
    fn opt_many_zero_is_absent_like_opt_one() {
        assert_eq!(
            capture("/AWS/resources/x", "/AWS/{rest*?}/resources/{tail*}"),
            Some(pairs(&[("tail", "x")]))
        );
    }

    #[test]
    fn opt_one_present_and_absent() {
        assert_eq!(
            capture("/a/b/c", "/a/{mid?}/c"),
            Some(pairs(&[("mid", "b")]))
        );
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
    fn multi_then_single_is_allowed_adjacent_multis_are_not() {
        assert_eq!(
            capture("/a/b/c/d", "/{dirs*}/{file}"),
            Some(pairs(&[("dirs", "a/b/c"), ("file", "d")]))
        );
        assert!(parse_pattern("capture", "/x/{a*}/{b?}").is_ok());
        assert!(parse_pattern("capture", "/x/{a*}/lit/{b*}").is_ok());
        assert!(parse_pattern("capture", "/x/{a*}/{b*}").is_err());
        assert!(parse_pattern("capture", "/x/{a*}/{b*?}").is_err());
    }

    #[test]
    fn empty_and_duplicate_names_are_rejected() {
        for p in ["/{}", "/{*}", "/{?}", "/{*?}", "/{a}/{a?}", "/{a}/x/{a*}"] {
            assert!(parse_pattern("capture", p).is_err(), "{p}");
        }
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

    fn eval_err(src: &str) -> String {
        lua().load(src).eval::<Value>().expect_err("must raise").to_string()
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
    fn join_absolute_replaces_and_empty_is_skipped() {
        assert_eq!(eval_string(r#"return path.join("a", "/b", "c")"#), "/b/c");
        assert_eq!(eval_string(r#"return path.join("a", "")"#), "a");
        assert_eq!(eval_string(r#"return path.join("", "a")"#), "a");
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
        assert_eq!(
            eval_strings(r#"return path.product({"/a"},{"x","y"},{"1"})"#),
            ["/a/x/1", "/a/y/1"]
        );
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
    fn component_edge_cases() {
        assert_eq!(eval_string(r#"return path.extension(".bashrc")"#), "");
        assert_eq!(eval_string(r#"return path.stem(".bashrc")"#), ".bashrc");
        assert_eq!(eval_string(r#"return path.extension("a.tar.gz")"#), "gz");
        assert_eq!(eval_string(r#"return path.stem("a.tar.gz")"#), "a.tar");
        assert_eq!(eval_string(r#"return path.extension("foo.")"#), "");
        assert_eq!(eval_string(r#"return path.stem("foo.")"#), "foo");
        assert_eq!(eval_string(r#"return path.file_name("..")"#), "");
        assert_eq!(eval_string(r#"return path.parent("/")"#), "");
        assert_eq!(eval_string(r#"return path.parent("a")"#), "");
        assert_eq!(eval_string(r#"return path.parent("/a")"#), "/");
    }

    #[test]
    fn new_functions() {
        assert_eq!(eval_string(r#"local d, n = path.split("/a/b.txt") return d .. "|" .. n"#), "/a|b.txt");
        assert_eq!(eval_string(r#"return path.with_extension("/a/b.txt", "md")"#), "/a/b.md");
        assert_eq!(eval_string(r#"return path.with_extension("/a/b.txt", "")"#), "/a/b");
        assert_eq!(eval_string(r#"return path.with_file_name("/a/b.txt", "c")"#), "/a/c");
        assert_eq!(eval_string(r#"return tostring(path.is_absolute("/a")) .. tostring(path.is_absolute("a"))"#), "truefalse");
        assert_eq!(eval_string(r#"return path.normalize("a/./b/../c//d/")"#), "a/c/d");
        assert_eq!(eval_string(r#"return path.normalize("../a")"#), "../a");
        assert_eq!(eval_string(r#"return path.normalize("/../a")"#), "/a");
        assert_eq!(eval_string(r#"return path.normalize("a/..")"#), ".");
    }

    #[test]
    fn batch_results_have_no_holes() {
        let lua = lua();
        let out = lua
            .load(r#"return path.capture({"/AWS/1/web","/x/y","/AWS/2/db"}, "/AWS/{acct}/{proj}")"#)
            .eval::<Table>()
            .expect("eval");
        assert_eq!(out.raw_len(), 3);
        assert!(matches!(out.get::<Value>(1).unwrap(), Value::Table(_)));
        assert!(matches!(out.get::<Value>(2).unwrap(), Value::Boolean(false)));
        assert_eq!(
            eval_string(r#"local r = path.relative_to({"/a/b", "/x"}, "/a") return #r .. tostring(r[2])"#),
            "2false"
        );
    }

    #[test]
    fn errors_name_the_function_and_argument() {
        assert!(eval_err(r#"return path.parent({{"x"}})"#).contains("path.parent: paths: array elements must be strings"));
        assert!(eval_err(r#"return path.join({1,2})"#).contains("path.join: argument 1"));
        assert!(eval_err(r#"return path.capture("a", 5)"#).contains("path.capture: pattern must be a string"));
        assert!(eval_err(r#"return path.relative_to("a", 5)"#).contains("path.relative_to: base must be a string"));
        assert!(eval_err(r#"return path.capture("a", "{}")"#).contains("path.capture: empty capture name"));
        assert!(eval_err(r#"return path.product("a")"#).contains("path.product"));
    }
}
