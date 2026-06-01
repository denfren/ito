//! String method overrides.
//!
//! Several Rhai built-in string methods mutate the string in place but
//! return unit, which surprises anyone expecting the new value back (as
//! in most other languages). We override each — same param types win —
//! so they still mutate in place *and* return the resulting string,
//! enabling assignment and chaining. The covered methods are `trim`,
//! `make_upper`, `make_lower`, `clear`, `truncate`, `crop`, `set`,
//! `pad`, `remove`, and `replace` (each with all their overloads).

use rhai::{Engine, ImmutableString};

pub fn register(engine: &mut Engine) {
    engine.register_fn("trim", |s: &mut ImmutableString| -> ImmutableString {
        let trimmed = s.trim();
        if trimmed != s.as_str() {
            *s = trimmed.into();
        }
        s.clone()
    });

    engine.register_fn("make_upper", |s: &mut ImmutableString| -> ImmutableString {
        let upper = s.to_uppercase();
        if upper != s.as_str() {
            *s = upper.into();
        }
        s.clone()
    });

    engine.register_fn("make_lower", |s: &mut ImmutableString| -> ImmutableString {
        let lower = s.to_lowercase();
        if lower != s.as_str() {
            *s = lower.into();
        }
        s.clone()
    });

    engine.register_fn("clear", |s: &mut ImmutableString| -> ImmutableString {
        if !s.is_empty() {
            *s = "".into();
        }
        s.clone()
    });

    engine.register_fn(
        "truncate",
        |s: &mut ImmutableString, len: i64| -> ImmutableString {
            let len = len.max(0) as usize;
            let truncated: String = s.chars().take(len).collect();
            if truncated != s.as_str() {
                *s = truncated.into();
            }
            s.clone()
        },
    );

    engine.register_fn(
        "crop",
        |s: &mut ImmutableString, start: i64| -> ImmutableString {
            crop_range(s, start, None)
        },
    );

    engine.register_fn(
        "crop",
        |s: &mut ImmutableString, start: i64, len: i64| -> ImmutableString {
            crop_range(s, start, Some(len))
        },
    );

    engine.register_fn(
        "set",
        |s: &mut ImmutableString, index: i64, ch: char| -> ImmutableString {
            let chars: Vec<char> = s.chars().collect();
            let len = chars.len() as i64;
            let idx = if index < 0 { len + index } else { index };
            if idx >= 0 && idx < len {
                let mut chars = chars;
                chars[idx as usize] = ch;
                *s = chars.into_iter().collect::<String>().into();
            }
            s.clone()
        },
    );

    engine.register_fn(
        "pad",
        |s: &mut ImmutableString, len: i64, ch: char| -> ImmutableString {
            pad_with(s, len, &ch.to_string())
        },
    );

    engine.register_fn(
        "pad",
        |s: &mut ImmutableString, len: i64, pad: ImmutableString| -> ImmutableString {
            pad_with(s, len, pad.as_str())
        },
    );

    engine.register_fn(
        "remove",
        |s: &mut ImmutableString, ch: char| -> ImmutableString {
            remove_all(s, &ch.to_string())
        },
    );

    engine.register_fn(
        "remove",
        |s: &mut ImmutableString, sub: ImmutableString| -> ImmutableString {
            remove_all(s, sub.as_str())
        },
    );

    engine.register_fn(
        "replace",
        |s: &mut ImmutableString, from: char, to: char| -> ImmutableString {
            replace_all(s, &from.to_string(), &to.to_string())
        },
    );

    engine.register_fn(
        "replace",
        |s: &mut ImmutableString, from: char, to: ImmutableString| -> ImmutableString {
            replace_all(s, &from.to_string(), to.as_str())
        },
    );

    engine.register_fn(
        "replace",
        |s: &mut ImmutableString, from: ImmutableString, to: char| -> ImmutableString {
            replace_all(s, from.as_str(), &to.to_string())
        },
    );

    engine.register_fn(
        "replace",
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
