//! Primitive lexer for ito.
//!
//! The lexer is mode-free. It exposes `peek()` for cheap classification
//! and `bump_simple()` for consumption. Plain scalars are parser-driven
//! via `read_plain(stop)`; quoted scalars are single-call via `bump_simple`.

use crate::ast::value::infer_plain;
use crate::syntax::SyntaxKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LexHint {
    Token(SyntaxKind),
    /// The parser must call `read_plain` with an appropriate stop predicate.
    PlainStart,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PlainStop {
    BlockKey,
    BlockValue,
    FlowKey,
    FlowValue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LexError {
    UnterminatedSingleQuoted { start: usize },
    UnterminatedDoubleQuoted { start: usize },
    EmptyAnchorName { start: usize },
    EmptyAliasName { start: usize },
    UnterminatedVerbatimTag { start: usize },
    MalformedBlockScalarHeader { start: usize },
}

#[derive(Debug)]
pub(crate) struct Lexer<'a> {
    src: &'a str,
    pos: usize,
}

impl<'a> Lexer<'a> {
    pub fn new(src: &'a str) -> Self {
        Self { src, pos: 0 }
    }

    pub fn source(&self) -> &'a str {
        self.src
    }

    pub fn pos(&self) -> usize {
        self.pos
    }

    pub fn set_pos(&mut self, pos: usize) {
        self.pos = pos;
    }

    pub fn at_eof(&self) -> bool {
        self.pos >= self.src.len()
    }

    /// 0-based byte column of `pos` on its current line.
    pub fn column(&self) -> usize {
        let bytes = self.src.as_bytes();
        let mut i = self.pos;
        while i > 0 {
            let b = bytes[i - 1];
            if b == b'\n' || b == b'\r' {
                break;
            }
            i -= 1;
        }
        self.pos - i
    }

    fn byte_at(&self, off: usize) -> Option<u8> {
        self.src.as_bytes().get(off).copied()
    }

    fn peek_byte(&self) -> Option<u8> {
        self.byte_at(self.pos)
    }

    /// Classify the token starting at `pos` without consuming it.
    pub fn peek(&self) -> Option<LexHint> {
        let b = self.peek_byte()?;
        match b {
            b' ' | b'\t' => Some(LexHint::Token(SyntaxKind::WHITESPACE)),
            b'\n' | b'\r' => Some(LexHint::Token(SyntaxKind::NEWLINE)),
            b'#' => {
                // A `#` introduces a comment only if it's at the start of
                // input OR the byte immediately before it is a space/tab
                // or newline. Otherwise `#` is a plain-scalar character.
                let is_comment = self.pos == 0
                    || matches!(
                        self.byte_at(self.pos - 1),
                        Some(b' ' | b'\t' | b'\n' | b'\r')
                    );
                if is_comment {
                    Some(LexHint::Token(SyntaxKind::COMMENT))
                } else {
                    Some(LexHint::PlainStart)
                }
            }
            // BOM (U+FEFF = EF BB BF). Only valid at start of stream, but
            // the lexer classifies it as BOM wherever the bytes appear;
            // the parser enforces the positional rule.
            0xEF if self.byte_at(self.pos + 1) == Some(0xBB)
                && self.byte_at(self.pos + 2) == Some(0xBF) =>
            {
                Some(LexHint::Token(SyntaxKind::BOM))
            }
            // `:` is a mapping value indicator only when followed by
            // space/tab/newline/EOF (block context). Otherwise (`:foo`,
            // `::`) the leading `:` is a plain-scalar character.
            b':' => {
                if matches!(
                    self.byte_at(self.pos + 1),
                    None | Some(b' ' | b'\t' | b'\n' | b'\r')
                ) {
                    Some(LexHint::Token(SyntaxKind::COLON))
                } else {
                    Some(LexHint::PlainStart)
                }
            }
            b',' => Some(LexHint::Token(SyntaxKind::COMMA)),
            b'{' => Some(LexHint::Token(SyntaxKind::L_BRACE)),
            b'}' => Some(LexHint::Token(SyntaxKind::R_BRACE)),
            b'[' => Some(LexHint::Token(SyntaxKind::L_BRACKET)),
            b']' => Some(LexHint::Token(SyntaxKind::R_BRACKET)),
            // `-` is a block-sequence indicator only when followed by
            // space/tab/newline/EOF. Otherwise it starts a plain scalar.
            b'-' => {
                if matches!(
                    self.byte_at(self.pos + 1),
                    None | Some(b' ' | b'\t' | b'\n' | b'\r')
                ) {
                    Some(LexHint::Token(SyntaxKind::DASH))
                } else {
                    Some(LexHint::PlainStart)
                }
            }
            // `?` as indicator (space/tab/newline-terminated) is an
            // explicit-key indicator. `?foo` is a plain scalar.
            b'?' => {
                if matches!(
                    self.byte_at(self.pos + 1),
                    None | Some(b' ' | b'\t' | b'\n' | b'\r')
                ) {
                    Some(LexHint::Token(SyntaxKind::QUESTION))
                } else {
                    Some(LexHint::PlainStart)
                }
            }
            b'\'' => Some(LexHint::Token(SyntaxKind::SINGLE_QUOTED_SCALAR)),
            b'"' => Some(LexHint::Token(SyntaxKind::DOUBLE_QUOTED_SCALAR)),
            b'&' => Some(LexHint::Token(SyntaxKind::ANCHOR)),
            b'*' => Some(LexHint::Token(SyntaxKind::ALIAS)),
            b'!' => Some(LexHint::Token(SyntaxKind::TAG)),
            b'|' => Some(LexHint::Token(SyntaxKind::LITERAL_SCALAR)),
            b'>' => Some(LexHint::Token(SyntaxKind::FOLDED_SCALAR)),
            b'%' if self.column() == 0 => Some(LexHint::Token(SyntaxKind::DIRECTIVE)),
            b'%' => Some(LexHint::PlainStart),
            _ => Some(LexHint::PlainStart),
        }
    }

    /// Consume the token at `pos` when it's a "simple" (non-plain,
    /// non-quoted) token. Returns `(kind, text_slice)`.
    ///
    /// Panics on EOF or when the token requires scalar scanning. Callers
    /// should use `peek()` and dispatch accordingly.
    pub fn bump_simple(&mut self) -> (SyntaxKind, &'a str) {
        let start = self.pos;
        let hint = self.peek().expect("bump_simple at EOF");
        let kind = match hint {
            LexHint::Token(k) => k,
            LexHint::PlainStart => {
                panic!("bump_simple called on PlainStart")
            }
        };
        match kind {
            SyntaxKind::WHITESPACE => {
                while let Some(b) = self.peek_byte() {
                    if b == b' ' || b == b'\t' {
                        self.pos += 1;
                    } else {
                        break;
                    }
                }
            }
            SyntaxKind::NEWLINE => {
                // \r\n is one token, two bytes. Lone \n or \r each one byte.
                let bytes = self.src.as_bytes();
                self.consume_newline(bytes);
            }
            SyntaxKind::COMMENT => {
                // Consume `#` and everything up to but not including
                // the terminating \r or \n.
                while let Some(b) = self.peek_byte() {
                    if b == b'\n' || b == b'\r' {
                        break;
                    }
                    self.pos += 1;
                }
            }
            SyntaxKind::BOM => {
                self.pos += 3;
            }
            SyntaxKind::COLON
            | SyntaxKind::COMMA
            | SyntaxKind::L_BRACE
            | SyntaxKind::R_BRACE
            | SyntaxKind::L_BRACKET
            | SyntaxKind::R_BRACKET
            | SyntaxKind::DASH
            | SyntaxKind::QUESTION => {
                self.pos += 1;
            }
            SyntaxKind::DIRECTIVE => {
                // `%` + everything up to (but not including) the newline.
                while let Some(b) = self.peek_byte() {
                    if b == b'\n' || b == b'\r' {
                        break;
                    }
                    self.pos += 1;
                }
            }
            SyntaxKind::DIRECTIVES_END => {
                // Already positioned at the `-`; consume `---`.
                self.pos += 3;
            }
            SyntaxKind::DOCUMENT_END => {
                self.pos += 3;
            }
            SyntaxKind::SINGLE_QUOTED_SCALAR | SyntaxKind::DOUBLE_QUOTED_SCALAR => {
                panic!("quoted scalars must be consumed via bump_quoted")
            }
            _ => unreachable!("unexpected simple kind {kind:?}"),
        }
        (kind, &self.src[start..self.pos])
    }

    /// Bump a `:` as a COLON token unconditionally. Unlike `bump_simple`,
    /// this does not consult `peek()` (which classifies `:` not followed by
    /// space/newline/EOF as the start of a plain scalar). Flow-context
    /// callers have already established via `is_flow_colon_indicator*` that
    /// the `:` here is a value indicator, even when no space follows.
    pub fn bump_colon(&mut self) -> (SyntaxKind, &'a str) {
        debug_assert_eq!(self.peek_byte(), Some(b':'));
        let start = self.pos;
        self.pos += 1;
        (SyntaxKind::COLON, &self.src[start..self.pos])
    }

    /// Consume a single- or double-quoted scalar starting at `pos`.
    /// Returns the whole token including the surrounding quotes.
    ///
    /// Single-quoted: `''` is an escaped single quote. Any other byte,
    /// including newlines, is content. Terminates at a single `'`.
    ///
    /// Double-quoted: `\X` (any X, including newline) consumes two bytes
    /// as content. Any other byte, including bare newlines, is content.
    /// Terminates at a bare `"`.
    ///
    /// EOF before the closing quote is `LexError::Unterminated*Quoted`.
    pub fn bump_quoted(&mut self) -> Result<(SyntaxKind, &'a str), LexError> {
        let start = self.pos;
        let b = self.peek_byte().expect("bump_quoted at EOF");
        match b {
            b'\'' => {
                self.pos += 1;
                loop {
                    match self.peek_byte() {
                        None => return Err(LexError::UnterminatedSingleQuoted { start }),
                        Some(b'\'') => {
                            // Lookahead for '' escape.
                            if self.byte_at(self.pos + 1) == Some(b'\'') {
                                self.pos += 2;
                            } else {
                                self.pos += 1;
                                return Ok((
                                    SyntaxKind::SINGLE_QUOTED_SCALAR,
                                    &self.src[start..self.pos],
                                ));
                            }
                        }
                        Some(_) => self.pos += 1,
                    }
                }
            }
            b'"' => {
                self.pos += 1;
                loop {
                    match self.peek_byte() {
                        None => return Err(LexError::UnterminatedDoubleQuoted { start }),
                        Some(b'\\') => {
                            // Escape consumes the next byte unconditionally
                            // (the lexer does not decode; it only needs to
                            // avoid misreading \" as a terminator). Includes
                            // `\<newline>` line-continuation escape.
                            match self.byte_at(self.pos + 1) {
                                None => {
                                    return Err(LexError::UnterminatedDoubleQuoted { start });
                                }
                                Some(_) => self.pos += 2,
                            }
                        }
                        Some(b'"') => {
                            self.pos += 1;
                            return Ok((
                                SyntaxKind::DOUBLE_QUOTED_SCALAR,
                                &self.src[start..self.pos],
                            ));
                        }
                        Some(_) => self.pos += 1,
                    }
                }
            }
            _ => panic!("bump_quoted called at non-quote byte"),
        }
    }

    /// Scan a plain scalar starting at `pos`, stopping according to
    /// `stop`. When `min_indent` is `Some(n)`, the scalar may span
    /// multiple lines: a continuation line is accepted if its indent is
    /// strictly greater than `n` and it doesn't start with a block
    /// indicator (`-`, `?`) or a mapping key (`key: `).
    ///
    /// Does not consume the terminator. Trailing ASCII-space/tab bytes
    /// between the last non-space content on the final line and the
    /// terminator are NOT part of the returned slice.
    #[cfg(test)]
    pub fn read_plain(&mut self, stop: PlainStop) -> &'a str {
        self.read_plain_with(stop, None)
    }

    /// `min_indent` is the indent barrier for multi-line plain-scalar
    /// continuation: a continuation line is accepted only when its indent
    /// is **strictly greater** than this value. The document root passes
    /// `Some(-1)` so a column-0 line still continues (the document-level
    /// indent is −1 per YAML 1.2 §7.3.3); nested block scalars pass their
    /// parent container's indent (>= 0).
    pub fn read_plain_with(&mut self, stop: PlainStop, min_indent: Option<isize>) -> &'a str {
        let start = self.pos;
        let mut last_non_space_end = self.scan_plain_line(stop);

        // Multi-line continuation applies in block context (bounded by an
        // explicit indent barrier) and in flow context (where a plain
        // scalar may span lines and fold; the flow terminators `,]}` and
        // `: ` bound it, handled by scan_plain_line).
        let is_flow = matches!(stop, PlainStop::FlowKey | PlainStop::FlowValue);
        let multiline_ok = (matches!(stop, PlainStop::BlockValue | PlainStop::BlockKey)
            && min_indent.is_some())
            || is_flow;
        if multiline_ok {
            // Block context uses the supplied indent barrier; flow context
            // is not indentation-bounded, so any indent (>= 0) continues.
            let min = min_indent.unwrap_or(0);
            loop {
                // Save the state before attempting a continuation.
                let line_end = self.pos;
                // Advance to the first non-blank line.
                let Some(cont_indent) = self.skip_blank_lines_for_continuation() else {
                    // EOF reached; stop.
                    self.pos = line_end;
                    break;
                };
                // A continuation line whose first non-blank byte is `#` is a
                // comment line, not scalar content — terminate the scalar and
                // leave the comment as trivia (applies in flow and block).
                if self.src.as_bytes().get(self.pos) == Some(&b'#') {
                    self.pos = line_end;
                    break;
                }
                if !is_flow && (cont_indent as isize) <= min {
                    self.pos = line_end;
                    break;
                }
                // A `- `/`? ` block indicator only ends the scalar when the
                // line sits at the owning container's indent (a real
                // sibling). The text-fold barrier `min` is −1 at the
                // document root, so the structural level there is column 0;
                // a deeper `- ` (e.g. `- a\n - b` at column 1) is folded
                // scalar text, not a nested sequence.
                let structural_indent = min.max(0);
                let break_on_dash_question = (cont_indent as isize) <= structural_indent;
                if !is_flow
                    && self.line_starts_with_block_indicator_or_key(stop, break_on_dash_question)
                {
                    self.pos = line_end;
                    break;
                }
                // Valid continuation: consume this line. The blank lines
                // and the indent whitespace between `line_end` and `self.pos`
                // are now part of the scalar. Scan the line content.
                let content_start = self.pos;
                let line_content_end = self.scan_plain_line(stop);
                if line_content_end == content_start {
                    // Line had no content (e.g., started with `#` which
                    // scan_plain_line would have rejected). Roll back so
                    // the newline stays as trivia.
                    self.pos = line_end;
                    break;
                }
                last_non_space_end = line_content_end;
            }
        }

        self.pos = last_non_space_end;
        &self.src[start..self.pos]
    }

    /// Scan one line's worth of plain scalar content. Returns the end of
    /// the last non-space byte on the line. Leaves `self.pos` at either
    /// the line's terminator (`\n`, `\r`, EOF) or just past trailing
    /// internal whitespace (but not past the line's terminator).
    fn scan_plain_line(&mut self, stop: PlainStop) -> usize {
        let bytes = self.src.as_bytes();
        let mut last_non_space_end = self.pos;
        while let Some(&b) = bytes.get(self.pos) {
            match b {
                b'\n' | b'\r' => break,
                b' ' | b'\t' => {
                    // Find the end of the whole whitespace run once.
                    let mut look = self.pos + 1;
                    while let Some(&c) = bytes.get(look) {
                        if c == b' ' || c == b'\t' {
                            look += 1;
                        } else {
                            break;
                        }
                    }
                    match bytes.get(look) {
                        None | Some(b'\n' | b'\r') => break,
                        Some(b'#') => break,
                        // Interior whitespace: skip the entire run at once
                        // (it does not advance `last_non_space_end`).
                        _ => self.pos = look,
                    }
                }
                b':' if matches!(stop, PlainStop::BlockKey | PlainStop::BlockValue)
                    && matches!(
                        bytes.get(self.pos + 1),
                        None | Some(b' ' | b'\t' | b'\n' | b'\r')
                    ) =>
                {
                    break;
                }
                b':' if matches!(stop, PlainStop::FlowKey | PlainStop::FlowValue)
                    && matches!(
                        bytes.get(self.pos + 1),
                        None | Some(b' ' | b'\t' | b'\n' | b'\r' | b',' | b'}' | b']')
                    ) =>
                {
                    break;
                }
                b',' | b'}' | b']' if matches!(stop, PlainStop::FlowKey | PlainStop::FlowValue) => {
                    break;
                }
                _ => {
                    self.pos += 1;
                    last_non_space_end = self.pos;
                }
            }
        }
        last_non_space_end
    }

    /// Consume a single newline (`\r\n`, `\r`, or `\n`) at `pos`.
    fn consume_newline(&mut self, bytes: &[u8]) {
        match bytes.get(self.pos) {
            Some(b'\r') => {
                self.pos += 1;
                if bytes.get(self.pos) == Some(&b'\n') {
                    self.pos += 1;
                }
            }
            Some(b'\n') => self.pos += 1,
            _ => {}
        }
    }

    /// Advance past one or more newlines (plus any trailing blank lines)
    /// to position `pos` at the first non-whitespace byte of the next
    /// non-blank line, or at EOF. Returns the indent (column) of that
    /// line, or `None` at EOF.
    fn skip_blank_lines_for_continuation(&mut self) -> Option<usize> {
        let bytes = self.src.as_bytes();
        // Skip trailing whitespace on the current line before checking for the
        // newline. `scan_plain_line` stops at trailing spaces (they are not
        // part of the scalar value), so `self.pos` may be at a space rather
        // than directly on the `\n`.
        while matches!(bytes.get(self.pos), Some(b' ' | b'\t')) {
            self.pos += 1;
        }
        // Must be on a newline to start.
        if !matches!(bytes.get(self.pos), Some(b'\n' | b'\r')) {
            return None;
        }
        loop {
            self.consume_newline(bytes);
            // Now at start of a line. Count indent.
            let line_start = self.pos;
            while let Some(b' ' | b'\t') = bytes.get(self.pos).copied() {
                self.pos += 1;
            }
            // Blank line?
            match bytes.get(self.pos) {
                Some(b'\n' | b'\r') => continue, // blank, skip
                None => return None,             // trailing whitespace at EOF
                _ => return Some(self.pos - line_start),
            }
        }
    }

    /// Assuming `self.pos` is at the first non-whitespace byte of a line,
    /// return true if that line is not a valid plain-scalar continuation:
    /// it starts with a block indicator or contains a `key: ` pattern.
    ///
    /// `break_on_dash_question` gates the `- `/`? ` sibling check: it is
    /// only a structural break when the line is at the container's indent.
    /// A deeper `- ` is folded scalar text (see `read_plain_with`).
    fn line_starts_with_block_indicator_or_key(
        &self,
        stop: PlainStop,
        break_on_dash_question: bool,
    ) -> bool {
        let bytes = self.src.as_bytes();
        let b = match bytes.get(self.pos) {
            Some(&b) => b,
            None => return false,
        };
        // Block sequence `-`, explicit key `?`.
        if break_on_dash_question
            && matches!(b, b'-' | b'?')
            && matches!(
                bytes.get(self.pos + 1),
                None | Some(b' ' | b'\t' | b'\n' | b'\r')
            )
        {
            return true;
        }
        // Document markers at column 0 only — but we're past the line's
        // indent here; the caller already skipped it. A non-zero column
        // can't be `---`/`...` as framing. If column is 0 AND the next
        // three bytes are `---` or `...` followed by boundary, stop.
        if self.column() == 0
            && (bytes.get(self.pos..self.pos + 3) == Some(b"---")
                || bytes.get(self.pos..self.pos + 3) == Some(b"..."))
            && matches!(
                bytes.get(self.pos + 3),
                None | Some(b' ' | b'\t' | b'\n' | b'\r')
            )
        {
            return true;
        }
        // Key pattern: trial-scan with BlockKey stop and see if it ends
        // at a block-colon indicator.
        let mut probe = Lexer {
            src: self.src,
            pos: self.pos,
        };
        let _ = probe.scan_plain_line(PlainStop::BlockKey);
        probe.is_block_colon_indicator()
            && matches!(stop, PlainStop::BlockValue | PlainStop::BlockKey)
    }

    /// Consume an anchor (`&name`) or alias (`*name`) token and return its
    /// slice including the leading sigil. Name characters are any
    /// non-whitespace, non-flow-indicator bytes; an empty name is an error.
    pub fn bump_anchor(&mut self) -> Result<&'a str, LexError> {
        self.bump_named(b'&', /*is_anchor=*/ true)
    }

    pub fn bump_alias(&mut self) -> Result<&'a str, LexError> {
        self.bump_named(b'*', /*is_anchor=*/ false)
    }

    fn bump_named(&mut self, sigil: u8, is_anchor: bool) -> Result<&'a str, LexError> {
        let start = self.pos;
        debug_assert_eq!(self.peek_byte(), Some(sigil));
        self.pos += 1;
        let name_start = self.pos;
        while let Some(&b) = self.src.as_bytes().get(self.pos) {
            match b {
                b' ' | b'\t' | b'\n' | b'\r' | b',' | b'[' | b']' | b'{' | b'}' => break,
                _ => self.pos += 1,
            }
        }
        if self.pos == name_start {
            self.pos = start;
            return Err(if is_anchor {
                LexError::EmptyAnchorName { start }
            } else {
                LexError::EmptyAliasName { start }
            });
        }
        Ok(&self.src[start..self.pos])
    }

    /// Consume a tag token starting at `!`. Forms accepted:
    /// - `!`                  — non-specific tag
    /// - `!<URI>`             — verbatim tag
    /// - `!!suffix`           — secondary handle shorthand
    /// - `!handle!suffix`     — named handle shorthand
    /// - `!suffix`            — primary handle shorthand (a.k.a. local tag)
    ///
    /// Stops at whitespace, flow indicator, or end-of-line. Suffix chars
    /// are any non-whitespace, non-flow-indicator byte (syntactic only —
    /// no %-escape or URI validation).
    pub fn bump_tag(&mut self) -> Result<&'a str, LexError> {
        let start = self.pos;
        debug_assert_eq!(self.peek_byte(), Some(b'!'));
        self.pos += 1;
        let bytes = self.src.as_bytes();

        // Verbatim `!<...>`
        if self.peek_byte() == Some(b'<') {
            self.pos += 1;
            loop {
                match bytes.get(self.pos) {
                    None | Some(b'\n' | b'\r') => {
                        self.pos = start;
                        return Err(LexError::UnterminatedVerbatimTag { start });
                    }
                    Some(b'>') => {
                        self.pos += 1;
                        return Ok(&self.src[start..self.pos]);
                    }
                    Some(_) => self.pos += 1,
                }
            }
        }

        // Scan name chars; possibly a second `!` for a named handle.
        let mut saw_second_bang = false;
        while let Some(&b) = bytes.get(self.pos) {
            match b {
                b' ' | b'\t' | b'\n' | b'\r' | b',' | b'[' | b']' | b'{' | b'}' => break,
                b'!' if !saw_second_bang => {
                    saw_second_bang = true;
                    self.pos += 1;
                }
                b'!' => break, // a third `!` would be malformed; stop lexing
                _ => self.pos += 1,
            }
        }
        // Bare `!` is the non-specific tag; that's fine.
        Ok(&self.src[start..self.pos])
    }

    /// Consume a block scalar (`|` literal or `>` folded) starting at
    /// `pos`. `parent_indent` is the indent of the node whose value this
    /// is; content lines must be indented strictly greater than that.
    ///
    /// Returns the full slice from the `|`/`>` through the last byte of
    /// the scalar (which may or may not be a newline). The returned
    /// token kind is `LITERAL_SCALAR` or `FOLDED_SCALAR`.
    /// `parent_indent` is the indentation of the node this scalar is the
    /// value of, as a *signed* column: a block scalar at the document root
    /// has no enclosing node, modeled as column −1 (so content at column 0
    /// is valid). A block scalar that is a mapping value or sequence entry
    /// passes the key/dash column, so its content must be indented strictly
    /// more than that column and a same-column sibling (`next: ...`) ends it.
    pub fn bump_block_scalar(
        &mut self,
        parent_indent: isize,
    ) -> Result<(SyntaxKind, &'a str), LexError> {
        let start = self.pos;
        let bytes = self.src.as_bytes();
        let sigil = bytes[self.pos];
        debug_assert!(sigil == b'|' || sigil == b'>');
        self.pos += 1;

        // Header: optional indent digit + optional chomp, in either order.
        let mut explicit_indent: Option<usize> = None;
        let mut chomp_seen = false;
        for _ in 0..2 {
            match bytes.get(self.pos) {
                Some(&d @ b'1'..=b'9') if explicit_indent.is_none() => {
                    explicit_indent = Some((d - b'0') as usize);
                    self.pos += 1;
                }
                Some(b'+' | b'-') if !chomp_seen => {
                    chomp_seen = true;
                    self.pos += 1;
                }
                _ => break,
            }
        }

        // Optional trailing whitespace + comment on the header line.
        while matches!(bytes.get(self.pos), Some(b' ' | b'\t')) {
            self.pos += 1;
        }
        if bytes.get(self.pos) == Some(&b'#') {
            while let Some(&b) = bytes.get(self.pos) {
                if b == b'\n' || b == b'\r' {
                    break;
                }
                self.pos += 1;
            }
        }

        // Header must be terminated by newline or EOF.
        match bytes.get(self.pos) {
            None => {
                // No body at all; that's legal — empty scalar.
                let kind = if sigil == b'|' {
                    SyntaxKind::LITERAL_SCALAR
                } else {
                    SyntaxKind::FOLDED_SCALAR
                };
                return Ok((kind, &self.src[start..self.pos]));
            }
            Some(b'\n' | b'\r') => {}
            Some(_) => {
                self.pos = start;
                return Err(LexError::MalformedBlockScalarHeader { start });
            }
        }

        // Body scan. `last_included_pos` is the end of the last byte that
        // belongs to the scalar. Each iteration handles one line *including*
        // its terminating newline, so a trailing run of blank lines is fully
        // captured (significant for keep-chomp) without depending on a
        // following content line to pull the newline in.
        //
        // A line belongs to the scalar if it is:
        //   - blank (whitespace only, with indent <= content indent), OR
        //   - a content line with indent >= the content indent (auto-detected
        //     from the first content line, or fixed by an explicit digit).
        // The first non-blank line whose indent is too small ends the block;
        // any blank lines already consumed before it stay in the token (the
        // decoder chomps them per the chomping indicator).
        // An explicit indent digit `n` sets the content indent to
        // parent_indent + n (clamped at 0 for a root parent at column −1).
        let effective_indent = explicit_indent.map(|n| (parent_indent + n as isize).max(0) as usize);
        let mut detected_indent = effective_indent;
        let mut last_included_pos = self.pos;
        // Consume the header's terminating newline before the first line.
        self.consume_newline(bytes);

        loop {
            let line_start = self.pos;
            // Count indent.
            while matches!(bytes.get(self.pos), Some(b' ')) {
                self.pos += 1;
            }
            let indent = self.pos - line_start;

            // Is the line blank (whitespace only, up to a newline or EOF)?
            //
            // A whitespace-only line whose indent exceeds the block content
            // indent is NOT blank: the spaces past the indent are significant
            // content (e.g. a literal block whose last line is a lone space).
            // Such a line is treated like any other content line below.
            let is_blank = matches!(bytes.get(self.pos), None | Some(b'\n' | b'\r'))
                && match detected_indent {
                    Some(n) => indent <= n,
                    None => true,
                };
            if is_blank {
                if bytes.get(self.pos).is_none() {
                    // EOF on a blank line: the spaces are not content (the
                    // line has no terminator), so stop before them.
                    last_included_pos = line_start;
                    break;
                }
                // Blank line: include it through its terminating newline.
                self.consume_newline(bytes);
                last_included_pos = self.pos;
                continue;
            }

            // A column-0 document marker (`---` or `...` followed by a space,
            // tab, newline, or EOF) ends the block scalar — it frames the
            // document, it is never content. This only bites a root block
            // scalar, where column-0 content is otherwise valid.
            if indent == 0
                && matches!(bytes.get(line_start..line_start + 3), Some(b"---" | b"..."))
                && matches!(
                    bytes.get(line_start + 3),
                    None | Some(b' ' | b'\t' | b'\n' | b'\r')
                )
            {
                self.pos = line_start;
                break;
            }

            // Non-blank line. Determine if it belongs to the scalar. The
            // first content line must be indented strictly more than the
            // parent node (root parent is column −1, so column 0 qualifies);
            // it fixes the content indent for every following line.
            let belongs = match detected_indent {
                Some(n) => indent >= n,
                None => (indent as isize) > parent_indent,
            };
            if !belongs {
                // Roll back to line_start (so caller sees this line).
                self.pos = line_start;
                break;
            }
            if detected_indent.is_none() {
                detected_indent = Some(indent);
            }
            // Include through the rest of this line and its terminator.
            while let Some(&b) = bytes.get(self.pos) {
                if b == b'\n' || b == b'\r' {
                    break;
                }
                self.pos += 1;
            }
            if bytes.get(self.pos).is_none() {
                last_included_pos = self.pos;
                break;
            }
            self.consume_newline(bytes);
            last_included_pos = self.pos;
        }

        self.pos = last_included_pos;
        let kind = if sigil == b'|' {
            SyntaxKind::LITERAL_SCALAR
        } else {
            SyntaxKind::FOLDED_SCALAR
        };
        Ok((kind, &self.src[start..self.pos]))
    }

    /// Whether `src[pos]` is a `:` acting as a block mapping indicator.
    pub fn is_block_colon_indicator(&self) -> bool {
        self.peek_byte() == Some(b':')
            && matches!(
                self.byte_at(self.pos + 1),
                None | Some(b' ' | b'\t' | b'\n' | b'\r')
            )
    }

    /// Whether `src[pos]` is a `:` acting as a flow mapping indicator.
    ///
    /// For plain keys the `:` must be followed by whitespace, a flow
    /// terminator, or EOF — otherwise `:` is part of the plain scalar
    /// (e.g. `http://x`). For **quoted** keys (single/double quoted) the
    /// `:` is always a separator in flow context, per the YAML 1.2 spec
    /// flow-key production. JSON-shaped YAML (`{"a":"b"}`, `{"a":[1]}`)
    /// relies on this relaxation.
    pub fn is_flow_colon_indicator(&self) -> bool {
        self.peek_byte() == Some(b':')
            && matches!(
                self.byte_at(self.pos + 1),
                None | Some(b' ' | b'\t' | b'\n' | b'\r' | b',' | b'}' | b']')
            )
    }

    /// Flow-colon indicator following a quoted scalar key: any `:` counts.
    pub fn is_flow_colon_indicator_after_quoted(&self) -> bool {
        self.peek_byte() == Some(b':')
    }
}

/// Classify a YAML 1.1 boolean word. Returns `Some(bool)` for the
/// recognised words, `None` for everything else.
pub fn as_yaml_1_1_bool(s: &str) -> Option<bool> {
    match s {
        "yes" | "Yes" | "YES" | "on" | "On" | "ON" => Some(true),
        "no" | "No" | "NO" | "off" | "Off" | "OFF" => Some(false),
        _ => None,
    }
}

/// True iff `s` is a YAML 1.1 octal literal (`0[0-7]+`).
/// Bare `0` returns false (already 1.2-safe).
pub fn is_yaml_1_1_octal(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.len() < 2 || bytes[0] != b'0' {
        return false;
    }
    bytes[1..].iter().all(|b| matches!(b, b'0'..=b'7'))
}

/// True iff `s` is a YAML 1.1 sexagesimal number (`1:30`, `-12:34:56.78`).
/// Each `:‑`separated segment after the first must be 0–59.
pub fn is_yaml_1_1_sexagesimal(s: &str) -> bool {
    let body = s.strip_prefix('-').unwrap_or(s);
    let (head, tail_ok) = match body.split_once('.') {
        Some((h, t)) => (h, !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit())),
        None => (body, true),
    };
    if !tail_ok {
        return false;
    }
    let mut segments = head.split(':');
    let first = match segments.next() {
        Some(s) if !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) => s,
        _ => return false,
    };
    let _ = first;
    let mut had_colon = false;
    for seg in segments {
        had_colon = true;
        if seg.is_empty() || seg.len() > 2 || !seg.bytes().all(|b| b.is_ascii_digit()) {
            return false;
        }
        let n: u32 = seg.parse().unwrap();
        if n > 59 {
            return false;
        }
    }
    had_colon
}

/// Given the decoded scalar content `s`, return the raw YAML plain-scalar
/// text to emit, or `None` if the value requires quoting.
///
/// Rules derived from the YAML 1.2 spec and the lexer's own scanning logic:
/// - Must not be empty or start/end with whitespace.
/// - First character must not be a reserved indicator or quote character.
/// - `-` or `?` as first character is only safe when not followed by
///   whitespace (otherwise the lexer reads it as DASH/QUESTION).
/// - Must not contain `: ` / `:\t` / `:\n` / `:\r` or end with `:`.
/// - Must not contain ` #` or `\t#` (would start an inline comment).
/// - Must not contain `,`, `}`, or `]` (flow-collection terminators,
///   forbidden in plain scalars even in block context).
/// - Must not be type-inferred as non-string by the YAML 1.2 Core schema
///   (null, bool, int, float) or misinterpreted by a YAML 1.1 parser
///   (bool, octal, sexagesimal).
pub fn as_plain_scalar(s: &str) -> Option<&str> {
    if s.is_empty() {
        return None;
    }
    let bytes = s.as_bytes();
    if matches!(bytes[0], b' ' | b'\t') {
        return None;
    }
    if matches!(bytes[bytes.len() - 1], b' ' | b'\t') {
        return None;
    }
    match bytes[0] {
        b'\'' | b'"' | b'|' | b'>' | b'&' | b'*' | b'!' | b'{' | b'}' | b'[' | b']' | b','
        | b'@' | b'`' | b'#' => return None,
        b'-' | b'?' if bytes.len() == 1 || matches!(bytes[1], b' ' | b'\t' | b'\n' | b'\r') => {
            return None;
        }
        b':' => return None,
        _ => {}
    }
    // Reject anything the YAML 1.2 Core schema would type-infer as non-string
    // (null, bool, int, float) or that a YAML 1.1 parser would misinterpret.
    if !matches!(infer_plain(s), crate::ast::Value::String(_)) {
        return None;
    }
    if as_yaml_1_1_bool(s).is_some() || is_yaml_1_1_octal(s) || is_yaml_1_1_sexagesimal(s) {
        return None;
    }
    for i in 0..bytes.len() {
        match bytes[i] {
            b',' | b'}' | b']' => return None,
            b':' if matches!(bytes.get(i + 1), None | Some(b' ' | b'\t' | b'\n' | b'\r')) => {
                return None;
            }
            b'#' if i > 0 && matches!(bytes[i - 1], b' ' | b'\t') => return None,
            _ => {}
        }
    }
    Some(s)
}

/// Given the decoded scalar content `s`, return a single-quoted YAML scalar
/// (including the surrounding `'`), or `None` if single-quoting is impossible.
///
/// Literal `'` characters are escaped as `''`. The only values that cannot
/// be represented are those containing control characters (U+0000–U+001F,
/// U+007F), which have no escape mechanism in single-quoted scalars.
pub fn as_single_quoted(s: &str) -> Option<String> {
    if s.bytes().any(|b| b < 0x20 || b == 0x7F) {
        return None;
    }
    let escaped = s.replace('\'', "''");
    Some(format!("'{escaped}'"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lex(src: &str) -> Lexer<'_> {
        Lexer::new(src)
    }

    fn bump<'a>(l: &mut Lexer<'a>) -> (SyntaxKind, &'a str) {
        l.bump_simple()
    }

    #[test]
    fn empty_is_eof() {
        let l = lex("");
        assert!(l.at_eof());
        assert_eq!(l.peek(), None);
    }

    #[test]
    fn whitespace_spaces_and_tabs_combined_no_newline() {
        let mut l = lex("  \t  \nX");
        let (k, t) = bump(&mut l);
        assert_eq!(k, SyntaxKind::WHITESPACE);
        assert_eq!(t, "  \t  ");
        assert_eq!(l.pos(), 5);
    }

    #[test]
    fn newline_lf_crlf_cr() {
        let mut l = lex("\n\r\n\rX");
        assert_eq!(bump(&mut l), (SyntaxKind::NEWLINE, "\n"));
        assert_eq!(bump(&mut l), (SyntaxKind::NEWLINE, "\r\n"));
        assert_eq!(bump(&mut l), (SyntaxKind::NEWLINE, "\r"));
        assert_eq!(l.peek(), Some(LexHint::PlainStart));
    }

    #[test]
    fn comment_at_line_start() {
        let mut l = lex("# hello\n");
        let (k, t) = bump(&mut l);
        assert_eq!(k, SyntaxKind::COMMENT);
        assert_eq!(t, "# hello");
        // newline is a separate token
        assert_eq!(bump(&mut l), (SyntaxKind::NEWLINE, "\n"));
    }

    #[test]
    fn comment_after_space_is_comment() {
        let mut l = lex("x #c");
        // skip 'x' (plain start); we don't scan plain yet, but we can
        // inspect by asking the byte.
        assert_eq!(l.peek(), Some(LexHint::PlainStart));
        l.pos = 1; // pretend the parser consumed "x"
        assert_eq!(bump(&mut l), (SyntaxKind::WHITESPACE, " "));
        assert_eq!(bump(&mut l), (SyntaxKind::COMMENT, "#c"));
    }

    #[test]
    fn hash_not_after_whitespace_is_plain() {
        // "a#b": the '#' is not after whitespace → PlainStart (entire run
        // a#b scanned as plain later)
        let mut l = lex("a#b");
        l.pos = 1; // parser positioned at '#'
        assert_eq!(l.peek(), Some(LexHint::PlainStart));
    }

    #[test]
    fn bom_bytes_classified() {
        let l = lex("\u{FEFF}x");
        assert_eq!(l.peek(), Some(LexHint::Token(SyntaxKind::BOM)));
    }

    #[test]
    fn bom_consumed_three_bytes() {
        let mut l = lex("\u{FEFF}x");
        assert_eq!(bump(&mut l), (SyntaxKind::BOM, "\u{FEFF}"));
        assert_eq!(l.pos(), 3);
    }

    #[test]
    fn colon_and_punctuation() {
        // A `:` followed by whitespace is the COLON indicator.
        let mut l = lex(": ,{}[]");
        for expected in [
            SyntaxKind::COLON,
            SyntaxKind::WHITESPACE,
            SyntaxKind::COMMA,
            SyntaxKind::L_BRACE,
            SyntaxKind::R_BRACE,
            SyntaxKind::L_BRACKET,
            SyntaxKind::R_BRACKET,
        ] {
            let (k, _) = bump(&mut l);
            assert_eq!(k, expected);
        }
    }

    #[test]
    fn colon_indicator_vs_plain() {
        // `:` followed by whitespace/newline/EOF is the COLON indicator.
        let l = lex(": v");
        assert_eq!(l.peek(), Some(LexHint::Token(SyntaxKind::COLON)));
        let l = lex(":\n");
        assert_eq!(l.peek(), Some(LexHint::Token(SyntaxKind::COLON)));
        let l = lex(":");
        assert_eq!(l.peek(), Some(LexHint::Token(SyntaxKind::COLON)));
        // `:` followed by a non-space (`::`, `:foo`) is a plain scalar.
        let l = lex("::");
        assert_eq!(l.peek(), Some(LexHint::PlainStart));
        let l = lex(":foo");
        assert_eq!(l.peek(), Some(LexHint::PlainStart));
    }

    #[test]
    fn dash_indicator_vs_plain() {
        let l = lex("- x");
        assert_eq!(l.peek(), Some(LexHint::Token(SyntaxKind::DASH)));

        let l = lex("-x");
        assert_eq!(l.peek(), Some(LexHint::PlainStart));

        let l = lex("-");
        assert_eq!(l.peek(), Some(LexHint::Token(SyntaxKind::DASH)));

        let l = lex("-\n");
        assert_eq!(l.peek(), Some(LexHint::Token(SyntaxKind::DASH)));
    }

    #[test]
    fn question_indicator_classified() {
        let l = lex("? key");
        assert_eq!(l.peek(), Some(LexHint::Token(SyntaxKind::QUESTION)));
    }

    #[test]
    fn question_not_indicator_is_plain() {
        let l = lex("?foo");
        assert_eq!(l.peek(), Some(LexHint::PlainStart));
    }

    #[test]
    fn percent_at_column_0_is_directive() {
        let l = lex("%YAML 1.2");
        assert_eq!(l.peek(), Some(LexHint::Token(SyntaxKind::DIRECTIVE)));
    }

    #[test]
    fn percent_not_at_column_0_is_plain() {
        // Hypothetical: after some content on a line, `%` is plain.
        let mut l = lex("a %x");
        l.pos = 2;
        assert_eq!(l.peek(), Some(LexHint::PlainStart));
    }

    #[test]
    fn bump_directive_consumes_to_end_of_line() {
        let mut l = lex("%YAML 1.2\n---");
        let (k, t) = l.bump_simple();
        assert_eq!(k, SyntaxKind::DIRECTIVE);
        assert_eq!(t, "%YAML 1.2");
    }

    #[test]
    fn block_scalar_sigils_classified() {
        assert_eq!(
            lex("| x").peek(),
            Some(LexHint::Token(SyntaxKind::LITERAL_SCALAR))
        );
        assert_eq!(
            lex("> x").peek(),
            Some(LexHint::Token(SyntaxKind::FOLDED_SCALAR))
        );
    }

    #[test]
    fn bump_block_scalar_basic_literal() {
        let mut l = lex("|\n  hello\n");
        let (k, t) = l.bump_block_scalar(0).unwrap();
        assert_eq!(k, SyntaxKind::LITERAL_SCALAR);
        // Includes the final newline because loop iter consumed it before
        // detecting EOF.
        assert_eq!(t, "|\n  hello\n");
    }

    #[test]
    fn bump_block_scalar_chomp_keep() {
        let mut l = lex("|+\n  a\n\n");
        let (_, t) = l.bump_block_scalar(0).unwrap();
        assert_eq!(t, "|+\n  a\n\n");
    }

    #[test]
    fn bump_block_scalar_terminated_by_lower_indent() {
        let mut l = lex("|\n  a\n  b\nnext\n");
        let (_, t) = l.bump_block_scalar(0).unwrap();
        // The last content line's terminating newline belongs to the scalar
        // (it is the line break of "b"); the lexer stops at the start of the
        // dedented "next" line.
        assert_eq!(t, "|\n  a\n  b\n");
        assert_eq!(l.peek_byte(), Some(b'n'));
    }

    #[test]
    fn anchor_alias_tag_classified() {
        assert_eq!(
            lex("&anchor").peek(),
            Some(LexHint::Token(SyntaxKind::ANCHOR))
        );
        assert_eq!(
            lex("*alias").peek(),
            Some(LexHint::Token(SyntaxKind::ALIAS))
        );
        assert_eq!(lex("!tag").peek(), Some(LexHint::Token(SyntaxKind::TAG)));
    }

    #[test]
    fn bump_anchor_consumes_name() {
        let mut l = lex("&abc rest");
        let s = l.bump_anchor().unwrap();
        assert_eq!(s, "&abc");
        assert_eq!(l.peek_byte(), Some(b' '));
    }

    #[test]
    fn bump_alias_stops_at_flow_indicator() {
        let mut l = lex("*a,b");
        let s = l.bump_alias().unwrap();
        assert_eq!(s, "*a");
        assert_eq!(l.peek_byte(), Some(b','));
    }

    #[test]
    fn bump_anchor_empty_is_error() {
        let mut l = lex("& ");
        assert_eq!(l.bump_anchor(), Err(LexError::EmptyAnchorName { start: 0 }));
    }

    #[test]
    fn bump_tag_bare() {
        let mut l = lex("! x");
        let s = l.bump_tag().unwrap();
        assert_eq!(s, "!");
    }

    #[test]
    fn bump_tag_secondary_shorthand() {
        let mut l = lex("!!str x");
        let s = l.bump_tag().unwrap();
        assert_eq!(s, "!!str");
    }

    #[test]
    fn bump_tag_named_handle() {
        let mut l = lex("!e!foo x");
        let s = l.bump_tag().unwrap();
        assert_eq!(s, "!e!foo");
    }

    #[test]
    fn bump_tag_local() {
        let mut l = lex("!local x");
        let s = l.bump_tag().unwrap();
        assert_eq!(s, "!local");
    }

    #[test]
    fn bump_tag_verbatim() {
        let mut l = lex("!<tag:example.com,2024:foo> x");
        let s = l.bump_tag().unwrap();
        assert_eq!(s, "!<tag:example.com,2024:foo>");
    }

    #[test]
    fn bump_tag_verbatim_unterminated() {
        let mut l = lex("!<oops\n");
        assert_eq!(
            l.bump_tag(),
            Err(LexError::UnterminatedVerbatimTag { start: 0 })
        );
    }

    #[test]
    fn block_colon_indicator_detection() {
        assert!(lex(": x").is_block_colon_indicator());
        assert!(lex(":\n").is_block_colon_indicator());
        assert!(lex(":\t").is_block_colon_indicator());
        assert!(lex(":").is_block_colon_indicator());
        let l = lex(":x");
        assert!(!l.is_block_colon_indicator());
    }

    #[test]
    fn flow_colon_indicator_detection() {
        assert!(lex(": ").is_flow_colon_indicator());
        assert!(lex(":,").is_flow_colon_indicator());
        assert!(lex(":}").is_flow_colon_indicator());
        assert!(lex(":]").is_flow_colon_indicator());
        assert!(!lex(":x").is_flow_colon_indicator());
    }

    #[test]
    fn single_quoted_simple() {
        let mut l = lex("'hello'");
        let (k, t) = l.bump_quoted().unwrap();
        assert_eq!(k, SyntaxKind::SINGLE_QUOTED_SCALAR);
        assert_eq!(t, "'hello'");
        assert!(l.at_eof());
    }

    #[test]
    fn single_quoted_escape() {
        let mut l = lex("'it''s'");
        let (_, t) = l.bump_quoted().unwrap();
        assert_eq!(t, "'it''s'");
        assert!(l.at_eof());
    }

    #[test]
    fn single_quoted_multiline_ok() {
        let mut l = lex("'oops\nmore'");
        let (k, t) = l.bump_quoted().unwrap();
        assert_eq!(k, SyntaxKind::SINGLE_QUOTED_SCALAR);
        assert_eq!(t, "'oops\nmore'");
    }

    #[test]
    fn single_quoted_unterminated_on_eof() {
        let mut l = lex("'oops");
        assert_eq!(
            l.bump_quoted(),
            Err(LexError::UnterminatedSingleQuoted { start: 0 })
        );
    }

    #[test]
    fn double_quoted_simple() {
        let mut l = lex("\"hello\"");
        let (k, t) = l.bump_quoted().unwrap();
        assert_eq!(k, SyntaxKind::DOUBLE_QUOTED_SCALAR);
        assert_eq!(t, "\"hello\"");
    }

    #[test]
    fn double_quoted_escape_does_not_terminate() {
        let mut l = lex(r#""a\"b""#);
        let (_, t) = l.bump_quoted().unwrap();
        assert_eq!(t, r#""a\"b""#);
    }

    #[test]
    fn double_quoted_various_escapes_preserved() {
        let mut l = lex(r#""a\\b\nc\tq""#);
        let (_, t) = l.bump_quoted().unwrap();
        assert_eq!(t, r#""a\\b\nc\tq""#);
    }

    #[test]
    fn double_quoted_multiline_ok() {
        let mut l = lex("\"oops\nmore\"");
        let (k, t) = l.bump_quoted().unwrap();
        assert_eq!(k, SyntaxKind::DOUBLE_QUOTED_SCALAR);
        assert_eq!(t, "\"oops\nmore\"");
    }

    #[test]
    fn double_quoted_line_continuation_escape() {
        let mut l = lex("\"a\\\n  b\"");
        let (_, t) = l.bump_quoted().unwrap();
        assert_eq!(t, "\"a\\\n  b\"");
    }

    #[test]
    fn double_quoted_unterminated_on_eof() {
        let mut l = lex("\"oops");
        assert_eq!(
            l.bump_quoted(),
            Err(LexError::UnterminatedDoubleQuoted { start: 0 })
        );
    }

    #[test]
    fn read_plain_block_key_stops_at_colon_space() {
        let mut l = lex("key: value");
        let s = l.read_plain(PlainStop::BlockKey);
        assert_eq!(s, "key");
        assert_eq!(l.pos(), 3);
    }

    #[test]
    fn read_plain_block_key_stops_at_colon_eol() {
        let mut l = lex("key:\n");
        let s = l.read_plain(PlainStop::BlockKey);
        assert_eq!(s, "key");
        assert_eq!(l.pos(), 3);
    }

    #[test]
    fn read_plain_block_key_stops_at_colon_eof() {
        let mut l = lex("key:");
        let s = l.read_plain(PlainStop::BlockKey);
        assert_eq!(s, "key");
    }

    #[test]
    fn read_plain_block_value_stops_at_newline() {
        let mut l = lex("hello world\n");
        let s = l.read_plain(PlainStop::BlockValue);
        assert_eq!(s, "hello world");
        assert_eq!(l.peek_byte(), Some(b'\n'));
    }

    #[test]
    fn read_plain_block_value_stops_at_eof() {
        let mut l = lex("hello");
        let s = l.read_plain(PlainStop::BlockValue);
        assert_eq!(s, "hello");
        assert!(l.at_eof());
    }

    #[test]
    fn read_plain_block_value_stops_at_space_hash_comment() {
        let mut l = lex("hello #comment\n");
        let s = l.read_plain(PlainStop::BlockValue);
        assert_eq!(s, "hello");
        // pos is at the space before '#'
        assert_eq!(l.peek_byte(), Some(b' '));
    }

    #[test]
    fn read_plain_block_value_allows_hash_inside() {
        let mut l = lex("a#b\n");
        let s = l.read_plain(PlainStop::BlockValue);
        assert_eq!(s, "a#b");
    }

    #[test]
    fn read_plain_trailing_spaces_excluded() {
        let mut l = lex("hello   \n");
        let s = l.read_plain(PlainStop::BlockValue);
        assert_eq!(s, "hello");
        assert_eq!(l.peek_byte(), Some(b' '));
    }

    #[test]
    fn read_plain_block_value_stops_at_colon_space() {
        // YAML says a plain scalar in block value still terminates at ": "
        let mut l = lex("a: b\n");
        let s = l.read_plain(PlainStop::BlockValue);
        assert_eq!(s, "a");
        assert_eq!(l.peek_byte(), Some(b':'));
    }

    #[test]
    fn read_plain_flow_value_stops_at_comma() {
        let mut l = lex("x, y");
        let s = l.read_plain(PlainStop::FlowValue);
        assert_eq!(s, "x");
    }

    #[test]
    fn read_plain_flow_value_stops_at_rbrace() {
        let mut l = lex("x}");
        let s = l.read_plain(PlainStop::FlowValue);
        assert_eq!(s, "x");
    }

    #[test]
    fn read_plain_flow_value_stops_at_rbracket() {
        let mut l = lex("x]");
        let s = l.read_plain(PlainStop::FlowValue);
        assert_eq!(s, "x");
    }

    #[test]
    fn read_plain_flow_key_stops_at_colon_flow_indicator() {
        let mut l = lex("k:v");
        // ":" is NOT a flow-colon indicator when followed by arbitrary char
        let s = l.read_plain(PlainStop::FlowKey);
        assert_eq!(s, "k:v");

        let mut l = lex("k: v");
        let s = l.read_plain(PlainStop::FlowKey);
        assert_eq!(s, "k");

        let mut l = lex("k:,");
        let s = l.read_plain(PlainStop::FlowKey);
        assert_eq!(s, "k");

        let mut l = lex("k:}");
        let s = l.read_plain(PlainStop::FlowKey);
        assert_eq!(s, "k");
    }

    #[test]
    fn read_plain_empty_at_terminator() {
        let mut l = lex(":");
        let s = l.read_plain(PlainStop::BlockKey);
        assert_eq!(s, "");
        assert_eq!(l.pos(), 0);
    }

    #[test]
    fn column_tracking() {
        let mut l = lex("a\nbc\ndef");
        assert_eq!(l.column(), 0);
        l.pos = 1;
        assert_eq!(l.column(), 1);
        l.pos = 2;
        assert_eq!(l.column(), 0); // just past \n
        l.pos = 4;
        assert_eq!(l.column(), 2);
        l.pos = 5;
        assert_eq!(l.column(), 0); // just past \n
        l.pos = 8;
        assert_eq!(l.column(), 3);
    }
}
