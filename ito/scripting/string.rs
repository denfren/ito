//! String method extensions.
//!
//! Rather than overriding Rhai's built-in mutating methods (which surprised
//! users expecting the mutate-in-place semantics), ito adds two explicit
//! variants for each operation:
//!
//! - `to_*` — pure: returns the result as a new string, never mutates the receiver.
//! - `make_*` — mutates the string in place AND returns the result for chaining.
//!
//! Rhai already provides `to_upper`/`to_lower` (pure copies) and
//! `make_upper`/`make_lower` (mutate+unit) — ito keeps `make_upper`/`make_lower`
//! but fixes their return type to `ImmutableString` so they fit the pattern.
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
        |s: &mut ImmutableString| -> ImmutableString {
            let trimmed = s.trim().into();
            *s = trimmed;
            s.clone()
        },
    );

    // --- trim_start ---

    engine.register_fn(
        "to_trimmed_start",
        |s: ImmutableString| -> ImmutableString { s.trim_start().into() },
    );

    engine.register_fn(
        "make_trimmed_start",
        |s: &mut ImmutableString| -> ImmutableString {
            let trimmed = s.trim_start().into();
            *s = trimmed;
            s.clone()
        },
    );

    // --- trim_end ---

    engine.register_fn("to_trimmed_end", |s: ImmutableString| -> ImmutableString {
        s.trim_end().into()
    });

    engine.register_fn(
        "make_trimmed_end",
        |s: &mut ImmutableString| -> ImmutableString {
            let trimmed = s.trim_end().into();
            *s = trimmed;
            s.clone()
        },
    );

    // --- upper (Rhai has `to_upper`; we add `make_upper` returning the string) ---

    engine.register_fn("make_upper", |s: &mut ImmutableString| -> ImmutableString {
        let upper: ImmutableString = s.to_uppercase().into();
        *s = upper;
        s.clone()
    });

    // --- lower (Rhai has `to_lower`; we add `make_lower` returning the string) ---

    engine.register_fn("make_lower", |s: &mut ImmutableString| -> ImmutableString {
        let lower: ImmutableString = s.to_lowercase().into();
        *s = lower;
        s.clone()
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
        |s: &mut ImmutableString, len: i64| -> ImmutableString {
            let len = len.max(0) as usize;
            *s = s.chars().take(len).collect::<String>().into();
            s.clone()
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
        |s: &mut ImmutableString, start: i64, len: i64| -> ImmutableString {
            crop_range(s, start, Some(len))
        },
    );

    // --- set ---

    engine.register_fn(
        "to_set",
        |s: ImmutableString, index: i64, ch: char| -> ImmutableString {
            let mut tmp = s;
            set_char(&mut tmp, index, ch);
            tmp
        },
    );

    engine.register_fn(
        "make_set",
        |s: &mut ImmutableString, index: i64, ch: char| -> ImmutableString {
            set_char(s, index, ch);
            s.clone()
        },
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
        |s: &mut ImmutableString, len: i64, ch: char| -> ImmutableString {
            pad_with(s, len, &ch.to_string())
        },
    );

    engine.register_fn(
        "make_padded",
        |s: &mut ImmutableString, len: i64, pad: ImmutableString| -> ImmutableString {
            pad_with(s, len, pad.as_str())
        },
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
        |s: &mut ImmutableString, sub: ImmutableString| -> ImmutableString {
            remove_all(s, sub.as_str())
        },
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
        |s: &mut ImmutableString, from: char, to: char| -> ImmutableString {
            replace_all(s, &from.to_string(), &to.to_string())
        },
    );

    engine.register_fn(
        "make_replaced",
        |s: &mut ImmutableString, from: char, to: ImmutableString| -> ImmutableString {
            replace_all(s, &from.to_string(), to.as_str())
        },
    );

    engine.register_fn(
        "make_replaced",
        |s: &mut ImmutableString, from: ImmutableString, to: char| -> ImmutableString {
            replace_all(s, from.as_str(), &to.to_string())
        },
    );

    engine.register_fn(
        "make_replaced",
        |s: &mut ImmutableString, from: ImmutableString, to: ImmutableString| -> ImmutableString {
            replace_all(s, from.as_str(), to.as_str())
        },
    );
}

fn crop_range(s: &mut ImmutableString, start: i64, len: Option<i64>) -> ImmutableString {
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
    s.clone()
}

fn set_char(s: &mut ImmutableString, index: i64, ch: char) {
    let chars: Vec<char> = s.chars().collect();
    let len = chars.len() as i64;
    let idx = if index < 0 { len + index } else { index };
    if idx >= 0 && idx < len {
        let mut chars = chars;
        chars[idx as usize] = ch;
        *s = chars.into_iter().collect::<String>().into();
    }
}

fn pad_with(s: &mut ImmutableString, len: i64, pad: &str) -> ImmutableString {
    if len <= 0 || pad.is_empty() {
        return s.clone();
    }
    let target = len as usize;
    let mut chars: Vec<char> = s.chars().collect();
    if chars.len() >= target {
        return s.clone();
    }
    let pad_chars: Vec<char> = pad.chars().collect();
    let mut i = 0;
    while chars.len() < target {
        chars.push(pad_chars[i % pad_chars.len()]);
        i += 1;
    }
    *s = chars.into_iter().collect::<String>().into();
    s.clone()
}

fn remove_all(s: &mut ImmutableString, sub: &str) -> ImmutableString {
    if !sub.is_empty() && s.contains(sub) {
        *s = s.replace(sub, "").into();
    }
    s.clone()
}

fn replace_all(s: &mut ImmutableString, from: &str, to: &str) -> ImmutableString {
    if !from.is_empty() && s.contains(from) {
        *s = s.replace(from, to).into();
    }
    s.clone()
}
