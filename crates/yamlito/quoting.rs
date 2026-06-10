//! One strict scalar-quoting policy, shared by the builder and the
//! sequence/mapping edit paths.
//!
//! Emitting a string scalar means choosing a style that survives a
//! round-trip: a plain scalar where it is unambiguous, otherwise a quoted
//! one. Two pitfalls historically lived in separate, weaker copies of this
//! decision (see FINDINGS.md A4):
//!
//! - A plain scalar must not re-infer as a non-string under the YAML 1.2
//!   Core schema (`null`/`true`/`42`/`1.5`), and — because the crate also
//!   ships fixers for the YAML 1.1 boolean/octal/sexagesimal hazards — must
//!   not look like one of *those* either (`yes`, `010`, `1:30`). This
//!   module mirrors [`crate::lexer::as_plain_scalar`] exactly so the
//!   builder never re-introduces a hazard the fixers exist to remove.
//! - Single-quoted style cannot represent control characters (`\n`, `\t`,
//!   …): a literal newline inside `'...'` line-folds back to a space on
//!   re-read. Such strings must use **double-quoted** style with escapes.
//!   This module owns the only double-quote *emitter* in the crate.
//!
//! Two contexts are distinguished:
//!
//! - **Block** ([`quote_scalar_block`]) — the rules of
//!   [`crate::lexer::as_plain_scalar`]. `,` `[` `]` `{` `}` are already
//!   rejected there (they are flow terminators, illegal even in block
//!   plain scalars), so the block and flow plain-scalar rules coincide.
//! - **Flow** ([`quote_scalar_flow`]) — currently identical to block: the
//!   strict plain rule already forbids every flow terminator, so a string
//!   safe as a block plain scalar is safe as a flow plain scalar too. The
//!   separate entry point documents intent and leaves room for divergence.
//!
//! The decision order for either context: plain if allowed, else
//! single-quoted if it has no control characters, else double-quoted.

use crate::ast::value::infer_plain;
use crate::ast::Value;
use crate::lexer::{as_yaml_1_1_bool, is_yaml_1_1_octal, is_yaml_1_1_sexagesimal};

/// True iff `s` is safe to emit as a **plain** scalar (block or flow) —
/// it parses back to the same string without quoting.
///
/// This is the inverse of "needs quoting" and mirrors
/// [`crate::lexer::as_plain_scalar`] rule-for-rule: empty strings, leading
/// indicators, flow terminators (`,` `[` `]` `{` `}`), `: `/`:`-at-end,
/// ` #` comments, surrounding whitespace, control characters, Core-schema
/// non-strings, and the YAML 1.1 bool/octal/sexagesimal hazards all fail.
fn is_plain_safe(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    let bytes = s.as_bytes();

    // No surrounding whitespace.
    if matches!(bytes[0], b' ' | b'\t') {
        return false;
    }
    if matches!(bytes[bytes.len() - 1], b' ' | b'\t') {
        return false;
    }

    // Leading indicator characters make a plain scalar ambiguous/invalid.
    // `%` is the directive indicator: a document-root scalar starting with
    // it is parsed as a `%YAML`-style directive, so it must be quoted even
    // though it is legal mid-scalar (and so absent from the lexer's
    // content-only `as_plain_scalar` set).
    match bytes[0] {
        b'\'' | b'"' | b'|' | b'>' | b'&' | b'*' | b'!' | b'{' | b'}' | b'[' | b']' | b','
        | b'@' | b'`' | b'#' | b'%' => return false,
        // `-`/`?` only start a plain scalar when not followed by a space
        // (otherwise the lexer reads DASH/QUESTION).
        b'-' | b'?' if bytes.len() == 1 || matches!(bytes[1], b' ' | b'\t' | b'\n' | b'\r') => {
            return false;
        }
        b':' => return false,
        _ => {}
    }

    // Core-schema non-strings (null/bool/int/float) must be quoted.
    if !matches!(infer_plain(s), Value::String(_)) {
        return false;
    }
    // YAML 1.1 hazards the fixers exist to repair must not be re-introduced.
    if as_yaml_1_1_bool(s).is_some() || is_yaml_1_1_octal(s) || is_yaml_1_1_sexagesimal(s) {
        return false;
    }

    // Interior bytes: flow terminators, the `: `/`:`-at-end mapping
    // indicator, ` #` comment start, and any control character (newline,
    // tab, …) all break a plain scalar.
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b',' | b'}' | b']' => return false,
            b':' if matches!(bytes.get(i + 1), None | Some(b' ' | b'\t' | b'\n' | b'\r')) => {
                return false;
            }
            b'#' if i > 0 && matches!(bytes[i - 1], b' ' | b'\t') => return false,
            _ if b < 0x20 || b == 0x7F => return false,
            _ => {}
        }
    }
    true
}

/// True iff `s` can be represented in **single-quoted** style.
///
/// Two classes of string cannot, and must fall through to double quotes:
/// - control characters (U+0000–U+001F, U+007F): single quotes have no
///   escape mechanism for them;
/// - a leading or trailing space/tab: a single-quoted scalar is still
///   line-folded on decode (`ast::decode::decode_single_quoted` trims
///   each line's surrounding whitespace), so `' a'` would decode back to
///   `"a"` — losing the space. Double-quoted scalars preserve them.
fn single_quotable(s: &str) -> bool {
    if s.bytes().any(|b| b < 0x20 || b == 0x7F) {
        return false;
    }
    let bytes = s.as_bytes();
    if matches!(bytes.first(), Some(b' ' | b'\t')) || matches!(bytes.last(), Some(b' ' | b'\t')) {
        return false;
    }
    true
}

/// Single-quote `s`, doubling any embedded `'`. Caller guarantees `s` has
/// no control characters (see [`single_quotable`]).
fn single_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        if c == '\'' {
            out.push('\'');
        }
        out.push(c);
    }
    out.push('\'');
    out
}

/// Double-quote `s`, escaping the characters a YAML 1.2 double-quoted
/// scalar requires: `\` and `"`, the named control escapes
/// (`\n` `\t` `\r` `\0` `\a` `\b` `\f` `\v` `\e`), and any other C0/DEL
/// control character as `\xNN`. This is the inverse of
/// `ast::decode::decode_double_quoted` and is the only double-quote
/// *emitter* in the crate.
fn double_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            '\0' => out.push_str("\\0"),
            '\u{07}' => out.push_str("\\a"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0B}' => out.push_str("\\v"),
            '\u{0C}' => out.push_str("\\f"),
            '\u{1B}' => out.push_str("\\e"),
            // Remaining C0 controls and DEL: hex byte escape.
            c if (c as u32) < 0x20 || c as u32 == 0x7F => {
                out.push_str(&format!("\\x{:02X}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Pick the narrowest safe style for `s` given whether a plain scalar is
/// allowed: plain → single-quoted → double-quoted.
fn quote_with(s: &str, plain_ok: bool) -> String {
    if plain_ok {
        s.to_string()
    } else if single_quotable(s) {
        single_quote(s)
    } else {
        double_quote(s)
    }
}

/// Quote `s` for **block** context, emitting plain when unambiguous,
/// single-quoted when it has no control characters, otherwise
/// double-quoted with escapes.
pub fn quote_scalar_block(s: &str) -> String {
    quote_with(s, is_plain_safe(s))
}

/// Quote `s` for **flow** context (inside `[...]` / `{...}`). The strict
/// plain rule already forbids every flow terminator, so the choice is the
/// same as [`quote_scalar_block`]; the separate entry point names the
/// flow intent and isolates any future divergence.
pub fn quote_scalar_flow(s: &str) -> String {
    quote_with(s, is_plain_safe(s))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse;
    use crate::ast::{AstNode, Node, Stream};

    /// Decode the single scalar value emitted for `s` in block context,
    /// asserting it round-trips back to exactly `s`.
    fn block_round_trip(s: &str) {
        let text = format!("{}\n", quote_scalar_block(s));
        let tree = parse(&text).unwrap_or_else(|e| panic!("parse {text:?}: {e:?}"));
        let stream = Stream::cast(tree.root().clone()).unwrap();
        let doc = stream.documents().next().unwrap();
        let node = doc.root_node().expect("scalar node");
        let Node::Scalar(scalar) = node else {
            panic!("expected scalar for {s:?}, got {node:?}");
        };
        let decoded = scalar.decoded().unwrap_or_else(|e| panic!("decode {s:?}: {e:?}"));
        assert_eq!(decoded, s, "round-trip via {text:?}");
    }

    /// Decode the single scalar value emitted for `s` in flow context
    /// (spliced into a `[...]`), asserting it round-trips back to `s`.
    fn flow_round_trip(s: &str) {
        let text = format!("[{}]\n", quote_scalar_flow(s));
        let tree = parse(&text).unwrap_or_else(|e| panic!("parse {text:?}: {e:?}"));
        let stream = Stream::cast(tree.root().clone()).unwrap();
        let doc = stream.documents().next().unwrap();
        let root = doc.root_node().expect("seq node");
        let seq = root.as_sequence().expect("sequence");
        let item = seq.iter().next().expect("one item");
        let Node::Scalar(scalar) = item else {
            panic!("expected scalar item for {s:?}");
        };
        let decoded = scalar.decoded().unwrap_or_else(|e| panic!("decode {s:?}: {e:?}"));
        assert_eq!(decoded, s, "flow round-trip via {text:?}");
    }

    /// The hostile alphabet from FINDINGS.md §6: every string here must
    /// round-trip through emit → parse → decode unchanged, in both
    /// contexts.
    fn hostile_strings() -> Vec<String> {
        let mut v: Vec<String> = Vec::new();
        // Structural / indicator characters and whitespace.
        for s in [
            "a,b", "a]b", "a}b", "a[b", "a{b", "a:b", "a: b", "a #c", "a#b",
            "a b", " a", "a ", "\ta", "a\t", ":", "::", "-", "- ", "?", "? ",
            "[", "]", "{", "}", ",", "#", "&a", "*a", "!a", "|", ">", "%", "@",
            "`", "'", "\"", "''", "\"\"", "", "a'b", "a\"b", "a\\b",
        ] {
            v.push(s.to_string());
        }
        // Control characters that force double-quoting.
        for s in ["a\nb", "line1\nline2", "a\tb", "a\rb", "\n", "\t", "\r", "\x00", "\x7f", "a\x01b"] {
            v.push(s.to_string());
        }
        // Core-schema and YAML 1.1 hazard words.
        for s in [
            "null", "Null", "NULL", "~", "true", "True", "false", "False",
            "yes", "Yes", "YES", "no", "No", "on", "On", "ON", "off", "Off",
            "OFF", "010", "0o17", "1:30", "12:34:56", "42", "-7", "1.5", ".inf",
            ".nan", "1e3",
        ] {
            v.push(s.to_string());
        }
        // Plain-safe strings that must stay unquoted.
        for s in ["hello", "it's", "a/b/c", "http://x", "a#b", "key_name", "v1.2.3"] {
            v.push(s.to_string());
        }
        v
    }

    #[test]
    fn block_round_trips_hostile_alphabet() {
        for s in hostile_strings() {
            block_round_trip(&s);
        }
    }

    #[test]
    fn flow_round_trips_hostile_alphabet() {
        for s in hostile_strings() {
            flow_round_trip(&s);
        }
    }

    #[test]
    fn newline_uses_double_quotes_not_single() {
        let out = quote_scalar_block("a\nb");
        assert!(out.starts_with('"'), "newline string must double-quote: {out:?}");
        assert_eq!(out, "\"a\\nb\"");
    }

    #[test]
    fn control_chars_hex_escaped() {
        assert_eq!(quote_scalar_block("\x01"), "\"\\x01\"");
        assert_eq!(quote_scalar_block("\x7f"), "\"\\x7F\"");
        assert_eq!(quote_scalar_block("\0"), "\"\\0\"");
    }

    #[test]
    fn yaml_1_1_hazards_quoted() {
        assert_eq!(quote_scalar_block("yes"), "'yes'");
        assert_eq!(quote_scalar_block("on"), "'on'");
        assert_eq!(quote_scalar_block("010"), "'010'");
        assert_eq!(quote_scalar_block("1:30"), "'1:30'");
    }

    #[test]
    fn flow_terminators_quoted() {
        assert_eq!(quote_scalar_flow("a,b"), "'a,b'");
        assert_eq!(quote_scalar_flow("a]b"), "'a]b'");
        assert_eq!(quote_scalar_flow("a}b"), "'a}b'");
    }

    #[test]
    fn plain_strings_stay_plain() {
        assert_eq!(quote_scalar_block("hello"), "hello");
        assert_eq!(quote_scalar_block("it's"), "it's");
        assert_eq!(quote_scalar_flow("hello"), "hello");
    }
}
