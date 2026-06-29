//! String helper functions.
//!
//! Lua strings are immutable values, so each of these is a pure function
//! that takes a string (plus any arguments) and returns a new string. They
//! extend Luau's built-in `string` library table with a handful of
//! character-aware helpers (crop/set/pad operate on Unicode `chars()`, not
//! bytes):
//!
//! - `trimmed` / `trimmed_start` / `trimmed_end` — strip whitespace.
//! - `truncated(s, len)` — keep the first `len` chars.
//! - `cropped(s, start[, len])` — substring by 1-based char index, matching
//!   `string.sub`'s convention (negative `start` counts from the end, `-1`
//!   is the last char; `len` is optional / nilable). Unlike `string.sub`
//!   (which indexes bytes and can split a multi-byte UTF-8 codepoint in
//!   two), these index whole characters, so they are always safe on
//!   non-ASCII text.
//! - `set(s, index, sub)` — replace the char at 1-based `index` with `sub`
//!   (negative `index` counts from the end, same convention as `cropped`).
//! - `padded(s, len, pad)` — pad on the right with `pad` up to `len` chars.
//! - `removed(s, sub)` — remove every occurrence of `sub`.
//! - `replaced(s, from, to)` — replace every occurrence of `from` with `to`.
//!
//! `upper`/`lower` are intentionally not registered: Luau's built-in
//! `string.upper`/`string.lower` already cover them.

use mlua::{Lua, Table};

/// Register the string helpers on Luau's built-in `string` table.
pub fn register(lua: &Lua) -> mlua::Result<()> {
    let string_table: Table = lua.globals().get("string")?;

    string_table.set(
        "trimmed",
        lua.create_function(|_, s: String| Ok(s.trim().to_string()))?,
    )?;

    string_table.set(
        "trimmed_start",
        lua.create_function(|_, s: String| Ok(s.trim_start().to_string()))?,
    )?;

    string_table.set(
        "trimmed_end",
        lua.create_function(|_, s: String| Ok(s.trim_end().to_string()))?,
    )?;

    string_table.set(
        "truncated",
        lua.create_function(|_, (s, len): (String, i64)| {
            let len = len.max(0) as usize;
            Ok(s.chars().take(len).collect::<String>())
        })?,
    )?;

    string_table.set(
        "cropped",
        lua.create_function(|_, (s, start, len): (String, i64, Option<i64>)| {
            Ok(crop_range(&s, start, len))
        })?,
    )?;

    string_table.set(
        "set",
        lua.create_function(|_, (s, index, sub): (String, i64, String)| {
            Ok(set_at(&s, index, &sub))
        })?,
    )?;

    string_table.set(
        "padded",
        lua.create_function(|_, (s, len, pad): (String, i64, String)| Ok(pad_with(&s, len, &pad)))?,
    )?;

    string_table.set(
        "removed",
        lua.create_function(|_, (s, sub): (String, String)| {
            if sub.is_empty() {
                Ok(s)
            } else {
                Ok(s.replace(sub.as_str(), ""))
            }
        })?,
    )?;

    string_table.set(
        "replaced",
        lua.create_function(|_, (s, from, to): (String, String, String)| {
            if from.is_empty() {
                Ok(s)
            } else {
                Ok(s.replace(from.as_str(), to.as_str()))
            }
        })?,
    )?;

    Ok(())
}

/// Convert a Lua-style 1-based char index (negative counts from the end,
/// `-1` is the last char) to a 0-based offset into `chars`, clamped to
/// `[0, total]`.
fn char_index(index: i64, total: i64) -> i64 {
    let idx = if index < 0 { total + index } else { index - 1 };
    idx.clamp(0, total)
}

fn crop_range(s: &str, start: i64, len: Option<i64>) -> String {
    let chars: Vec<char> = s.chars().collect();
    let total = chars.len() as i64;
    let start = char_index(start, total);
    let end = match len {
        Some(len) => (start + len.max(0)).min(total),
        None => total,
    };
    chars[start as usize..end as usize].iter().collect()
}

fn set_at(s: &str, index: i64, sub: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let len = chars.len() as i64;
    let idx = if index < 0 { len + index } else { index - 1 };
    if idx >= 0 && idx < len {
        let mut out: String = chars[..idx as usize].iter().collect();
        out.push_str(sub);
        out.extend(chars[idx as usize + 1..].iter());
        out
    } else {
        s.to_string()
    }
}

fn pad_with(s: &str, len: i64, pad: &str) -> String {
    if len <= 0 || pad.is_empty() {
        return s.to_string();
    }
    let target = len as usize;
    let mut chars: Vec<char> = s.chars().collect();
    if chars.len() >= target {
        return s.to_string();
    }
    let pad_chars: Vec<char> = pad.chars().collect();
    let mut i = 0;
    while chars.len() < target {
        chars.push(pad_chars[i % pad_chars.len()]);
        i += 1;
    }
    chars.into_iter().collect()
}
