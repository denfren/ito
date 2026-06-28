//! Scalar value decoding: raw token text → logical string.
//!
//! Covers plain, single-quoted, double-quoted, literal (`|`), and folded
//! (`>`) scalar styles per YAML 1.2. Purely syntactic; no tag resolution
//! or type inference (that's the Value layer's job).

use crate::ast::scalar::{Scalar, ScalarStyle};
use crate::syntax::tree_utils::observed_block_col;

/// Errors producible while decoding a scalar's raw text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    /// The scalar node is malformed — e.g., missing its value token.
    Empty,
    /// A quoted scalar token's text is missing its surrounding quotes.
    /// The parser guarantees well-formed tokens; this only triggers for
    /// hand-built trees whose token text is not a complete quoted scalar.
    MalformedQuoted,
    /// An unrecognized escape sequence in a double-quoted scalar.
    InvalidEscape { at: usize, text: String },
    /// A malformed hex/unicode escape payload (`\xGG`, `\uZZZZ`, …).
    InvalidHex { at: usize, text: String },
    /// Block scalar header didn't match `(<digit>)?(<sign>)?` in either order.
    MalformedBlockHeader,
    /// Block scalar indent indicator (1-9) conflicts with body indent.
    BlockIndentMismatch,
}

impl Scalar {
    /// Decode this scalar's raw text into its logical string value.
    pub fn decoded(&self) -> Result<String, DecodeError> {
        let token = self.value_token().ok_or(DecodeError::Empty)?;
        let raw = token.text();
        let style = self.style().ok_or(DecodeError::Empty)?;
        match style {
            ScalarStyle::Plain => Ok(decode_plain(raw)),
            ScalarStyle::SingleQuoted => decode_single_quoted(raw),
            ScalarStyle::DoubleQuoted => decode_double_quoted(raw),
            ScalarStyle::Literal => {
                decode_block(raw, BlockStyle::Literal, self.block_parent_indent())
            }
            ScalarStyle::Folded => {
                decode_block(raw, BlockStyle::Folded, self.block_parent_indent())
            }
        }
    }

    /// The parent indent for this block scalar: the column of the enclosing
    /// block collection (BLOCK_MAPPING or BLOCK_SEQUENCE), or 0 if the scalar
    /// is at the document root.
    fn block_parent_indent(&self) -> usize {
        let mut cur = self.0.parent();
        while let Some(node) = cur {
            if matches!(
                node.kind(),
                crate::SyntaxKind::BLOCK_MAPPING | crate::SyntaxKind::BLOCK_SEQUENCE
            ) {
                return observed_block_col(&node);
            }
            cur = node.parent();
        }
        0
    }
}

fn decode_plain(raw: &str) -> String {
    fold_lines(split_lines(raw).map(trim_line_spaces))
}

fn decode_single_quoted(raw: &str) -> Result<String, DecodeError> {
    // Strip surrounding quotes. The lexer guarantees they're present on
    // well-formed input; a malformed token (hand-built tree) errors.
    let inner = raw
        .strip_prefix('\'')
        .and_then(|s| s.strip_suffix('\''))
        .ok_or(DecodeError::MalformedQuoted)?;
    // Unescape `''` → `'`, then fold lines.
    let unescaped = inner.replace("''", "\x00"); // placeholder to preserve through split
    let folded = fold_quoted_lines(split_lines(&unescaped).collect());
    Ok(folded.replace('\x00', "'"))
}

fn decode_double_quoted(raw: &str) -> Result<String, DecodeError> {
    let inner = raw
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .ok_or(DecodeError::MalformedQuoted)?;
    // Two-phase: unescape (including line-continuation which consumes the
    // following newline + indent), then fold remaining newlines.
    //
    // We scan byte by byte; on `\` we decode one escape; on `\<newline>`
    // the newline plus following indent-whitespace is consumed entirely.
    // Everything else passes through as content.
    let bytes = inner.as_bytes();
    let mut out = String::with_capacity(inner.len());
    let mut i = 0;
    // Newlines need the fold rules applied, so buffer newline-runs and
    // flush them when non-newline content arrives.
    let mut pending_newlines: usize = 0;
    // Byte length of `out` that must never be trimmed as trailing line
    // whitespace. Escape sequences advance this (so escaped trailing
    // whitespace like `\ ` is preserved); literal spaces/tabs do not.
    let mut protected_len = 0usize;
    while i < bytes.len() {
        let b = bytes[i];
        // A raw line break — `\n`, `\r`, or `\r\n` — is normalized to a
        // single fold break (matching the lexer's NEWLINE handling). The
        // `\r` escape (`\\r`) is decoded in the escape branch below and is
        // unaffected.
        if b == b'\n' || b == b'\r' {
            // Trailing whitespace on the line being closed is excluded
            // from the folded content (YAML 1.2 §7.3.1). Only trim
            // literal whitespace — escape-produced whitespace is protected.
            while out.len() > protected_len && matches!(out.as_bytes().last(), Some(b' ' | b'\t')) {
                out.pop();
            }
            pending_newlines += 1;
            i += 1;
            // Consume the `\n` of a `\r\n` pair as part of the same break.
            if b == b'\r' && i < bytes.len() && bytes[i] == b'\n' {
                i += 1;
            }
            // Skip the following line's leading spaces/tabs (indent
            // absorbed by the fold).
            while i < bytes.len() && matches!(bytes[i], b' ' | b'\t') {
                i += 1;
            }
            continue;
        }
        // Flush any pending newlines using plain/single-quoted fold rules:
        // 1 newline → " ", N newlines → (N-1) `\n`.
        if pending_newlines > 0 {
            if pending_newlines == 1 {
                out.push(' ');
            } else {
                for _ in 0..(pending_newlines - 1) {
                    out.push('\n');
                }
            }
            pending_newlines = 0;
            // Fold output (the join space / line feeds) is content and
            // must survive a subsequent trailing-whitespace trim.
            protected_len = out.len();
        }
        if b == b'\\' {
            // Line-continuation: `\<newline>` drops both the backslash and
            // the newline + following indent. Per YAML 1.2 only spaces
            // count as indent; a leading tab on the continuation line is
            // content.
            if i + 1 < bytes.len() && matches!(bytes[i + 1], b'\n' | b'\r') {
                i += 2;
                // Consume the `\n` of a `\r\n` line break.
                if bytes[i - 1] == b'\r' && i < bytes.len() && bytes[i] == b'\n' {
                    i += 1;
                }
                while i < bytes.len() && bytes[i] == b' ' {
                    i += 1;
                }
                continue;
            }
            // Escaped content (including `\t`/`\ ` → space) is never
            // folding whitespace; protect it from end-of-line trimming.
            let (decoded, consumed) = decode_dq_escape(inner, i)?;
            out.push_str(&decoded);
            protected_len = out.len();
            i += consumed;
            continue;
        }
        // Pass through one Unicode char (re-slice to respect UTF-8).
        let ch = inner[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    // Flush any trailing newlines (they were pending at EOL). The same
    // fold rules apply as for interior breaks: a single trailing break
    // folds to a space, N breaks to N-1 newlines (YAML 1.2 §7.3.1).
    if pending_newlines > 0 {
        if pending_newlines == 1 {
            out.push(' ');
        } else {
            for _ in 0..(pending_newlines - 1) {
                out.push('\n');
            }
        }
    }
    Ok(out)
}

/// Decode a single escape sequence starting at `text[at..]` (where
/// `text[at]` is `\\`). Returns the decoded string and the number of
/// bytes consumed from `text` starting at `at` (including the `\\`).
fn decode_dq_escape(text: &str, at: usize) -> Result<(String, usize), DecodeError> {
    let bytes = text.as_bytes();
    debug_assert_eq!(bytes[at], b'\\');
    let next = bytes
        .get(at + 1)
        .copied()
        .ok_or(DecodeError::InvalidEscape {
            at,
            text: "\\".into(),
        })?;
    let (decoded, extra) = match next {
        b'0' => ('\0'.to_string(), 0),
        b'a' => ('\x07'.to_string(), 0),
        b'b' => ('\x08'.to_string(), 0),
        b't' | b'\t' => ('\t'.to_string(), 0),
        b'n' => ('\n'.to_string(), 0),
        b'v' => ('\x0B'.to_string(), 0),
        b'f' => ('\x0C'.to_string(), 0),
        b'r' => ('\r'.to_string(), 0),
        b'e' => ('\x1B'.to_string(), 0),
        b' ' => (' '.to_string(), 0),
        b'"' => ('"'.to_string(), 0),
        b'/' => ('/'.to_string(), 0),
        b'\\' => ('\\'.to_string(), 0),
        b'N' => ('\u{85}'.to_string(), 0),
        b'_' => ('\u{A0}'.to_string(), 0),
        b'L' => ('\u{2028}'.to_string(), 0),
        b'P' => ('\u{2029}'.to_string(), 0),
        b'x' => {
            let hex = hex_slice(text, at + 2, 2)?;
            (unicode_from_hex(hex, at)?, 2)
        }
        b'u' => {
            let hex = hex_slice(text, at + 2, 4)?;
            (unicode_from_hex(hex, at)?, 4)
        }
        b'U' => {
            let hex = hex_slice(text, at + 2, 8)?;
            (unicode_from_hex(hex, at)?, 8)
        }
        other => {
            return Err(DecodeError::InvalidEscape {
                at,
                text: format!("\\{}", other as char),
            });
        }
    };
    Ok((decoded, 2 + extra))
}

fn hex_slice(text: &str, start: usize, len: usize) -> Result<&str, DecodeError> {
    let end = start + len;
    text.get(start..end).ok_or(DecodeError::InvalidHex {
        at: start,
        text: text.get(start..).unwrap_or("").to_string(),
    })
}

fn unicode_from_hex(hex: &str, at: usize) -> Result<String, DecodeError> {
    let code = u32::from_str_radix(hex, 16).map_err(|_| DecodeError::InvalidHex {
        at,
        text: hex.to_string(),
    })?;
    char::from_u32(code)
        .map(|c| c.to_string())
        .ok_or(DecodeError::InvalidHex {
            at,
            text: hex.to_string(),
        })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BlockStyle {
    Literal,
    Folded,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Chomp {
    Strip, // `-` : remove all trailing newlines
    Clip,  // default : keep exactly one trailing newline
    Keep,  // `+` : keep all trailing newlines
}

fn decode_block(raw: &str, style: BlockStyle, parent_indent: usize) -> Result<String, DecodeError> {
    // Header parse: sigil is at raw[0]; then up to two of {digit, +/-}
    // in any order; then optional inline whitespace + comment; then
    // the body starts after the first newline. If there's no newline,
    // the body is empty.
    let bytes = raw.as_bytes();
    if bytes.is_empty() || !matches!(bytes[0], b'|' | b'>') {
        return Err(DecodeError::MalformedBlockHeader);
    }
    let mut i = 1;
    let mut explicit_indent: Option<usize> = None;
    let mut chomp = Chomp::Clip;
    for _ in 0..2 {
        match bytes.get(i) {
            Some(&d @ b'1'..=b'9') if explicit_indent.is_none() => {
                explicit_indent = Some((d - b'0') as usize);
                i += 1;
            }
            Some(b'+') if chomp == Chomp::Clip => {
                chomp = Chomp::Keep;
                i += 1;
            }
            Some(b'-') if chomp == Chomp::Clip => {
                chomp = Chomp::Strip;
                i += 1;
            }
            _ => break,
        }
    }
    // Skip inline whitespace + optional comment up to the first newline.
    while let Some(&b) = bytes.get(i) {
        if b == b'\n' || b == b'\r' {
            break;
        }
        i += 1;
    }
    // Step past the newline terminator of the header.
    let body_start = match bytes.get(i) {
        Some(b'\r') if bytes.get(i + 1) == Some(&b'\n') => i + 2,
        Some(b'\n' | b'\r') => i + 1,
        None => return Ok(String::new()),
        _ => unreachable!(),
    };
    let body = &raw[body_start..];
    if body.is_empty() {
        return Ok(apply_chomp(String::new(), chomp));
    }

    // Split body into lines, keeping each line's content (no trailing
    // newline). A trailing `\r` from a CRLF terminator is not content.
    let lines: Vec<&str> = split_lines(body).collect();
    // `body.split('\n')` yields one extra empty at the end when body ends
    // in `\n`; keep it — it's significant for "trailing newline present".

    // Determine the effective indent. With an explicit indicator digit `n`,
    // the spec defines the content indent as parent_indent + n. Without one,
    // auto-detect from the first non-blank body line.
    let base_indent = match explicit_indent {
        Some(n) => parent_indent + n,
        None => detect_indent(&lines),
    };

    // Strip base_indent from each line (leaving more-indented lines' extra
    // indent intact). Blank lines that have less than base_indent just
    // become empty.
    let stripped: Vec<String> = lines
        .iter()
        .map(|&line| {
            let strip = line
                .bytes()
                .take(base_indent)
                .take_while(|&b| b == b' ')
                .count();
            line[strip..].to_string()
        })
        .collect();

    let joined = match style {
        BlockStyle::Literal => join_literal(&stripped),
        BlockStyle::Folded => join_folded(&stripped),
    };
    Ok(apply_chomp(joined, chomp))
}

/// Auto-detect base indent (YAML 1.2 §8.1.1.1): the indent of the first
/// non-empty line. When the block has no non-empty line, the content
/// indentation is the number of leading spaces on the longest line, so the
/// blank lines reduce to empty content (their spaces are all indentation).
fn detect_indent(lines: &[&str]) -> usize {
    for line in lines {
        let leading_spaces = line.bytes().take_while(|&b| b == b' ').count();
        if leading_spaces < line.len() {
            return leading_spaces;
        }
    }
    // All lines blank: take the maximum leading-space run.
    lines
        .iter()
        .map(|line| line.bytes().take_while(|&b| b == b' ').count())
        .max()
        .unwrap_or(0)
}

/// Literal: join stripped lines, terminating each (including the last)
/// with a `\n`. This yields a trailing newline so `apply_chomp` can clip
/// it to one (Clip), keep it (Keep, including trailing blank lines), or
/// strip it (Strip).
///
/// The body's line split yields a trailing empty element when the body
/// ends in `\n` (the common case); that empty element is the line
/// terminator of the last content line, so it must not produce its own
/// `\n` — otherwise every body would gain a spurious blank line. We emit
/// `\n` after each line *except* a single trailing empty one.
fn join_literal(lines: &[String]) -> String {
    let mut out = String::new();
    // Drop a single trailing empty line: it's the terminator of the last
    // content line, not a blank line of its own.
    let end = if lines.last().map(|l| l.is_empty()).unwrap_or(false) {
        lines.len() - 1
    } else {
        lines.len()
    };
    for line in &lines[..end] {
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// Folded block scalar joining per YAML 1.2 (after base-indent stripping):
///   - Leading blank lines each contribute one `\n` (before the content).
///   - Two adjacent non-blank, non-more-indented lines join with a space.
///   - A more-indented line is separated by `\n` on both sides.
///   - A run of N (≥1) blank lines between two content lines contributes
///     N newlines when both surrounding lines are regular, or N+1 when
///     either is more-indented (the more-indented break is kept literally
///     in addition to the blank lines' own newlines).
///   - The last content line is terminated by one `\n`, plus one `\n` per
///     trailing blank line. `apply_chomp` then clips/keeps/strips them.
fn join_folded(lines: &[String]) -> String {
    let is_more_indented = |s: &str| s.starts_with(' ') || s.starts_with('\t');
    // The body's line split yields a trailing empty element when the body
    // ends in `\n`; that element is the last content line's terminator,
    // not a blank line of its own. Drop it before folding.
    let end = if lines.last().map(|l| l.is_empty()).unwrap_or(false) {
        lines.len() - 1
    } else {
        lines.len()
    };
    let lines = &lines[..end];

    let mut out = String::new();
    let mut i = 0;
    // Leading blank lines → one `\n` each.
    while i < lines.len() && lines[i].is_empty() {
        out.push('\n');
        i += 1;
    }
    if i == lines.len() {
        return out;
    }
    out.push_str(&lines[i]);
    i += 1;
    while i < lines.len() {
        // Count a run of blank lines.
        let run_start = i;
        while i < lines.len() && lines[i].is_empty() {
            i += 1;
        }
        let blanks = i - run_start;
        if i == lines.len() {
            // Trailing blanks: one `\n` per blank line. Chomp adjusts later.
            for _ in 0..blanks {
                out.push('\n');
            }
            break;
        }
        let cur = &lines[i];
        let prev_content = &lines[run_start - 1];
        let prev_more = is_more_indented(prev_content);
        let cur_more = is_more_indented(cur);
        if blanks == 0 {
            // Two adjacent non-blank lines.
            if prev_more || cur_more {
                out.push('\n');
            } else {
                out.push(' ');
            }
        } else {
            // N blank lines: N newlines, plus one more when a more-indented
            // line is involved (its line break is kept literally).
            let count = if prev_more || cur_more {
                blanks + 1
            } else {
                blanks
            };
            for _ in 0..count {
                out.push('\n');
            }
        }
        out.push_str(cur);
        i += 1;
    }
    // Terminate the last content line.
    out.push('\n');
    out
}

fn apply_chomp(mut s: String, chomp: Chomp) -> String {
    match chomp {
        Chomp::Keep => s,
        Chomp::Strip => {
            while s.ends_with('\n') {
                s.pop();
            }
            s
        }
        Chomp::Clip => {
            // Keep the line break of the last non-empty line and drop any
            // trailing empty lines (YAML 1.2 §8.1.1.2). A block with no
            // content (only line breaks) chomps to the empty string — clip
            // keeps a break only when there is content to terminate.
            let had = s.ends_with('\n');
            while s.ends_with('\n') {
                s.pop();
            }
            if had && !s.is_empty() {
                s.push('\n');
            }
            s
        }
    }
}

fn trim_line_spaces(line: &str) -> &str {
    line.trim_matches(|c: char| c == ' ' || c == '\t')
}

/// Fold flow-quoted scalar lines (single-quoted; double-quoted handles
/// its own escapes separately) per YAML 1.2 §7.3.
///
/// Each line break strips the trailing whitespace of the line it closes
/// and the leading whitespace of the next line (that whitespace abuts the
/// break and is not content). A run of `n` breaks then folds: one break to
/// a single space, `n` breaks to `n-1` newlines. This holds even when the
/// scalar has no other content (e.g. `'\n  '` → `" "`, `'\n\n  '` → `"\n"`):
/// the breaks fold among themselves. Only whitespace not adjacent to any
/// break — the first line's leading run and the last line's trailing run —
/// survives as content.
fn fold_quoted_lines(lines: Vec<&str>) -> String {
    if lines.is_empty() {
        return String::new();
    }
    let last = lines.len() - 1;
    let mut out = String::new();
    let mut pending_breaks = 0usize;
    for (i, &line) in lines.iter().enumerate() {
        // Strip whitespace that abuts a break: every line's leading run
        // except the first line's, every line's trailing run except the
        // last line's.
        let line = if i == 0 {
            line
        } else {
            line.trim_start_matches([' ', '\t'])
        };
        let line = if i == last {
            line
        } else {
            line.trim_end_matches([' ', '\t'])
        };
        // Flush the breaks accumulated since the previous emitted content.
        if i > 0 {
            pending_breaks += 1;
        }
        if !line.is_empty() {
            flush_flow_breaks(&mut out, pending_breaks);
            pending_breaks = 0;
            out.push_str(line);
        }
    }
    // Trailing breaks (after the last content) fold the same way.
    flush_flow_breaks(&mut out, pending_breaks);
    out
}

/// Emit a folded run of `n` flow line breaks: nothing for 0, a single
/// space for 1, and `n-1` newlines for `n >= 2` (YAML 1.2 §7.3.1).
fn flush_flow_breaks(out: &mut String, n: usize) {
    match n {
        0 => {}
        1 => out.push(' '),
        _ => {
            for _ in 0..(n - 1) {
                out.push('\n');
            }
        }
    }
}

/// Split raw scalar text into logical lines, treating `\n`, `\r`, and
/// `\r\n` uniformly as line terminators (matching the lexer's NEWLINE
/// handling). Splitting on `\n` alone would leave a trailing `\r` from a
/// CRLF terminator as content; this strips it so folding sees clean lines.
fn split_lines(raw: &str) -> impl Iterator<Item = &str> {
    // Split on `\n` first, then strip a trailing `\r` (CRLF case). Lone
    // `\r` (old-Mac) terminators are handled by the inner split.
    raw.split('\n')
        .flat_map(|line| line.strip_suffix('\r').unwrap_or(line).split('\r'))
}

/// Fold a sequence of trimmed lines per YAML 1.2 line-folding rules used
/// by plain and single-quoted scalars:
/// - Lines separated by a single newline are joined with a single space.
/// - A run of `n` newlines (i.e., `n-1` blank lines between two non-empty
///   lines) contributes `n-1` newlines.
fn fold_lines<'a>(lines: impl Iterator<Item = &'a str>) -> String {
    let lines: Vec<&str> = lines.collect();
    if lines.is_empty() {
        return String::new();
    }
    let mut out = String::with_capacity(lines.iter().map(|l| l.len()).sum::<usize>() + lines.len());
    let mut i = 0;
    // Emit the first non-empty line (or nothing if all are empty).
    while i < lines.len() && lines[i].is_empty() {
        i += 1;
    }
    if i == lines.len() {
        return String::new();
    }
    out.push_str(lines[i]);
    i += 1;
    while i < lines.len() {
        // Count run of consecutive blank lines starting here.
        let mut blanks = 0;
        while i < lines.len() && lines[i].is_empty() {
            blanks += 1;
            i += 1;
        }
        if i == lines.len() {
            break; // trailing blanks dropped
        }
        if blanks == 0 {
            out.push(' ');
        } else {
            // `blanks` blank lines between two content lines fold to
            // `blanks` newlines (the first newline is consumed by the
            // join; each additional blank line adds one newline).
            for _ in 0..blanks {
                out.push('\n');
            }
        }
        out.push_str(lines[i]);
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    //! Tests for Scalar::decoded() for each style.

    use crate::ast::{AstNode, Node, Scalar, Stream};

    fn first_scalar(src: &'static str) -> Scalar {
        let tree = crate::parse(src).expect("parse");
        let stream = Stream::cast(tree.root().clone()).expect("stream");
        let doc = stream.documents().next().expect("doc");
        let root = doc.root_node().expect("root");
        fn find(node: Node) -> Option<Scalar> {
            match node {
                Node::Scalar(s) => Some(s),
                Node::BlockMapping(m) => m.entries().next().and_then(|e| e.value()).and_then(find),
                Node::BlockSequence(s) => s.entries().next().and_then(|e| e.value()).and_then(find),
                _ => None,
            }
        }
        find(root).expect("scalar")
    }

    #[test]
    fn plain_simple() {
        assert_eq!(first_scalar("hello\n").decoded().unwrap(), "hello");
    }

    #[test]
    fn plain_with_internal_spaces() {
        assert_eq!(
            first_scalar("hello world\n").decoded().unwrap(),
            "hello world"
        );
    }

    #[test]
    fn plain_in_mapping_value() {
        assert_eq!(first_scalar("k: v\n").decoded().unwrap(), "v");
    }

    #[test]
    fn plain_multiline_folds_newline_to_space() {
        // "k: line one\n  line two\n" → "line one line two"
        assert_eq!(
            first_scalar("k: line one\n  line two\n").decoded().unwrap(),
            "line one line two"
        );
    }

    #[test]
    fn plain_multiline_three_lines() {
        assert_eq!(first_scalar("k: a\n  b\n  c\n").decoded().unwrap(), "a b c");
    }

    #[test]
    fn plain_multiline_blank_line_is_newline() {
        // Blank line inside a plain scalar folds to a newline.
        assert_eq!(first_scalar("k: a\n\n  b\n").decoded().unwrap(), "a\nb");
    }

    #[test]
    fn plain_multiline_two_blank_lines_is_two_newlines() {
        assert_eq!(first_scalar("k: a\n\n\n  b\n").decoded().unwrap(), "a\n\nb");
    }

    #[test]
    fn single_quoted_simple() {
        assert_eq!(first_scalar("'hello'\n").decoded().unwrap(), "hello");
    }

    #[test]
    fn single_quoted_with_escape() {
        assert_eq!(first_scalar("'it''s'\n").decoded().unwrap(), "it's");
    }

    #[test]
    fn single_quoted_empty() {
        assert_eq!(first_scalar("''\n").decoded().unwrap(), "");
    }

    #[test]
    fn single_quoted_multiline_folds() {
        assert_eq!(
            first_scalar("k: 'line one\n  line two'\n")
                .decoded()
                .unwrap(),
            "line one line two"
        );
    }

    #[test]
    fn single_quoted_multiline_blank_is_newline() {
        assert_eq!(first_scalar("k: 'a\n\n  b'\n").decoded().unwrap(), "a\nb");
    }

    #[test]
    fn single_quoted_escape_across_lines() {
        assert_eq!(
            first_scalar("k: 'it''s\n  great'\n").decoded().unwrap(),
            "it's great"
        );
    }

    #[test]
    fn double_quoted_simple() {
        assert_eq!(first_scalar("\"hello\"\n").decoded().unwrap(), "hello");
    }

    #[test]
    fn double_quoted_empty() {
        assert_eq!(first_scalar("\"\"\n").decoded().unwrap(), "");
    }

    #[test]
    fn double_quoted_escape_newline() {
        assert_eq!(first_scalar("\"a\\nb\"\n").decoded().unwrap(), "a\nb");
    }

    #[test]
    fn double_quoted_escape_tab() {
        assert_eq!(first_scalar("\"a\\tb\"\n").decoded().unwrap(), "a\tb");
    }

    #[test]
    fn double_quoted_escape_backslash() {
        assert_eq!(first_scalar("\"a\\\\b\"\n").decoded().unwrap(), "a\\b");
    }

    #[test]
    fn double_quoted_escape_quote() {
        assert_eq!(first_scalar("\"a\\\"b\"\n").decoded().unwrap(), "a\"b");
    }

    #[test]
    fn double_quoted_hex_escape() {
        assert_eq!(first_scalar("\"A\\x41\"\n").decoded().unwrap(), "AA");
    }

    #[test]
    fn double_quoted_unicode_4hex() {
        assert_eq!(first_scalar("\"\\u00e9\"\n").decoded().unwrap(), "é");
    }

    #[test]
    fn double_quoted_unicode_8hex() {
        assert_eq!(first_scalar("\"\\U0001F600\"\n").decoded().unwrap(), "😀");
    }

    #[test]
    fn double_quoted_null_escape() {
        assert_eq!(first_scalar("\"a\\0b\"\n").decoded().unwrap(), "a\0b");
    }

    #[test]
    fn double_quoted_line_continuation_backslash_newline() {
        // `\<newline>` suppresses the newline; indent on next line absorbed.
        assert_eq!(first_scalar("\"a\\\n  b\"\n").decoded().unwrap(), "ab");
    }

    #[test]
    fn double_quoted_multiline_folds() {
        assert_eq!(
            first_scalar("k: \"line one\n  line two\"\n")
                .decoded()
                .unwrap(),
            "line one line two"
        );
    }

    #[test]
    fn double_quoted_multiline_blank_is_newline() {
        assert_eq!(first_scalar("k: \"a\n\n  b\"\n").decoded().unwrap(), "a\nb");
    }

    #[test]
    fn double_quoted_invalid_escape_is_error() {
        let s = first_scalar("\"a\\qb\"\n");
        assert!(s.decoded().is_err());
    }

    #[test]
    fn double_quoted_slash_escape() {
        // `\/` decodes to `/`.
        assert_eq!(first_scalar("\"a\\/b\"\n").decoded().unwrap(), "a/b");
    }

    #[test]
    fn double_quoted_space_escape() {
        // `\ ` decodes to a literal space.
        assert_eq!(first_scalar("\"a\\ b\"\n").decoded().unwrap(), "a b");
    }

    #[test]
    fn literal_single_line_clip() {
        // Default chomp is clip: one trailing newline.
        assert_eq!(
            first_scalar("k: |\n  hello\n").decoded().unwrap(),
            "hello\n"
        );
    }

    #[test]
    fn literal_two_lines_clip() {
        assert_eq!(
            first_scalar("k: |\n  hello\n  world\n").decoded().unwrap(),
            "hello\nworld\n"
        );
    }

    #[test]
    fn literal_strip() {
        assert_eq!(first_scalar("k: |-\n  hello\n").decoded().unwrap(), "hello");
    }

    #[test]
    fn literal_keep() {
        // Trailing blank lines preserved.
        assert_eq!(
            first_scalar("k: |+\n  hello\n\n").decoded().unwrap(),
            "hello\n\n"
        );
    }

    #[test]
    fn literal_preserves_blank_lines_inside() {
        assert_eq!(
            first_scalar("k: |\n  a\n\n  b\n").decoded().unwrap(),
            "a\n\nb\n"
        );
    }

    #[test]
    fn literal_explicit_indent_indicator() {
        assert_eq!(
            first_scalar("k: |2\n  hello\n").decoded().unwrap(),
            "hello\n"
        );
    }

    #[test]
    fn literal_empty_body() {
        assert_eq!(first_scalar("k: |\n").decoded().unwrap(), "");
    }

    #[test]
    fn literal_more_indented_lines_preserve_extra_indent() {
        // Body is at indent 2; the "  extra" line is at indent 4 → 2 spaces preserved.
        assert_eq!(
            first_scalar("k: |\n  a\n    extra\n  b\n")
                .decoded()
                .unwrap(),
            "a\n  extra\nb\n"
        );
    }

    #[test]
    fn folded_single_line_clip() {
        assert_eq!(
            first_scalar("k: >\n  hello\n").decoded().unwrap(),
            "hello\n"
        );
    }

    #[test]
    fn folded_two_lines_fold_to_space() {
        assert_eq!(
            first_scalar("k: >\n  hello\n  world\n").decoded().unwrap(),
            "hello world\n"
        );
    }

    #[test]
    fn folded_blank_line_becomes_newline() {
        assert_eq!(
            first_scalar("k: >\n  a\n\n  b\n").decoded().unwrap(),
            "a\nb\n"
        );
    }

    #[test]
    fn folded_more_indented_line_keeps_newlines() {
        // "  a\n    indent\n  b" → "a\n  indent\nb\n"
        assert_eq!(
            first_scalar("k: >\n  a\n    indent\n  b\n")
                .decoded()
                .unwrap(),
            "a\n  indent\nb\n"
        );
    }

    #[test]
    fn folded_strip() {
        assert_eq!(first_scalar("k: >-\n  a\n  b\n").decoded().unwrap(), "a b");
    }

    // --- CRLF line endings: raw `\r` must not leak into decoded values. ---

    #[test]
    fn plain_crlf_no_carriage_return() {
        assert_eq!(first_scalar("k: a\r\n  b\r\n").decoded().unwrap(), "a b");
    }

    #[test]
    fn single_quoted_crlf_no_carriage_return() {
        assert_eq!(first_scalar("k: 'a\r\n  b'\r\n").decoded().unwrap(), "a b");
    }

    #[test]
    fn double_quoted_crlf_no_carriage_return() {
        assert_eq!(
            first_scalar("k: \"a\r\n  b\"\r\n").decoded().unwrap(),
            "a b"
        );
    }

    #[test]
    fn literal_crlf_no_carriage_return() {
        assert_eq!(
            first_scalar("k: |\r\n  a\r\n  b\r\n").decoded().unwrap(),
            "a\nb\n"
        );
    }

    #[test]
    fn folded_crlf_no_carriage_return() {
        assert_eq!(
            first_scalar("k: >\r\n  a\r\n  b\r\n").decoded().unwrap(),
            "a b\n"
        );
    }

    #[test]
    fn double_quoted_escaped_carriage_return_preserved() {
        // An escaped `\r` is a literal carriage return in the value; only
        // raw `\r` line-break bytes are normalized away.
        assert_eq!(first_scalar("\"a\\rb\"\n").decoded().unwrap(), "a\rb");
    }
}
