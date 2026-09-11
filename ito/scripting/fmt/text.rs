//! Text helpers: character- and escape-aware string functions.
//!
//! Luau strings are immutable values, so each of these is a pure function that
//! takes a string (plus arguments) and returns a new string. Each is available
//! as `fmt.<name>(s, …)` and as the `string` method `s:<name>(…)`.
//!
//! Positions and lengths count **visible characters**: Unicode `chars()`, not
//! bytes, and with ANSI escape sequences (see [`tokens`]) not counted. The
//! escapes of a styled string are never cut in half: `cropped`/`truncated`/
//! `replaced_at` keep every escape sequence and drop only the visible
//! characters outside the selection, so a style opened before the cut is still
//! closed after it.
//!
//! - `trimmed` / `trimmed_start` / `trimmed_end` — strip Unicode whitespace, or
//!   the characters of an optional `chars` set.
//! - `truncated(s, len[, ellipsis])` — keep at most `len` visible characters.
//! - `cropped(s, start[, len])` — substring by 1-based character index
//!   (negative `start` counts from the end; `len` is a length, not an end
//!   index; both clamp).
//! - `replaced_at(s, index, sub)` — replace the character at `index`; raises
//!   when out of range.
//! - `padded_start` / `padded_end(s, len[, pad])` — pad to `len` characters.
//! - `removed(s, sub[, n])` / `replaced(s, from, to[, n])` — **literal**
//!   (not pattern) removal/replacement of the first `n` (default all).
//! - `starts_with` `ends_with` `contains` `without_prefix` `without_suffix`
//!   `lines` — literal prefix/suffix/substring tests and line splitting.
//! - `fmt.strip(s)` / `s:without_ansi()` and `fmt.width(s)` / `s:width()`.
//!
//! `upper`/`lower` are intentionally not registered: Luau's built-in
//! `string.upper`/`string.lower` already cover them. Invalid UTF-8 raises (the
//! arguments are converted to Rust strings).

use mlua::{Lua, Table};
use unicode_width::UnicodeWidthChar;

use super::def;

type Result<T> = mlua::Result<T>;

/// One piece of a string: an escape sequence, or a visible character.
enum Tok {
    Esc(String),
    Ch(char),
}

/// Split into escape sequences and visible characters.
///
/// Recognised: CSI (`\27[` … final byte `@`–`~`, or the C1 `U+009B`), OSC
/// (`\27]` … BEL or `\27\`), nF escapes (`\27` + intermediates `0x20`–`0x2f`
/// + a final byte) and any other two-byte escape. An unterminated sequence
/// runs to the end of the string, as a terminal would treat it.
fn tokens(s: &str) -> Vec<Tok> {
    let cs: Vec<char> = s.chars().collect();
    let mut out = Vec::with_capacity(cs.len());
    let mut i = 0;
    while i < cs.len() {
        let c = cs[i];
        if c != '\u{1b}' && c != '\u{9b}' {
            out.push(Tok::Ch(c));
            i += 1;
            continue;
        }
        let start = i;
        i += 1;
        let csi = c == '\u{9b}'
            || (i < cs.len() && cs[i] == '[' && {
                i += 1;
                true
            });
        if csi {
            while i < cs.len() {
                let f = cs[i];
                i += 1;
                if ('\u{40}'..='\u{7e}').contains(&f) {
                    break;
                }
            }
        } else if i < cs.len() && cs[i] == ']' {
            i += 1;
            while i < cs.len() {
                let f = cs[i];
                i += 1;
                if f == '\u{7}' {
                    break;
                }
                if f == '\u{1b}' {
                    // The `\` of a String Terminator.
                    i = (i + 1).min(cs.len());
                    break;
                }
            }
        } else {
            while i < cs.len() && ('\u{20}'..='\u{2f}').contains(&cs[i]) {
                i += 1;
            }
            i = (i + 1).min(cs.len());
        }
        out.push(Tok::Esc(cs[start..i].iter().collect()));
    }
    out
}

fn visible(toks: &[Tok]) -> i64 {
    toks.iter().filter(|t| matches!(t, Tok::Ch(_))).count() as i64
}

/// Rebuild the string keeping every escape; `f` decides, per visible
/// character (by visible index), what to emit in its place (`None` drops it).
fn rebuild(toks: &[Tok], mut f: impl FnMut(i64, char) -> Option<String>) -> String {
    let mut out = String::new();
    let mut i = 0;
    for t in toks {
        match t {
            Tok::Esc(e) => out.push_str(e),
            Tok::Ch(c) => {
                if let Some(s) = f(i, *c) {
                    out.push_str(&s);
                }
                i += 1;
            }
        }
    }
    out
}

/// Remove ANSI escape sequences.
pub fn strip(s: &str) -> String {
    tokens(s)
        .into_iter()
        .filter_map(|t| match t {
            Tok::Ch(c) => Some(c),
            Tok::Esc(_) => None,
        })
        .collect()
}

/// Display width: sum of Unicode character widths (wide CJK/emoji = 2,
/// combining marks, tabs, newlines and other controls = 0), escapes excluded.
/// The same measure the table renderer uses.
pub fn width(s: &str) -> usize {
    tokens(s)
        .iter()
        .map(|t| match t {
            Tok::Ch(c) => c.width().unwrap_or(0),
            Tok::Esc(_) => 0,
        })
        .sum()
}

/// A 1-based char index (negative counts from the end, `-1` is the last
/// char) as a 0-based offset, clamped to `[0, total]`.
fn char_index(index: i64, total: i64) -> i64 {
    let idx = if index < 0 { total + index } else { index - 1 };
    idx.clamp(0, total)
}

fn truncated(s: &str, len: i64, ellipsis: &str) -> String {
    let toks = tokens(s);
    let len = len.max(0);
    if visible(&toks) <= len {
        return s.to_string();
    }
    let ell: Vec<char> = ellipsis.chars().collect();
    let tail_len = (ell.len() as i64).min(len);
    let keep = len - tail_len;
    let tail: String = ell[..tail_len as usize].iter().collect();
    rebuild(&toks, |i, c| {
        if i < keep {
            Some(c.to_string())
        } else if i == keep {
            Some(tail.clone())
        } else {
            None
        }
    })
}

fn cropped(s: &str, start: i64, len: Option<i64>) -> String {
    let toks = tokens(s);
    let total = visible(&toks);
    let start = char_index(start, total);
    let end = match len {
        Some(len) => (start + len.max(0)).min(total),
        None => total,
    };
    rebuild(&toks, |i, c| (i >= start && i < end).then(|| c.to_string()))
}

fn replaced_at(s: &str, index: i64, sub: &str) -> Result<String> {
    let toks = tokens(s);
    let total = visible(&toks);
    let idx = if index < 0 { total + index } else { index - 1 };
    if index == 0 || idx < 0 || idx >= total {
        return Err(mlua::Error::runtime(format!(
            "replaced_at: index {index} out of range (string has {total} characters)"
        )));
    }
    Ok(rebuild(&toks, |i, c| {
        Some(if i == idx { sub.to_string() } else { c.to_string() })
    }))
}

fn padded(s: &str, len: i64, pad: &str, start: bool) -> Result<String> {
    if pad.is_empty() {
        return Err(mlua::Error::runtime("pad must not be empty"));
    }
    let n = visible(&tokens(s));
    if len <= n {
        return Ok(s.to_string());
    }
    let fill: String = pad.chars().cycle().take((len - n) as usize).collect();
    Ok(if start {
        format!("{fill}{s}")
    } else {
        format!("{s}{fill}")
    })
}

fn count(n: Option<i64>) -> Result<usize> {
    match n {
        None => Ok(usize::MAX),
        Some(n) if n >= 0 => Ok(n as usize),
        Some(n) => Err(mlua::Error::runtime(format!("n must be >= 0, got {n}"))),
    }
}

fn trim_with(s: &str, chars: Option<&str>, f: impl Fn(&str, &dyn Fn(char) -> bool) -> String) -> String {
    match chars {
        None => f(s, &char::is_whitespace),
        Some(set) => f(s, &|c| set.contains(c)),
    }
}

/// Register the text helpers on the `fmt` table and the `string` table.
pub fn register(lua: &Lua, fmt: &Table, string: &Table) -> Result<()> {
    def(
        fmt,
        string,
        "trimmed",
        lua.create_function(|_, (s, chars): (String, Option<String>)| {
            Ok(trim_with(&s, chars.as_deref(), |s, p| s.trim_matches(p).to_string()))
        })?,
    )?;
    def(
        fmt,
        string,
        "trimmed_start",
        lua.create_function(|_, (s, chars): (String, Option<String>)| {
            Ok(trim_with(&s, chars.as_deref(), |s, p| {
                s.trim_start_matches(p).to_string()
            }))
        })?,
    )?;
    def(
        fmt,
        string,
        "trimmed_end",
        lua.create_function(|_, (s, chars): (String, Option<String>)| {
            Ok(trim_with(&s, chars.as_deref(), |s, p| {
                s.trim_end_matches(p).to_string()
            }))
        })?,
    )?;
    def(
        fmt,
        string,
        "truncated",
        lua.create_function(|_, (s, len, ell): (String, i64, Option<String>)| {
            Ok(truncated(&s, len, ell.as_deref().unwrap_or("")))
        })?,
    )?;
    def(
        fmt,
        string,
        "cropped",
        lua.create_function(|_, (s, start, len): (String, i64, Option<i64>)| {
            Ok(cropped(&s, start, len))
        })?,
    )?;
    def(
        fmt,
        string,
        "replaced_at",
        lua.create_function(|_, (s, index, sub): (String, i64, String)| {
            replaced_at(&s, index, &sub)
        })?,
    )?;
    def(
        fmt,
        string,
        "padded_start",
        lua.create_function(|_, (s, len, pad): (String, i64, Option<String>)| {
            padded(&s, len, pad.as_deref().unwrap_or(" "), true)
        })?,
    )?;
    def(
        fmt,
        string,
        "padded_end",
        lua.create_function(|_, (s, len, pad): (String, i64, Option<String>)| {
            padded(&s, len, pad.as_deref().unwrap_or(" "), false)
        })?,
    )?;
    def(
        fmt,
        string,
        "removed",
        lua.create_function(|_, (s, sub, n): (String, String, Option<i64>)| {
            let n = count(n)?;
            Ok(if sub.is_empty() { s } else { s.replacen(sub.as_str(), "", n) })
        })?,
    )?;
    def(
        fmt,
        string,
        "replaced",
        lua.create_function(|_, (s, from, to, n): (String, String, String, Option<i64>)| {
            let n = count(n)?;
            Ok(if from.is_empty() {
                s
            } else {
                s.replacen(from.as_str(), to.as_str(), n)
            })
        })?,
    )?;
    def(
        fmt,
        string,
        "starts_with",
        lua.create_function(|_, (s, p): (String, String)| Ok(s.starts_with(p.as_str())))?,
    )?;
    def(
        fmt,
        string,
        "ends_with",
        lua.create_function(|_, (s, p): (String, String)| Ok(s.ends_with(p.as_str())))?,
    )?;
    def(
        fmt,
        string,
        "contains",
        lua.create_function(|_, (s, p): (String, String)| Ok(s.contains(p.as_str())))?,
    )?;
    def(
        fmt,
        string,
        "without_prefix",
        lua.create_function(|_, (s, p): (String, String)| {
            Ok(s.strip_prefix(p.as_str()).map(str::to_string).unwrap_or(s))
        })?,
    )?;
    def(
        fmt,
        string,
        "without_suffix",
        lua.create_function(|_, (s, p): (String, String)| {
            Ok(s.strip_suffix(p.as_str()).map(str::to_string).unwrap_or(s))
        })?,
    )?;
    def(
        fmt,
        string,
        "lines",
        lua.create_function(|lua, s: String| lua.create_sequence_from(s.lines()))?,
    )?;

    // `fmt.strip` reads better than `fmt.without_ansi`; as a method the
    // `without_*` family name says it is not whitespace stripping.
    let strip_fn = lua.create_function(|_, s: String| Ok(strip(&s)))?;
    fmt.set("strip", strip_fn.clone())?;
    string.set("without_ansi", strip_fn)?;
    def(
        fmt,
        string,
        "width",
        lua.create_function(|_, s: String| Ok(width(&s) as i64))?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_removes_what_the_styles_add() {
        assert_eq!(strip("\u{1b}[31mred\u{1b}[39m"), "red");
        assert_eq!(strip("\u{1b}[1m\u{1b}[31mx\u{1b}[39m\u{1b}[22m"), "x");
        assert_eq!(strip("plain"), "plain");
        assert_eq!(strip("\u{1b}[38;2;255;128;0mo\u{1b}[0m"), "o");
        // An unterminated CSI is dropped whole, as a terminal would.
        assert_eq!(strip("a\u{1b}[31"), "a");
        // OSC 8 hyperlink, BEL- and ST-terminated.
        assert_eq!(strip("\u{1b}]8;;http://x\u{7}link\u{1b}]8;;\u{7}"), "link");
        assert_eq!(strip("\u{1b}]0;t\u{1b}\\ok"), "ok");
        // C1 CSI, nF and a two-byte escape followed by non-ASCII text.
        assert_eq!(strip("\u{9b}31mé"), "é");
        assert_eq!(strip("\u{1b}(Bx"), "x");
        assert_eq!(strip("\u{1b}7é"), "é");
    }

    #[test]
    fn width_is_unicode_and_escape_aware() {
        assert_eq!(width("hello"), 5);
        assert_eq!(width("héllo"), 5);
        assert_eq!(width("\u{1b}[31mhello\u{1b}[39m"), 5);
        assert_eq!(width("日本"), 4);
        assert_eq!(width("a\tb\nc"), 3, "controls count 0");
    }

    #[test]
    fn truncated_and_cropped_keep_escapes_intact() {
        let s = "\u{1b}[31mhello\u{1b}[39m";
        assert_eq!(truncated(s, 3, ""), "\u{1b}[31mhel\u{1b}[39m");
        assert_eq!(cropped(s, 2, Some(2)), "\u{1b}[31mel\u{1b}[39m");
        assert_eq!(strip(&cropped(s, -3, None)), "llo");
        assert_eq!(truncated("hello", 10, "…"), "hello");
        assert_eq!(truncated("hello", 4, "…"), "hel…");
        assert_eq!(truncated("hello", 2, "..."), "..");
        assert_eq!(truncated("hello", -1, ""), "");
        assert_eq!(cropped("héllo", 2, Some(3)), "éll");
        assert_eq!(cropped("hello", 9, None), "");
    }

    #[test]
    fn replaced_at_raises_out_of_range() {
        assert_eq!(replaced_at("hello", 1, "H").unwrap(), "Hello");
        assert_eq!(replaced_at("hello", -1, "!").unwrap(), "hell!");
        assert!(replaced_at("hello", 6, "x").is_err());
        assert!(replaced_at("hello", 0, "x").is_err());
        assert!(replaced_at("hello", -6, "x").is_err());
    }

    #[test]
    fn padding_counts_visible_characters() {
        assert_eq!(padded("42", 5, "0", false).unwrap(), "42000");
        assert_eq!(padded("42", 5, "ab", true).unwrap(), "aba42");
        assert_eq!(padded("hello", 3, " ", false).unwrap(), "hello");
        let red = "\u{1b}[31mab\u{1b}[39m";
        assert_eq!(strip(&padded(red, 4, " ", false).unwrap()), "ab  ");
        assert!(padded("x", 3, "", false).is_err());
    }

    #[test]
    fn lua_surface() {
        let lua = Lua::new();
        super::super::register(&lua).unwrap();
        let ev = |src: &str| -> String { lua.load(src).eval().unwrap() };
        assert_eq!(ev(r#"return ("  hi  "):trimmed()"#), "hi");
        assert_eq!(ev(r#"return ("xxhixx"):trimmed("x")"#), "hi");
        assert_eq!(ev(r#"return fmt.padded_end("a", 3)"#), "a  ");
        assert_eq!(ev(r#"return ("a-b-c"):replaced("-", "_", 1)"#), "a_b-c");
        assert_eq!(ev(r#"return ("a.b.c"):removed(".", 0)"#), "a.b.c");
        assert_eq!(ev(r#"return ("prefix-x"):without_prefix("prefix-")"#), "x");
        assert_eq!(ev(r#"return ("x.lua"):without_suffix(".lua")"#), "x");
        assert_eq!(ev(r#"return table.concat(("a\r\nb\n\nc\n"):lines(), "|")"#), "a|b||c");
        assert_eq!(ev(r#"return tostring(("abc"):starts_with("ab") and ("abc"):ends_with("bc") and ("abc"):contains("b"))"#), "true");
        assert!(lua.load(r#"return ("a"):removed("a", -1)"#).exec().is_err());
    }
}
