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
            b':' => Some(LexHint::Token(SyntaxKind::COLON)),
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
                match self.peek_byte() {
                    Some(b'\r') => {
                        self.pos += 1;
                        if self.peek_byte() == Some(b'\n') {
                            self.pos += 1;
                        }
                    }
                    Some(b'\n') => self.pos += 1,
                    _ => unreachable!(),
                }
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

    pub fn read_plain_with(&mut self, stop: PlainStop, min_indent: Option<usize>) -> &'a str {
        let start = self.pos;
        let mut last_non_space_end = self.scan_plain_line(stop);

        // Multi-line continuation is only meaningful in block context.
        let multiline_ok =
            matches!(stop, PlainStop::BlockValue | PlainStop::BlockKey) && min_indent.is_some();
        if multiline_ok {
            let min = min_indent.unwrap();
            loop {
                // Save the state before attempting a continuation.
                let line_end = self.pos;
                // Advance to the first non-blank line.
                let Some(cont_indent) = self.skip_blank_lines_for_continuation() else {
                    // EOF reached; stop.
                    self.pos = line_end;
                    break;
                };
                if cont_indent <= min {
                    self.pos = line_end;
                    break;
                }
                if self.line_starts_with_block_indicator_or_key(stop) {
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
                        _ => {}
                    }
                    self.pos += 1;
                }
                b':' if matches!(stop, PlainStop::BlockKey | PlainStop::BlockValue)
                    && matches!(
                        bytes.get(self.pos + 1),
                        None | Some(b' ' | b'\t' | b'\n' | b'\r')
                    ) =>
                {
                    break;
                }
                b':' if matches!(stop, PlainStop::FlowKey)
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

    /// Advance past one or more newlines (plus any trailing blank lines)
    /// to position `pos` at the first non-whitespace byte of the next
    /// non-blank line, or at EOF. Returns the indent (column) of that
    /// line, or `None` at EOF.
    fn skip_blank_lines_for_continuation(&mut self) -> Option<usize> {
        let bytes = self.src.as_bytes();
        // Must be on a newline to start.
        if !matches!(bytes.get(self.pos), Some(b'\n' | b'\r')) {
            return None;
        }
        loop {
            match bytes.get(self.pos) {
                Some(b'\r') => {
                    self.pos += 1;
                    if bytes.get(self.pos) == Some(&b'\n') {
                        self.pos += 1;
                    }
                }
                Some(b'\n') => self.pos += 1,
                _ => unreachable!(),
            }
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
    fn line_starts_with_block_indicator_or_key(&self, stop: PlainStop) -> bool {
        let bytes = self.src.as_bytes();
        let b = match bytes.get(self.pos) {
            Some(&b) => b,
            None => return false,
        };
        // Block sequence `-`, explicit key `?`.
        if matches!(b, b'-' | b'?')
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
    pub fn bump_block_scalar(
        &mut self,
        parent_indent: usize,
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

        // Body scan. We keep `last_included_pos` as the end of the last
        // byte that belongs to the scalar. Lines are included if:
        //   - blank (all whitespace to newline), OR
        //   - indent > parent_indent (when effective_indent not yet set
        //     by an explicit digit); once effective_indent is known, the
        //     line must have indent >= effective_indent.
        let effective_indent = explicit_indent.map(|n| parent_indent + n);
        let mut detected_indent = effective_indent;
        let mut last_included_pos = self.pos;

        loop {
            // Consume the terminator of the previous line.
            match bytes.get(self.pos) {
                Some(b'\r') => {
                    self.pos += 1;
                    if bytes.get(self.pos) == Some(&b'\n') {
                        self.pos += 1;
                    }
                }
                Some(b'\n') => self.pos += 1,
                None => break,
                _ => unreachable!(),
            }
            let line_start = self.pos;
            // Count indent.
            while matches!(bytes.get(self.pos), Some(b' ')) {
                self.pos += 1;
            }
            let indent = self.pos - line_start;

            // Is the line blank?
            match bytes.get(self.pos) {
                None => {
                    // EOF after whitespace-only line; include whatever
                    // newlines we consumed but not the trailing spaces.
                    last_included_pos = line_start;
                    break;
                }
                Some(b'\n' | b'\r') => {
                    // Blank line — tentatively include newlines up to here.
                    // The trailing spaces on this blank line belong with it.
                    last_included_pos = self.pos;
                    continue;
                }
                _ => {}
            }

            // Non-blank line. Determine if it belongs to the scalar.
            let belongs = match detected_indent {
                Some(n) => indent >= n,
                None => indent > parent_indent,
            };
            if !belongs {
                // Roll back to line_start (so caller sees this line).
                self.pos = line_start;
                break;
            }
            if detected_indent.is_none() {
                detected_indent = Some(indent);
            }
            // Include through the rest of this line.
            while let Some(&b) = bytes.get(self.pos) {
                if b == b'\n' || b == b'\r' {
                    break;
                }
                self.pos += 1;
            }
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
        let mut l = lex(":,{}[]");
        for expected in [
            SyntaxKind::COLON,
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
        // Stops at the newline before "next"; the newline stays as trivia.
        assert_eq!(t, "|\n  a\n  b");
        assert_eq!(l.peek_byte(), Some(b'\n'));
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
