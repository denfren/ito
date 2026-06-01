//! `path` module: pure lexical path manipulation.
//!
//! These are total string functions — no disk access, no sandbox
//! resolution. They operate on script-visible paths, so their results
//! feed straight back into the disk-touching `fs::` functions
//! (`fs::read(path::join(dir, name))`). Mirrors the `std::fs` /
//! `std::path` split: `fs::` is I/O, `path::` is string math.

use std::path::{Path, PathBuf};

use rhai::{Array, Dynamic, Engine, EvalAltResult, ImmutableString, Map, Module};

type Result<T> = std::result::Result<T, Box<EvalAltResult>>;

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
fn parse_pattern(pattern: &str) -> Result<Vec<Elem>> {
    let mut elems = Vec::new();
    for seg in segments(pattern) {
        let elem = if let Some(inner) = seg.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
            if inner.is_empty() {
                return Err(format!("empty capture name in pattern: {pattern}").into());
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
            return Err(format!("malformed capture segment '{seg}' in pattern: {pattern}").into());
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
                    return Err(format!(
                        "ambiguous pattern: a multi-segment capture must be \
                         followed by a literal segment or end of pattern: {pattern}"
                    )
                    .into());
                }
            }
        }
    }
    Ok(elems)
}

/// Match `path_segs` against `elems`, recording captures into `out`.
/// Returns `true` on a full match. Variable-width captures are greedy
/// but bounded by the next literal (or end of input).
fn match_segments(elems: &[Elem], path_segs: &[&str], out: &mut Vec<(String, Dynamic)>) -> bool {
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
                    out.push((name.clone(), (*seg).into()));
                    match_segments(rest, tail, out)
                }
                None => false,
            },
            Elem::OptOne(name) => {
                // Try consuming one segment; fall back to consuming none.
                if let Some((seg, tail)) = path_segs.split_first() {
                    let mark = out.len();
                    out.push((name.clone(), (*seg).into()));
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
                    out.push((name.clone(), captured.into()));
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
/// segments: a string is a single segment, an array must be all
/// strings. Rejects nested arrays and non-string elements.
fn as_segments(arg: &Dynamic) -> Result<Vec<String>> {
    if let Some(s) = arg.read_lock::<ImmutableString>() {
        return Ok(vec![s.to_string()]);
    }
    if arg.is_array() {
        let arr = arg.read_lock::<Array>().expect("array");
        let mut out = Vec::with_capacity(arr.len());
        for elem in arr.iter() {
            let s = elem
                .read_lock::<ImmutableString>()
                .ok_or_else(|| -> Box<EvalAltResult> {
                    format!("path: array elements must be strings, got {}", elem.type_name())
                        .into()
                })?;
            out.push(s.to_string());
        }
        return Ok(out);
    }
    Err(format!("path: expected string or array, got {}", arg.type_name()).into())
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
fn join_args(args: &[&Dynamic]) -> Result<ImmutableString> {
    let mut segs = Vec::new();
    for arg in args {
        segs.extend(as_segments(arg)?);
    }
    Ok(join_segments(&segs).into())
}

/// Borrow each element of an array as a string, rejecting non-strings.
/// Backing for the element-wise array overloads (flat arrays only).
fn string_elems(arr: &Array) -> Result<Vec<String>> {
    arr.iter()
        .map(|elem| {
            elem.read_lock::<ImmutableString>()
                .map(|s| s.to_string())
                .ok_or_else(|| -> Box<EvalAltResult> {
                    format!("path: array elements must be strings, got {}", elem.type_name())
                        .into()
                })
        })
        .collect()
}

/// Match one path against pre-parsed pattern `elems`, returning a map of
/// captures or `()` on no match. Shared by the scalar and array `capture`.
fn capture_one(elems: &[Elem], path: &str) -> Dynamic {
    let path_segs = segments(path);
    let mut captured = Vec::new();
    if match_segments(elems, &path_segs, &mut captured) {
        let mut map = Map::new();
        for (k, v) in captured {
            map.insert(k.into(), v);
        }
        map.into()
    } else {
        Dynamic::UNIT
    }
}

/// Register the `path` module on `engine`.
pub fn register(engine: &mut Engine) {
    let mut module = Module::new();

    // path::join(parts) -> String  (one array of string segments)
    // path::join(a, b)  -> String  (each of a/b a string or string array)
    // Flatten all arguments in order and join into a single path.
    module.set_native_fn("join", |parts: Array| -> Result<ImmutableString> {
        let dyns: Vec<&Dynamic> = parts.iter().collect();
        join_args(&dyns)
    });
    module.set_native_fn("join", |a: Dynamic, b: Dynamic| -> Result<ImmutableString> {
        join_args(&[&a, &b])
    });

    // path::product(a, b) -> [String]  (each of a/b a string or string array)
    // Cartesian product of the two axes, each combination joined into a
    // path. An empty axis yields an empty result.
    module.set_native_fn("product", |a: Dynamic, b: Dynamic| -> Result<Array> {
        let left = as_segments(&a)?;
        let right = as_segments(&b)?;
        let mut out = Array::with_capacity(left.len() * right.len());
        for l in &left {
            for r in &right {
                out.push(join_segments(&[l.clone(), r.clone()]).into());
            }
        }
        Ok(out)
    });

    // The single-path component extractors, each with an element-wise
    // array overload (array of paths -> array of results) so consumers
    // can map a whole list without looping in Rhai.
    //   path::parent     parent directory ("" when none)
    //   path::file_name  final component ("" if none)
    //   path::extension  extension without the dot ("" if none)
    //   path::stem       file name without its extension
    macro_rules! component_fn {
        ($name:literal, $f:expr) => {
            module.set_native_fn($name, |path: ImmutableString| -> Result<ImmutableString> {
                Ok($f(path.as_str()).into())
            });
            module.set_native_fn($name, |paths: Array| -> Result<Array> {
                Ok(string_elems(&paths)?
                    .iter()
                    .map(|p| Dynamic::from($f(p)))
                    .collect())
            });
        };
    }
    component_fn!("parent", parent_of);
    component_fn!("file_name", file_name_of);
    component_fn!("extension", extension_of);
    component_fn!("stem", stem_of);

    // path::capture(path, pattern) -> map | ()
    // Match `path` against a segment pattern with named captures and
    // return a map of the captures, or `()` on no match. Placeholders:
    //   {name}    one segment (required)
    //   {name?}   zero or one segment
    //   {name*}   one-or-more segments, joined with "/"
    //   {name*?}  zero-or-more segments, joined with "/"
    // Optionals that match nothing are simply absent from the map. A
    // variable-width capture must be followed by a literal or end the
    // pattern (e.g. "/AWS/{account}/{rest*}/resources/{tail*}").
    module.set_native_fn(
        "capture",
        |path: ImmutableString, pattern: ImmutableString| -> Result<Dynamic> {
            let elems = parse_pattern(&pattern)?;
            Ok(capture_one(&elems, &path))
        },
    );
    // path::capture(paths, pattern) -> [map | ()]   (element-wise)
    // The pattern is parsed once, then matched against each path.
    module.set_native_fn(
        "capture",
        |paths: Array, pattern: ImmutableString| -> Result<Array> {
            let elems = parse_pattern(&pattern)?;
            Ok(string_elems(&paths)?
                .iter()
                .map(|p| capture_one(&elems, p))
                .collect())
        },
    );

    engine.register_static_module("path", module.into());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn capture(path: &str, pattern: &str) -> Option<Vec<(String, String)>> {
        let elems = parse_pattern(pattern).expect("pattern parses");
        let segs = segments(path);
        let mut out = Vec::new();
        if match_segments(&elems, &segs, &mut out) {
            Some(
                out.into_iter()
                    .map(|(k, v)| (k, v.into_string().unwrap()))
                    .collect(),
            )
        } else {
            None
        }
    }

    fn pairs(p: &[(&str, &str)]) -> Vec<(String, String)> {
        p.iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
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

    fn engine() -> Engine {
        let mut e = Engine::new();
        register(&mut e);
        e
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
    fn join_variants() {
        assert_eq!(eval_string(r#"path::join(["/a","b","c"])"#), "/a/b/c");
        assert_eq!(eval_string(r#"path::join("/a","b")"#), "/a/b");
        assert_eq!(eval_string(r#"path::join("/a",["b","c"])"#), "/a/b/c");
        assert_eq!(eval_string(r#"path::join(["/a","b"],"c")"#), "/a/b/c");
        assert_eq!(eval_string(r#"path::join(["/a","b"],["c","d"])"#), "/a/b/c/d");
    }

    #[test]
    fn product_variants() {
        assert_eq!(
            eval_strings(r#"path::product(["/a","/b"],["x","y"])"#),
            ["/a/x", "/a/y", "/b/x", "/b/y"]
        );
        assert_eq!(eval_strings(r#"path::product("/a",["x","y"])"#), ["/a/x", "/a/y"]);
        assert!(eval_strings(r#"path::product([],["x"])"#).is_empty());
    }

    #[test]
    fn elementwise_overloads() {
        assert_eq!(eval_strings(r#"path::parent(["/a/b","/c/d.txt"])"#), ["/a", "/c"]);
        assert_eq!(
            eval_strings(r#"path::file_name(["/a/b","/c/d.txt"])"#),
            ["b", "d.txt"]
        );
        assert_eq!(eval_strings(r#"path::extension(["/c/d.txt","/x"])"#), ["txt", ""]);
        assert_eq!(eval_strings(r#"path::stem(["/c/d.txt"])"#), ["d"]);
    }

    #[test]
    fn capture_elementwise() {
        // One entry per input; () for the non-matching path.
        let out = engine()
            .eval::<Array>(r#"path::capture(["/AWS/1/web","/x/y"], "/AWS/{acct}/{proj}")"#)
            .expect("eval");
        assert_eq!(out.len(), 2);
        assert!(out[0].is_map());
        assert!(out[1].is_unit());
    }

    #[test]
    fn array_elements_must_be_strings() {
        assert!(engine().eval::<Array>(r#"path::parent([["x"]])"#).is_err());
        assert!(engine().eval::<String>(r#"path::join([1,2])"#).is_err());
    }
}
