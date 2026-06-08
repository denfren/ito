//! String method extensions.
//!
//! Rather than overriding Rhai's built-in mutating methods (which surprised
//! users expecting the mutate-in-place semantics), ito adds two explicit
//! variants for each operation:
//!
//! - `to_*` — pure: returns the result as a new string, never mutates the receiver.
//! - `make_*` — mutates the string in place and returns nothing.
//!
//! Rhai already provides `to_upper`/`to_lower` (pure copies) and
//! `make_upper`/`make_lower` (mutate, return unit) — ito keeps the
//! `make_upper`/`make_lower` unit semantics and adds the rest to match.
//!
//! `join` is added to arrays as a method (no Rhai built-in for it).

use rhai::{Array, Engine, ImmutableString};

pub fn register(engine: &mut Engine) {
    // --- join (array method, unchanged) ---

    engine.register_fn(
        "join",
        |arr: Array, sep: ImmutableString| -> ImmutableString {
            arr.into_iter()
                .map(|v| v.into_string().unwrap_or_default())
                .collect::<Vec<_>>()
                .join(sep.as_str())
                .into()
        },
    );

    // --- trim ---

    engine.register_fn("to_trimmed", |s: ImmutableString| -> ImmutableString {
        s.trim().into()
    });

    engine.register_fn(
        "make_trimmed",
        |s: &mut ImmutableString| {
            *s = s.trim().into();
        },
    );

    // --- trim_start ---

    engine.register_fn(
        "to_trimmed_start",
        |s: ImmutableString| -> ImmutableString { s.trim_start().into() },
    );

    engine.register_fn(
        "make_trimmed_start",
        |s: &mut ImmutableString| {
            *s = s.trim_start().into();
        },
    );

    // --- trim_end ---

    engine.register_fn("to_trimmed_end", |s: ImmutableString| -> ImmutableString {
        s.trim_end().into()
    });

    engine.register_fn(
        "make_trimmed_end",
        |s: &mut ImmutableString| {
            *s = s.trim_end().into();
        },
    );

    // --- upper (Rhai has `to_upper`; we add `make_upper` returning the string) ---

    engine.register_fn("make_upper", |s: &mut ImmutableString| {
        *s = s.to_uppercase().into();
    });

    // --- lower (Rhai has `to_lower`; we add `make_lower` returning the string) ---

    engine.register_fn("make_lower", |s: &mut ImmutableString| {
        *s = s.to_lowercase().into();
    });

    // --- clear ---

    engine.register_fn("to_cleared", |_s: ImmutableString| -> ImmutableString {
        "".into()
    });

    engine.register_fn(
        "make_cleared",
        |s: &mut ImmutableString| {
            *s = "".into();
        },
    );

    // --- truncate ---

    engine.register_fn(
        "to_truncated",
        |s: ImmutableString, len: i64| -> ImmutableString {
            let len = len.max(0) as usize;
            s.chars().take(len).collect::<String>().into()
        },
    );

    engine.register_fn(
        "make_truncated",
        |s: &mut ImmutableString, len: i64| {
            let len = len.max(0) as usize;
            *s = s.chars().take(len).collect::<String>().into();
        },
    );

    // --- crop ---

    engine.register_fn(
        "to_cropped",
        |s: ImmutableString, start: i64| -> ImmutableString {
            let mut tmp = s;
            crop_range(&mut tmp, start, None);
            tmp
        },
    );

    engine.register_fn(
        "to_cropped",
        |s: ImmutableString, start: i64, len: i64| -> ImmutableString {
            let mut tmp = s;
            crop_range(&mut tmp, start, Some(len));
            tmp
        },
    );

    engine.register_fn(
        "make_cropped",
        |s: &mut ImmutableString, start: i64| crop_range(s, start, None),
    );

    engine.register_fn(
        "make_cropped",
        |s: &mut ImmutableString, start: i64, len: i64| crop_range(s, start, Some(len)),
    );

    // --- set ---

    engine.register_fn(
        "to_set",
        |s: ImmutableString, index: i64, ch: char| -> ImmutableString {
            let mut tmp = s;
            set_at(&mut tmp, index, &ch.to_string());
            tmp
        },
    );

    engine.register_fn(
        "to_set",
        |s: ImmutableString, index: i64, sub: ImmutableString| -> ImmutableString {
            let mut tmp = s;
            set_at(&mut tmp, index, sub.as_str());
            tmp
        },
    );

    engine.register_fn(
        "make_set",
        |s: &mut ImmutableString, index: i64, ch: char| set_at(s, index, &ch.to_string()),
    );

    engine.register_fn(
        "make_set",
        |s: &mut ImmutableString, index: i64, sub: ImmutableString| set_at(s, index, sub.as_str()),
    );

    // --- pad ---

    engine.register_fn(
        "to_padded",
        |s: ImmutableString, len: i64, ch: char| -> ImmutableString {
            let mut tmp = s;
            pad_with(&mut tmp, len, &ch.to_string());
            tmp
        },
    );

    engine.register_fn(
        "to_padded",
        |s: ImmutableString, len: i64, pad: ImmutableString| -> ImmutableString {
            let mut tmp = s;
            pad_with(&mut tmp, len, pad.as_str());
            tmp
        },
    );

    engine.register_fn(
        "make_padded",
        |s: &mut ImmutableString, len: i64, ch: char| pad_with(s, len, &ch.to_string()),
    );

    engine.register_fn(
        "make_padded",
        |s: &mut ImmutableString, len: i64, pad: ImmutableString| pad_with(s, len, pad.as_str()),
    );

    // --- remove ---

    engine.register_fn(
        "to_removed",
        |s: ImmutableString, ch: char| -> ImmutableString { s.replace(&ch.to_string(), "").into() },
    );

    engine.register_fn(
        "to_removed",
        |s: ImmutableString, sub: ImmutableString| -> ImmutableString {
            if sub.is_empty() {
                s
            } else {
                s.replace(sub.as_str(), "").into()
            }
        },
    );

    engine.register_fn(
        "make_removed",
        |s: &mut ImmutableString, ch: char| remove_all(s, &ch.to_string()),
    );

    engine.register_fn(
        "make_removed",
        |s: &mut ImmutableString, sub: ImmutableString| remove_all(s, sub.as_str()),
    );

    // --- replace ---

    engine.register_fn(
        "to_replaced",
        |s: ImmutableString, from: char, to: char| -> ImmutableString {
            s.replace(&from.to_string(), &to.to_string()).into()
        },
    );

    engine.register_fn(
        "to_replaced",
        |s: ImmutableString, from: char, to: ImmutableString| -> ImmutableString {
            s.replace(&from.to_string(), to.as_str()).into()
        },
    );

    engine.register_fn(
        "to_replaced",
        |s: ImmutableString, from: ImmutableString, to: char| -> ImmutableString {
            if from.is_empty() {
                s
            } else {
                s.replace(from.as_str(), &to.to_string()).into()
            }
        },
    );

    engine.register_fn(
        "to_replaced",
        |s: ImmutableString, from: ImmutableString, to: ImmutableString| -> ImmutableString {
            if from.is_empty() {
                s
            } else {
                s.replace(from.as_str(), to.as_str()).into()
            }
        },
    );

    engine.register_fn(
        "make_replaced",
        |s: &mut ImmutableString, from: char, to: char| {
            replace_all(s, &from.to_string(), &to.to_string())
        },
    );

    engine.register_fn(
        "make_replaced",
        |s: &mut ImmutableString, from: char, to: ImmutableString| {
            replace_all(s, &from.to_string(), to.as_str())
        },
    );

    engine.register_fn(
        "make_replaced",
        |s: &mut ImmutableString, from: ImmutableString, to: char| {
            replace_all(s, from.as_str(), &to.to_string())
        },
    );

    engine.register_fn(
        "make_replaced",
        |s: &mut ImmutableString, from: ImmutableString, to: ImmutableString| {
            replace_all(s, from.as_str(), to.as_str())
        },
    );
}

fn crop_range(s: &mut ImmutableString, start: i64, len: Option<i64>) {
    let chars: Vec<char> = s.chars().collect();
    let total = chars.len() as i64;
    let start = if start < 0 {
        (total + start).max(0)
    } else {
        start.min(total)
    };
    let end = match len {
        Some(len) => (start + len.max(0)).min(total),
        None => total,
    };
    let cropped: String = chars[start as usize..end as usize].iter().collect();
    if cropped != s.as_str() {
        *s = cropped.into();
    }
}

fn set_at(s: &mut ImmutableString, index: i64, sub: &str) {
    let chars: Vec<char> = s.chars().collect();
    let len = chars.len() as i64;
    let idx = if index < 0 { len + index } else { index };
    if idx >= 0 && idx < len {
        let mut out: String = chars[..idx as usize].iter().collect();
        out.push_str(sub);
        out.extend(chars[idx as usize + 1..].iter());
        *s = out.into();
    }
}

fn pad_with(s: &mut ImmutableString, len: i64, pad: &str) {
    if len <= 0 || pad.is_empty() {
        return;
    }
    let target = len as usize;
    let mut chars: Vec<char> = s.chars().collect();
    if chars.len() >= target {
        return;
    }
    let pad_chars: Vec<char> = pad.chars().collect();
    let mut i = 0;
    while chars.len() < target {
        chars.push(pad_chars[i % pad_chars.len()]);
        i += 1;
    }
    *s = chars.into_iter().collect::<String>().into();
}

fn remove_all(s: &mut ImmutableString, sub: &str) {
    if !sub.is_empty() && s.contains(sub) {
        *s = s.replace(sub, "").into();
    }
}

fn replace_all(s: &mut ImmutableString, from: &str, to: &str) {
    if !from.is_empty() && s.contains(from) {
        *s = s.replace(from, to).into();
    }
}
