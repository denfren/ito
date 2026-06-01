use rowan::{GreenNodeBuilder, Language};

use crate::error::ParseError;
use crate::lexer::{LexError, LexHint, Lexer, PlainStop};
use crate::syntax::{SyntaxKind, SyntaxNode, SyntaxTree, YamlLang};

/// Strict parse: succeeds only for well-formed input, returning the
/// first error otherwise. Most callers want this.
pub fn parse(source: &str) -> Result<SyntaxTree, ParseError> {
    let (tree, errors) = parse_recover(source);
    match errors.into_iter().next() {
        Some(e) => Err(e),
        None => Ok(tree),
    }
}

/// Error-recovering parse: always returns a tree plus any errors. On the
/// first unexpected token the parser captures the unconsumed remainder
/// in an `ERROR` token and closes the tree, so a linter/formatter can
/// still operate on the well-formed prefix and report the error. The
/// returned tree always round-trips (`emit() == source`).
pub fn parse_recover(source: &str) -> (SyntaxTree, Vec<ParseError>) {
    let mut p = Parser::new(source);
    let mut errors = Vec::new();
    if let Err(e) = p.parse_stream() {
        // Capture every byte past the last committed token as an ERROR
        // token so the tree stays lossless (the lexer may have scanned
        // ahead of what was committed), then close any nodes left open
        // at the point of failure.
        let rest = source[p.committed..].to_string();
        if !rest.is_empty() {
            p.token(SyntaxKind::ERROR, &rest);
        }
        while p.open_nodes > 0 {
            p.finish();
        }
        errors.push(e);
    }
    let green = p.builder.finish();
    // Mutable root: enables in-place edits via splice_children. The
    // edit layer asserts mutability, so making this universal keeps
    // the contract simple.
    let root = SyntaxNode::new_root_mut(green);
    let tree = SyntaxTree::new(root);
    // Lossless contract: every byte of the source lives in some token,
    // so re-emitting the tree must reproduce the input exactly. Cheap
    // insurance against off-by-one slicing bugs in the bump_* helpers.
    debug_assert_eq!(
        tree.emit(),
        source,
        "round-trip invariant violated: parse(s).emit() != s"
    );
    (tree, errors)
}

// Entry-indent ownership invariant (Phase 2)
// -------------------------------------------
// A block entry (BLOCK_MAPPING_ENTRY / BLOCK_SEQUENCE_ENTRY) that begins
// at the start of its line owns its leading indent WHITESPACE as its own
// first child token. An entry that begins at column 0, or mid-line
// (immediately after a parent's `- ` or `key:` prefix), owns no indent
// token. This is enforced uniformly by `eat_leading_whitespace()` at the
// top of every entry constructor, so consumers (indent_of, comment
// placement, fixers) can rely on a single rule: "first child is
// WHITESPACE ⟺ this is a line-start entry that owns its indent."
struct Parser<'a> {
    lex: Lexer<'a>,
    builder: GreenNodeBuilder<'static>,
    flow_depth: u32,
    /// Count of `start_node` calls not yet matched by `finish_node`.
    /// Used by error recovery to close the tree cleanly on early exit.
    open_nodes: usize,
    /// Total byte length committed to the builder so far. Error recovery
    /// captures `source[committed..]` as the ERROR remainder, since the
    /// lexer position may have scanned ahead of the last committed token.
    committed: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScalarTokenKind {
    Plain,
    SingleQuoted,
    DoubleQuoted,
}

impl<'a> Parser<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            lex: Lexer::new(source),
            builder: GreenNodeBuilder::new(),
            flow_depth: 0,
            open_nodes: 0,
            committed: 0,
        }
    }

    fn start(&mut self, k: SyntaxKind) {
        self.open_nodes += 1;
        self.builder.start_node(YamlLang::kind_to_raw(k));
    }

    /// Retroactively open a node at a checkpoint (promotion). Tracked
    /// for error recovery like `start`.
    fn start_at(&mut self, cp: rowan::Checkpoint, k: SyntaxKind) {
        self.open_nodes += 1;
        self.builder.start_node_at(cp, YamlLang::kind_to_raw(k));
    }

    fn finish(&mut self) {
        self.open_nodes = self.open_nodes.saturating_sub(1);
        self.builder.finish_node();
    }

    fn token(&mut self, k: SyntaxKind, text: &str) {
        self.committed += text.len();
        self.builder.token(YamlLang::kind_to_raw(k), text);
    }

    fn peek(&self) -> Option<LexHint> {
        self.lex.peek()
    }

    /// Emit a zero-width NULL_SCALAR node standing in for an implicit
    /// null value (empty mapping value or empty sequence entry). Carries
    /// no tokens, so it does not affect the emitted text.
    fn emit_null(&mut self) {
        self.start(SyntaxKind::NULL_SCALAR);
        self.finish();
    }

    /// Bump a simple (non-plain, non-quoted) token to the builder.
    fn bump_simple(&mut self) {
        let (k, t) = self.lex.bump_simple();
        self.token(k, t);
    }

    /// Bump a quoted scalar as a token, attributing errors to the caller.
    fn bump_quoted(&mut self) -> Result<(), ParseError> {
        match self.lex.bump_quoted() {
            Ok((k, t)) => {
                self.token(k, t);
                Ok(())
            }
            Err(LexError::UnterminatedSingleQuoted { start }) => Err(ParseError::new(
                (start as u32).into(),
                "unterminated single-quoted scalar",
            )),
            Err(LexError::UnterminatedDoubleQuoted { start }) => Err(ParseError::new(
                (start as u32).into(),
                "unterminated double-quoted scalar",
            )),
            Err(_) => unreachable!("bump_quoted only yields quoted-scalar errors"),
        }
    }

    /// Consume trivia (whitespace/newline/comment) tokens greedily.
    fn eat_trivia(&mut self) {
        while let Some(LexHint::Token(k)) = self.lex.peek() {
            if matches!(
                k,
                SyntaxKind::WHITESPACE | SyntaxKind::NEWLINE | SyntaxKind::COMMENT
            ) {
                self.bump_simple();
            } else {
                break;
            }
        }
    }

    /// Consume trivia between block-collection entries: eats all
    /// NEWLINE/COMMENT/WHITESPACE tokens EXCEPT for the final WHITESPACE
    /// that immediately precedes the next non-trivia token (the entry-
    /// leading indent). That last WHITESPACE is left for the entry to
    /// consume so that it ends up inside the entry node.
    ///
    /// Specifically: a WHITESPACE token is eaten only if what follows it
    /// is another trivia token (NEWLINE, COMMENT, or WHITESPACE). If the
    /// next token after the WHITESPACE is non-trivia, the WHITESPACE is
    /// the entry indent and we stop before it.
    fn eat_trivia_except_entry_indent(&mut self) {
        loop {
            match self.lex.peek() {
                Some(LexHint::Token(SyntaxKind::NEWLINE | SyntaxKind::COMMENT)) => {
                    self.bump_simple();
                }
                Some(LexHint::Token(SyntaxKind::WHITESPACE)) => {
                    let after = self.peek_after_token();
                    match after {
                        // Followed by more trivia — eat the whitespace now.
                        Some(LexHint::Token(
                            SyntaxKind::WHITESPACE | SyntaxKind::NEWLINE | SyntaxKind::COMMENT,
                        ))
                        | None => {
                            self.bump_simple();
                        }
                        // Followed by non-trivia — this is the entry-leading
                        // indent; stop here and let the entry consume it.
                        _ => break,
                    }
                }
                _ => break,
            }
        }
    }

    /// Consume a single WHITESPACE token if present (leading indent).
    fn eat_leading_whitespace(&mut self) {
        if matches!(
            self.lex.peek(),
            Some(LexHint::Token(SyntaxKind::WHITESPACE))
        ) {
            self.bump_simple();
        }
    }

    /// Consume trivia on the same line only (stops at NEWLINE without
    /// consuming it).
    fn eat_inline_trivia(&mut self) {
        while let Some(LexHint::Token(k)) = self.lex.peek() {
            if matches!(k, SyntaxKind::WHITESPACE | SyntaxKind::COMMENT) {
                self.bump_simple();
            } else {
                break;
            }
        }
    }

    fn err_here(&self, msg: impl Into<String>) -> ParseError {
        ParseError::new((self.lex.pos() as u32).into(), msg)
    }

    fn err_at(&self, offset: usize, msg: impl Into<String>) -> ParseError {
        ParseError::new((offset as u32).into(), msg)
    }

    fn parse_stream(&mut self) -> Result<(), ParseError> {
        self.start(SyntaxKind::STREAM);

        if self.lex.peek() == Some(LexHint::Token(SyntaxKind::BOM)) {
            self.bump_simple();
        }

        self.eat_trivia();

        // Loop over documents. Each iteration parses optional directives,
        // an optional `---`, a body (possibly empty), and an optional `...`.
        let mut first = true;
        let mut need_directives_end = false;
        while !self.lex.at_eof() {
            // Parse directives (only valid before `---` of the next doc).
            if matches!(self.peek(), Some(LexHint::Token(SyntaxKind::DIRECTIVE))) {
                self.parse_directives()?;
                need_directives_end = true;
            }

            // `---` marker. Required when directives were present, or when
            // this is not the first document (unless the previous ended
            // with `...` already — then it's still required).
            let at_dashes = self.at_document_start_marker();
            if at_dashes {
                self.start(SyntaxKind::DOCUMENT);
                self.bump_directives_end();
                self.eat_trivia();
                self.parse_document_body_if_any()?;
                self.finish(); // DOCUMENT
                need_directives_end = false;
            } else if need_directives_end {
                return Err(self.err_here("expected '---' after directives"));
            } else if !first {
                // No `---` and not first — must be EOF or trailing trivia.
                // Any other content here would be silently discarded; that
                // mangles round-tripping (e.g. ansible-vault bodies whose
                // header parses as a bare plain scalar).
                return Err(
                    self.err_here("unexpected content after document; expected '---' or '...'")
                );
            } else {
                // First doc, bare (no `---`).
                self.start(SyntaxKind::DOCUMENT);
                self.parse_document_body_if_any()?;
                self.finish(); // DOCUMENT
            }

            // Optional `...` end marker.
            self.eat_trivia();
            if self.at_document_end_marker() {
                self.bump_document_end();
                self.eat_trivia();
            }

            first = false;
        }

        self.finish();
        Ok(())
    }

    fn parse_directives(&mut self) -> Result<(), ParseError> {
        self.start(SyntaxKind::DIRECTIVES);
        while matches!(self.peek(), Some(LexHint::Token(SyntaxKind::DIRECTIVE))) {
            self.bump_simple();
            self.eat_trivia();
        }
        self.finish();
        Ok(())
    }

    fn parse_document_body_if_any(&mut self) -> Result<(), ParseError> {
        // A document body may be empty (just `---\n`).
        if self.lex.at_eof() {
            return Ok(());
        }
        // Stop at next document boundary.
        if self.at_document_start_marker() || self.at_document_end_marker() {
            return Ok(());
        }
        let indent = self.lex.column();
        self.parse_block_node(indent, indent)?;
        Ok(())
    }

    fn at_document_start_marker(&self) -> bool {
        self.at_line_start_marker(b"---")
    }

    fn at_document_end_marker(&self) -> bool {
        self.at_line_start_marker(b"...")
    }

    fn at_line_start_marker(&self, needle: &[u8]) -> bool {
        if self.lex.column() != 0 {
            return false;
        }
        let pos = self.lex.pos();
        let bytes = self.lex.source().as_bytes();
        if bytes.get(pos..pos + 3) != Some(needle) {
            return false;
        }
        matches!(
            bytes.get(pos + 3),
            None | Some(b' ' | b'\t' | b'\n' | b'\r')
        )
    }

    fn bump_directives_end(&mut self) {
        let start = self.lex.pos();
        self.lex.set_pos(start + 3);
        let text = &self.lex.source()[start..start + 3];
        self.token(SyntaxKind::DIRECTIVES_END, text);
    }

    fn bump_document_end(&mut self) {
        let start = self.lex.pos();
        self.lex.set_pos(start + 3);
        let text = &self.lex.source()[start..start + 3];
        self.token(SyntaxKind::DOCUMENT_END, text);
    }

    /// Like `parse_block_node`, but with an explicit barrier for
    /// multi-line plain-scalar continuation. `scalar_min_indent` is the
    /// parent container's indent; continuation lines must be **strictly
    /// greater than** this value. Use this when dispatching into the
    /// value of a mapping entry whose value starts on the next line: the
    /// value's own start column is NOT the barrier (YAML 1.2 §7.3.3,
    /// YAML 1.1 §9.1.3).
    fn parse_block_node(
        &mut self,
        indent: usize,
        scalar_min_indent: usize,
    ) -> Result<(), ParseError> {
        self.reject_tab_indent_at_line_start()?;
        // Node properties (anchor/alias/tag) may precede any block node.
        // If the property group is an alias, it IS the node — return.
        if self.at_node_property_start() {
            let was_alias = self.parse_node_properties_with_alias_check()?;
            if was_alias {
                return Ok(());
            }
            self.eat_inline_trivia();
            if self.at_line_end_or_eof() {
                // The node value may live on the next line at indent
                // `>= indent` (e.g. `!tag\napplication:\n  ...` at col 0,
                // or `!tag\n  key: val` nested deeper). Peek past trivia
                // and recurse when content follows. Otherwise treat as
                // a null value decorated by properties.
                if let Some(col) = self.column_past_trivia()
                    && col >= indent
                {
                    self.eat_trivia();
                    if self.at_document_start_marker() || self.at_document_end_marker() {
                        return Ok(());
                    }
                    return self.parse_block_node(col, scalar_min_indent);
                }
                return Ok(());
            }
        }
        // Peek past a leading WHITESPACE (the first entry's indent left by
        // eat_trivia_except_entry_indent) to determine what to dispatch on.
        // For collection/mapping cases the WHITESPACE will be consumed inside
        // the entry node by eat_leading_whitespace(). For other cases (flow
        // nodes, block scalars) it belongs at the current level and is eaten
        // here before delegating.
        let hint = self.peek_past_leading_whitespace();
        match hint {
            None => Ok(()), // empty node after properties
            Some(LexHint::Token(SyntaxKind::L_BRACE)) => {
                self.eat_leading_whitespace();
                self.parse_flow_mapping()
            }
            Some(LexHint::Token(SyntaxKind::L_BRACKET)) => {
                self.eat_leading_whitespace();
                self.parse_flow_sequence()
            }
            Some(LexHint::Token(SyntaxKind::DASH)) => self.parse_block_sequence(indent),
            Some(LexHint::Token(SyntaxKind::QUESTION)) => {
                self.parse_block_mapping_or_scalar_ext(indent, scalar_min_indent)
            }
            Some(LexHint::Token(SyntaxKind::LITERAL_SCALAR | SyntaxKind::FOLDED_SCALAR)) => {
                self.eat_leading_whitespace();
                self.parse_block_scalar(indent)
            }
            Some(LexHint::Token(SyntaxKind::SINGLE_QUOTED_SCALAR))
            | Some(LexHint::Token(SyntaxKind::DOUBLE_QUOTED_SCALAR))
            | Some(LexHint::PlainStart) => {
                self.parse_block_mapping_or_scalar_ext(indent, scalar_min_indent)
            }
            Some(LexHint::Token(k)) => Err(self.err_here(format!("unexpected token {k:?}"))),
        }
    }

    fn at_node_property_start(&self) -> bool {
        matches!(
            self.peek(),
            Some(LexHint::Token(
                SyntaxKind::ANCHOR | SyntaxKind::ALIAS | SyntaxKind::TAG
            ))
        )
    }

    fn at_line_end_or_eof(&self) -> bool {
        matches!(
            self.peek(),
            None | Some(LexHint::Token(SyntaxKind::NEWLINE | SyntaxKind::COMMENT))
        )
    }

    /// Consume a run of node properties. Returns `Ok(true)` if the run
    /// was a bare alias (which IS the node — caller must not try to
    /// consume further content). Returns `Ok(false)` if the run was
    /// anchors/tags only (caller should continue into node dispatch).
    fn parse_node_properties_with_alias_check(&mut self) -> Result<bool, ParseError> {
        // Bare alias: emit ALIAS_NODE and return.
        if matches!(self.peek(), Some(LexHint::Token(SyntaxKind::ALIAS))) {
            self.start(SyntaxKind::ALIAS_NODE);
            self.consume_alias()?;
            self.finish();
            return Ok(true);
        }
        self.start(SyntaxKind::NODE_PROPERTIES);
        loop {
            match self.peek() {
                Some(LexHint::Token(SyntaxKind::ANCHOR)) => self.consume_anchor()?,
                Some(LexHint::Token(SyntaxKind::TAG)) => self.consume_tag()?,
                Some(LexHint::Token(SyntaxKind::ALIAS)) => {
                    return Err(self.err_here("alias cannot follow other node properties"));
                }
                _ => break,
            }
            // If whitespace is next AND another property follows, consume
            // the whitespace inside NODE_PROPERTIES and keep looping.
            // Otherwise stop — the whitespace belongs outside.
            if matches!(self.peek(), Some(LexHint::Token(SyntaxKind::WHITESPACE)))
                && self.next_non_ws_byte_is_property_sigil()
            {
                self.bump_simple();
            } else {
                break;
            }
        }
        self.finish(); // NODE_PROPERTIES
        Ok(false)
    }

    /// Byte-level lookahead: after skipping ASCII space/tab from the
    /// current position, is the next byte `&`, `*`, or `!`?
    fn next_non_ws_byte_is_property_sigil(&self) -> bool {
        let bytes = self.lex.source().as_bytes();
        let mut i = self.lex.pos();
        while let Some(&b) = bytes.get(i) {
            if b == b' ' || b == b'\t' {
                i += 1;
            } else {
                return matches!(b, b'&' | b'*' | b'!');
            }
        }
        false
    }

    fn consume_anchor(&mut self) -> Result<(), ParseError> {
        match self.lex.bump_anchor() {
            Ok(t) => {
                self.token(SyntaxKind::ANCHOR, t);
                Ok(())
            }
            Err(LexError::EmptyAnchorName { start }) => {
                Err(self.err_at(start, "anchor name cannot be empty"))
            }
            Err(_) => unreachable!(),
        }
    }

    fn consume_alias(&mut self) -> Result<(), ParseError> {
        match self.lex.bump_alias() {
            Ok(t) => {
                self.token(SyntaxKind::ALIAS, t);
                Ok(())
            }
            Err(LexError::EmptyAliasName { start }) => {
                Err(self.err_at(start, "alias name cannot be empty"))
            }
            Err(_) => unreachable!(),
        }
    }

    fn consume_tag(&mut self) -> Result<(), ParseError> {
        match self.lex.bump_tag() {
            Ok(t) => {
                self.token(SyntaxKind::TAG, t);
                Ok(())
            }
            Err(LexError::UnterminatedVerbatimTag { start }) => {
                Err(self.err_at(start, "unterminated verbatim tag"))
            }
            Err(_) => unreachable!(),
        }
    }

    /// Read a scalar (could be the key of a mapping OR a lone scalar
    /// document). Uses rowan's checkpoint to retroactively wrap in
    /// `scalar_min_indent` is the indent barrier for multi-line plain
    /// scalar continuation when this node turns out to be a lone scalar.
    /// It's typically the parent container's indent (the dash column for
    /// a sequence entry, or the key's indent for a mapping value).
    fn parse_block_mapping_or_scalar_ext(
        &mut self,
        indent: usize,
        scalar_min_indent: usize,
    ) -> Result<(), ParseError> {
        // Explicit-key form: `? key` at this indent introduces a block
        // mapping whose first entry uses the explicit syntax.
        // peek_past_leading_whitespace already dispatched us here knowing
        // that (after any leading WHITESPACE) a QUESTION follows; but we
        // must still check explicitly since this function is also called
        // directly without a leading WHITESPACE.
        if matches!(
            self.peek_past_leading_whitespace(),
            Some(LexHint::Token(SyntaxKind::QUESTION))
        ) {
            self.start(SyntaxKind::BLOCK_MAPPING);
            self.parse_explicit_block_mapping_entry(indent)?;
            self.parse_block_mapping_tail(indent)?;
            self.finish();
            return Ok(());
        }
        let cp = self.builder.checkpoint();
        // Emit any leading entry-indent WHITESPACE inside the checkpoint so
        // it lands inside BLOCK_MAPPING_ENTRY after checkpoint promotion.
        self.eat_leading_whitespace();
        let scalar_start = self.lex.pos();
        let kind_and_span = self.scan_scalar_first_line(PlainStop::BlockKey)?;
        // Check for mapping-key indicator without consuming.
        let save = self.lex.pos();
        self.eat_inline_trivia_peek_only();
        let is_mapping = self.lex.is_block_colon_indicator();
        self.lex.set_pos(save);

        if is_mapping {
            // Commit scalar as a single-line key.
            self.commit_scalar(kind_and_span);
            self.eat_inline_trivia();
            // Promote into BLOCK_MAPPING + BLOCK_MAPPING_ENTRY.
            self.start_at(cp, SyntaxKind::BLOCK_MAPPING);
            self.start_at(cp, SyntaxKind::BLOCK_MAPPING_ENTRY);
            self.finish_block_mapping_entry_body(indent)?;
            self.eat_inline_trivia();
            self.finish(); // BLOCK_MAPPING_ENTRY
            self.parse_block_mapping_tail(indent)?;
            self.finish(); // BLOCK_MAPPING
            Ok(())
        } else {
            // Lone scalar. For plain scalars, extend across continuation
            // lines using `indent` as the min_indent barrier.
            let (scalar_kind, _first_line_end) = kind_and_span;
            match scalar_kind {
                ScalarTokenKind::Plain => {
                    self.lex.set_pos(scalar_start);
                    let text = self
                        .lex
                        .read_plain_with(PlainStop::BlockValue, Some(scalar_min_indent));
                    self.start(SyntaxKind::SCALAR);
                    self.token(SyntaxKind::PLAIN_SCALAR, text);
                    self.finish();
                }
                ScalarTokenKind::SingleQuoted | ScalarTokenKind::DoubleQuoted => {
                    self.commit_scalar(kind_and_span);
                }
            }
            Ok(())
        }
    }

    /// Scan the first line of a scalar without committing to the builder.
    /// Returns the token kind and the ending position.
    fn scan_scalar_first_line(
        &mut self,
        stop: PlainStop,
    ) -> Result<(ScalarTokenKind, usize), ParseError> {
        match self.peek() {
            Some(LexHint::Token(
                SyntaxKind::SINGLE_QUOTED_SCALAR | SyntaxKind::DOUBLE_QUOTED_SCALAR,
            )) => {
                // We need to consume the token into a saved slice; but we
                // can't uncommit. So we do the full scan and commit later.
                let start = self.lex.pos();
                let kind = match self.lex.peek() {
                    Some(LexHint::Token(SyntaxKind::SINGLE_QUOTED_SCALAR)) => {
                        ScalarTokenKind::SingleQuoted
                    }
                    _ => ScalarTokenKind::DoubleQuoted,
                };
                self.lex
                    .bump_quoted()
                    .map_err(|e| self.lex_err_to_parse(e))?;
                Ok((kind, start))
            }
            Some(LexHint::PlainStart) => {
                let start = self.lex.pos();
                self.lex.read_plain_with(stop, None);
                if self.lex.pos() == start {
                    return Err(self.err_at(start, "expected scalar content"));
                }
                Ok((ScalarTokenKind::Plain, start))
            }
            _ => Err(self.err_here("expected scalar")),
        }
    }

    /// Commit the previously-scanned scalar to the builder. `start` is
    /// where scanning began. The lexer must still be positioned past the
    /// end of the token text.
    fn commit_scalar(&mut self, (kind, start): (ScalarTokenKind, usize)) {
        let end = self.lex.pos();
        let text = &self.lex.source()[start..end];
        let tok = match kind {
            ScalarTokenKind::Plain => SyntaxKind::PLAIN_SCALAR,
            ScalarTokenKind::SingleQuoted => SyntaxKind::SINGLE_QUOTED_SCALAR,
            ScalarTokenKind::DoubleQuoted => SyntaxKind::DOUBLE_QUOTED_SCALAR,
        };
        self.start(SyntaxKind::SCALAR);
        self.token(tok, text);
        self.finish();
    }

    fn lex_err_to_parse(&self, e: LexError) -> ParseError {
        match e {
            LexError::UnterminatedSingleQuoted { start } => {
                self.err_at(start, "unterminated single-quoted scalar")
            }
            LexError::UnterminatedDoubleQuoted { start } => {
                self.err_at(start, "unterminated double-quoted scalar")
            }
            LexError::EmptyAnchorName { start } => {
                self.err_at(start, "anchor name cannot be empty")
            }
            LexError::EmptyAliasName { start } => self.err_at(start, "alias name cannot be empty"),
            LexError::UnterminatedVerbatimTag { start } => {
                self.err_at(start, "unterminated verbatim tag")
            }
            LexError::MalformedBlockScalarHeader { start } => {
                self.err_at(start, "malformed block scalar header")
            }
        }
    }

    /// Peek one token ahead past the current token without consuming it.
    fn peek_after_token(&mut self) -> Option<LexHint> {
        let save = self.lex.pos();
        self.lex.bump_simple();
        let h = self.lex.peek();
        self.lex.set_pos(save);
        h
    }

    /// Return the current peek hint, but if it is a WHITESPACE token peek one
    /// token further. Used in parse_block_node to dispatch past a leading
    /// entry-indent that eat_trivia_except_entry_indent() left in place.
    fn peek_past_leading_whitespace(&mut self) -> Option<LexHint> {
        match self.lex.peek() {
            Some(LexHint::Token(SyntaxKind::WHITESPACE)) => self.peek_after_token(),
            other => other,
        }
    }

    /// Step over inline trivia via the raw byte stream without committing
    /// any tokens. Used for lookahead that must be reversible.
    fn eat_inline_trivia_peek_only(&mut self) {
        let bytes = self.lex.source().as_bytes();
        let mut i = self.lex.pos();
        while matches!(bytes.get(i), Some(b' ' | b'\t')) {
            i += 1;
        }
        self.lex.set_pos(i);
    }

    /// At this point the mapping entry is open (BLOCK_MAPPING_ENTRY
    /// started via start_node_at). The key SCALAR is inside it, followed
    /// by any inline trivia. We're now positioned at the COLON.
    /// Consumes the colon, optional value, and any indented block value.
    /// Does NOT close BLOCK_MAPPING_ENTRY.
    fn finish_block_mapping_entry_body(&mut self, indent: usize) -> Result<(), ParseError> {
        // Consume COLON.
        self.bump_simple();
        self.eat_inline_trivia();

        // Node properties on the value.
        if self.at_node_property_start() {
            let was_alias = self.parse_node_properties_with_alias_check()?;
            if was_alias {
                return Ok(());
            }
            self.eat_inline_trivia();
        }

        match self.peek() {
            None => {
                self.emit_null(); // empty value at EOF
                Ok(())
            }
            Some(LexHint::Token(SyntaxKind::NEWLINE)) => {
                // Could be: empty value, nested block (greater indent), or
                // block sequence at the same indent (YAML allows that).
                let next_col = self.column_past_trivia();
                match next_col {
                    Some(col) if col > indent => {
                        self.eat_trivia_except_entry_indent();
                        // Parent is the mapping at `indent`; plain-scalar
                        // continuation lines are bounded by that indent,
                        // not by the value's starting column `col`.
                        self.parse_block_node(col, indent)?;
                        Ok(())
                    }
                    Some(col) if col == indent && self.peek_past_trivia_is_dash() => {
                        // Block sequence value at same indent as the key.
                        self.eat_trivia_except_entry_indent();
                        self.parse_block_sequence(col)?;
                        Ok(())
                    }
                    _ => {
                        // Empty value; node properties (if any) decorate
                        // this null node.
                        self.emit_null();
                        Ok(())
                    }
                }
            }
            Some(LexHint::Token(SyntaxKind::L_BRACE)) => self.parse_flow_mapping(),
            Some(LexHint::Token(SyntaxKind::L_BRACKET)) => self.parse_flow_sequence(),
            Some(LexHint::Token(SyntaxKind::DASH)) => {
                // Compact block sequence on the same line: `a: - 1`.
                // Its indent is the column of the dash.
                let col = self.lex.column();
                self.parse_block_sequence(col)
            }
            Some(LexHint::PlainStart)
            | Some(LexHint::Token(
                SyntaxKind::SINGLE_QUOTED_SCALAR | SyntaxKind::DOUBLE_QUOTED_SCALAR,
            )) => self.parse_scalar_as_node(PlainStop::BlockValue, Some(indent)),
            Some(LexHint::Token(SyntaxKind::LITERAL_SCALAR | SyntaxKind::FOLDED_SCALAR)) => {
                self.parse_block_scalar(indent)
            }
            Some(LexHint::Token(k)) => Err(self.err_here(format!("unexpected token {k:?}"))),
        }
    }

    /// Parse a `? key\n: value` explicit-key mapping entry at `indent`.
    /// Positioned at the leading indent WHITESPACE or the `?` token.
    fn parse_explicit_block_mapping_entry(&mut self, indent: usize) -> Result<(), ParseError> {
        self.start(SyntaxKind::BLOCK_MAPPING_ENTRY);
        // Consume leading indent whitespace inside the entry.
        self.eat_leading_whitespace();
        // Consume `?`.
        self.bump_simple();
        self.eat_inline_trivia();
        // Key: single-line scalar, or newline-then-indented block, or empty.
        match self.peek() {
            None => {}
            Some(LexHint::Token(SyntaxKind::NEWLINE)) => {
                // Empty key on this line; look for nested block key.
                if let Some(col) = self.column_past_trivia()
                    && col > indent
                {
                    self.eat_trivia();
                    self.parse_block_node(col, col)?;
                }
            }
            Some(LexHint::PlainStart)
            | Some(LexHint::Token(
                SyntaxKind::SINGLE_QUOTED_SCALAR | SyntaxKind::DOUBLE_QUOTED_SCALAR,
            )) => {
                self.parse_scalar_as_node(PlainStop::BlockValue, None)?;
            }
            Some(LexHint::Token(SyntaxKind::L_BRACE)) => self.parse_flow_mapping()?,
            Some(LexHint::Token(SyntaxKind::L_BRACKET)) => self.parse_flow_sequence()?,
            Some(LexHint::Token(SyntaxKind::QUESTION)) => {
                // Nested explicit-key mapping as the key (`? ? nested ...`).
                // Recurse as a block node at this column.
                let col = self.lex.column();
                self.parse_block_node(col, col)?;
            }
            Some(LexHint::Token(k)) => {
                return Err(self.err_here(format!("unexpected token {k:?} after '?'")));
            }
        }
        // Find the matching `:` at `indent`.
        self.eat_trivia();
        if !self.lex.at_eof() {
            let col = self.lex.column();
            if col == indent && matches!(self.peek(), Some(LexHint::Token(SyntaxKind::COLON))) {
                self.finish_block_mapping_entry_body(indent)?;
            }
            // If no matching `:`, it's a key-only entry (empty value).
        }
        self.eat_inline_trivia();
        self.finish(); // BLOCK_MAPPING_ENTRY
        Ok(())
    }

    fn parse_block_scalar(&mut self, parent_indent: usize) -> Result<(), ParseError> {
        let (kind, text) = self
            .lex
            .bump_block_scalar(parent_indent)
            .map_err(|e| self.lex_err_to_parse(e))?;
        self.start(SyntaxKind::SCALAR);
        self.token(kind, text);
        self.finish();
        Ok(())
    }

    /// Parse remaining entries of a block mapping at `indent`. The first
    /// entry has already been consumed by the caller. Trivia between
    /// entries is attached at the BLOCK_MAPPING level (siblings of the
    /// entries). Trivia is only consumed once we've committed to another
    /// entry — otherwise it is left for an enclosing producer to attach
    /// at a higher structural level.
    fn parse_block_mapping_tail(&mut self, indent: usize) -> Result<(), ParseError> {
        // Peek past trivia without consuming, so that if we break we
        // don't leave orphaned trivia nested inside BLOCK_MAPPING.
        while let Some(next_col) = self.column_past_trivia() {
            if next_col < indent {
                break;
            }

            // Commit to another entry: now safe to absorb newlines/comments.
            // Leave the indent WHITESPACE for the entry node to consume.
            if next_col > indent {
                return Err(self.err_here("unexpected indentation in block mapping"));
            }
            self.eat_trivia_except_entry_indent();

            // Stop on document boundary markers.
            if self.at_document_start_marker() || self.at_document_end_marker() {
                break;
            }

            // Must be another mapping key (implicit or explicit).
            // We may be sitting on a WHITESPACE token (the indent); peek
            // past it to determine the key type.
            let key_hint = if matches!(
                self.lex.peek(),
                Some(LexHint::Token(SyntaxKind::WHITESPACE))
            ) {
                self.peek_after_token()
            } else {
                self.lex.peek()
            };
            match key_hint {
                Some(LexHint::Token(
                    SyntaxKind::SINGLE_QUOTED_SCALAR | SyntaxKind::DOUBLE_QUOTED_SCALAR,
                ))
                | Some(LexHint::PlainStart) => {}
                Some(LexHint::Token(SyntaxKind::QUESTION)) => {
                    self.reject_tab_indent_at_line_start()?;
                    self.parse_explicit_block_mapping_entry(indent)?;
                    continue;
                }
                _ => break, // not a key; let caller surface the error
            }

            // Reject tab indentation at the start of a continuation line.
            self.reject_tab_indent_at_line_start()?;

            self.start(SyntaxKind::BLOCK_MAPPING_ENTRY);
            // Consume leading indent whitespace inside the entry.
            self.eat_leading_whitespace();
            self.parse_scalar_as_node(PlainStop::BlockKey, None)?;
            self.eat_inline_trivia();
            if !self.lex.is_block_colon_indicator() {
                return Err(self.err_here("expected ':' after mapping key"));
            }
            self.finish_block_mapping_entry_body(indent)?;
            self.eat_inline_trivia();
            self.finish(); // BLOCK_MAPPING_ENTRY
        }
        Ok(())
    }

    // Raw structural lookahead.
    //
    // The following helpers (`column_past_trivia`, `peek_past_trivia_is_dash`,
    // `eat_inline_trivia_peek_only`, `next_non_ws_byte_is_property_sigil`,
    // `reject_tab_indent_at_line_start`) scan `self.lex.source().as_bytes()`
    // directly rather than going through the lexer. This is deliberate, not
    // an oversight: they answer *parser-context* questions — the column of
    // the next significant token, whether a block-sequence dash follows,
    // whether the indent contains a tab — that depend on indentation state
    // the lexer intentionally does not track (it is mode-free; plain-scalar
    // scanning is parser-driven). They are all reversible peeks: they either
    // take `&self` or save/restore `lex.pos()`, and never commit tokens.
    // Pushing them into the lexer would relocate the same byte loops while
    // handing the lexer parser context it deliberately lacks, so they stay
    // here, close to the indentation logic that consumes them.

    /// Return the column of the next non-trivia token, without advancing
    /// the lexer. Returns `None` at EOF.
    fn column_past_trivia(&self) -> Option<usize> {
        let bytes = self.lex.source().as_bytes();
        let mut i = self.lex.pos();
        let mut col = self.lex.column();
        while let Some(&b) = bytes.get(i) {
            match b {
                b' ' | b'\t' => {
                    i += 1;
                    col += 1;
                }
                b'\n' => {
                    i += 1;
                    col = 0;
                }
                b'\r' => {
                    i += 1;
                    if bytes.get(i) == Some(&b'\n') {
                        i += 1;
                    }
                    col = 0;
                }
                b'#' => {
                    // comment to end of line
                    while let Some(&c) = bytes.get(i) {
                        if c == b'\n' || c == b'\r' {
                            break;
                        }
                        i += 1;
                    }
                    // let outer loop handle the newline
                }
                _ => return Some(col),
            }
        }
        None
    }

    /// True iff, after skipping trivia, the next byte is a DASH acting
    /// as a block-sequence indicator.
    fn peek_past_trivia_is_dash(&self) -> bool {
        let bytes = self.lex.source().as_bytes();
        let mut i = self.lex.pos();
        while let Some(&b) = bytes.get(i) {
            match b {
                b' ' | b'\t' | b'\n' | b'\r' => i += 1,
                b'#' => {
                    while let Some(&c) = bytes.get(i) {
                        if c == b'\n' || c == b'\r' {
                            break;
                        }
                        i += 1;
                    }
                }
                b'-' => {
                    return matches!(bytes.get(i + 1), None | Some(b' ' | b'\t' | b'\n' | b'\r'));
                }
                _ => return false,
            }
        }
        false
    }

    fn parse_block_sequence(&mut self, indent: usize) -> Result<(), ParseError> {
        self.reject_tab_indent_at_line_start()?;
        self.start(SyntaxKind::BLOCK_SEQUENCE);

        let mut first = true;
        loop {
            if !first {
                // Peek past trivia without consuming. If the next
                // significant content is not a continuation of this
                // sequence, leave the trivia for a higher-level producer
                // to attach (keeps trivia out of the previous entry).
                let next_col = match self.column_past_trivia() {
                    Some(col) => col,
                    None => break,
                };
                if next_col < indent {
                    break;
                }
                // For an indentless sequence (dash column == enclosing
                // key column), a sibling key of the sequence's owner sits
                // at the same column as the dashes. Don't absorb the
                // inter-entry trivia unless the next significant token is
                // actually a DASH — otherwise it's a sibling of the
                // enclosing mapping entry, and the trivia belongs at that
                // higher level.
                if !self.peek_past_trivia_is_dash() {
                    break;
                }
                if next_col > indent {
                    return Err(self.err_here("unexpected indentation in block sequence"));
                }
                self.eat_trivia_except_entry_indent();
            }
            first = false;

            // After eat_trivia_except_entry_indent() we may be at a leading WHITESPACE
            // token before the DASH (indent ws is now inside the entry).
            // Peek past any whitespace to confirm we have a DASH.
            let at_dash = match self.peek() {
                Some(LexHint::Token(SyntaxKind::DASH)) => true,
                Some(LexHint::Token(SyntaxKind::WHITESPACE)) => {
                    matches!(
                        self.peek_after_token(),
                        Some(LexHint::Token(SyntaxKind::DASH))
                    )
                }
                _ => false,
            };
            if !at_dash {
                break;
            }

            self.reject_tab_indent_at_line_start()?;
            self.parse_block_sequence_entry(indent)?;
        }

        self.finish();
        Ok(())
    }

    fn parse_block_sequence_entry(&mut self, indent: usize) -> Result<(), ParseError> {
        self.start(SyntaxKind::BLOCK_SEQUENCE_ENTRY);

        // Consume leading indent whitespace (absent for the first entry
        // since the parent already consumed it before opening the sequence).
        self.eat_leading_whitespace();

        // Consume DASH.
        self.bump_simple();

        // Inline trivia after the dash (usually a single space).
        self.eat_inline_trivia();

        // Node properties on the entry's value.
        if self.at_node_property_start() {
            let was_alias = self.parse_node_properties_with_alias_check()?;
            if was_alias {
                self.eat_inline_trivia();
                self.finish();
                return Ok(());
            }
            self.eat_inline_trivia();
        }

        match self.peek() {
            None => {
                // `-` at EOF is an empty entry.
                self.emit_null();
                self.finish();
                return Ok(());
            }
            Some(LexHint::Token(SyntaxKind::NEWLINE)) => {
                // Empty inline; look for nested block on next line at
                // greater indent.
                let next_col = self.column_past_trivia();
                match next_col {
                    Some(col) if col > indent => {
                        self.eat_trivia_except_entry_indent();
                        self.parse_block_node(col, col)?;
                    }
                    _ => self.emit_null(), // empty entry, trivia stays for the sequence
                }
            }
            Some(LexHint::Token(SyntaxKind::DASH)) => {
                // Nested sequence, `- - x`. The nested sequence's indent
                // is the column of the inner dash.
                let col = self.lex.column();
                self.parse_block_sequence(col)?;
            }
            Some(LexHint::Token(SyntaxKind::L_BRACE)) => self.parse_flow_mapping()?,
            Some(LexHint::Token(SyntaxKind::L_BRACKET)) => self.parse_flow_sequence()?,
            Some(LexHint::Token(SyntaxKind::LITERAL_SCALAR | SyntaxKind::FOLDED_SCALAR)) => {
                self.parse_block_scalar(indent)?;
            }
            Some(LexHint::PlainStart)
            | Some(LexHint::Token(
                SyntaxKind::SINGLE_QUOTED_SCALAR | SyntaxKind::DOUBLE_QUOTED_SCALAR,
            )) => {
                // Could be a lone scalar entry OR a mapping starting with
                // an implicit key on the same line (`- a: 1`). Use the
                // same checkpoint-promote machinery. For a lone scalar,
                // multi-line continuation is bounded by the dash column
                // (`indent`), not the key-column of the content.
                let child_indent = self.lex.column();
                self.parse_block_mapping_or_scalar_ext(child_indent, indent)?;
            }
            Some(LexHint::Token(k)) => {
                return Err(self.err_here(format!("unexpected token {k:?} in sequence entry")));
            }
        }

        self.eat_inline_trivia();
        self.finish();
        Ok(())
    }

    /// Error if the bytes from the start of the current line up to `pos`
    /// contain a tab. This enforces "tabs not allowed in block indentation"
    /// per user-selected scope decision.
    fn reject_tab_indent_at_line_start(&self) -> Result<(), ParseError> {
        let bytes = self.lex.source().as_bytes();
        let pos = self.lex.pos();
        // Find start of current line.
        let mut start = pos;
        while start > 0 {
            let b = bytes[start - 1];
            if b == b'\n' || b == b'\r' {
                break;
            }
            start -= 1;
        }
        // Scan from line start through the current leading-whitespace token
        // (if pos is at a WHITESPACE/tab that was left by
        // eat_trivia_except_entry_indent, it must also be checked).
        let scan_end = if matches!(bytes.get(pos), Some(b' ' | b'\t')) {
            // Advance past the contiguous whitespace run.
            let mut e = pos;
            while matches!(bytes.get(e), Some(b' ' | b'\t')) {
                e += 1;
            }
            e
        } else {
            pos
        };
        for (i, &b) in bytes[start..scan_end].iter().enumerate() {
            if b == b'\t' {
                return Err(self.err_at(start + i, "tabs not allowed in indentation"));
            }
        }
        Ok(())
    }

    fn parse_scalar_as_node(
        &mut self,
        stop: PlainStop,
        min_indent: Option<usize>,
    ) -> Result<(), ParseError> {
        self.start(SyntaxKind::SCALAR);
        match self.peek() {
            Some(LexHint::Token(SyntaxKind::SINGLE_QUOTED_SCALAR))
            | Some(LexHint::Token(SyntaxKind::DOUBLE_QUOTED_SCALAR)) => {
                self.bump_quoted()?;
            }
            Some(LexHint::PlainStart) => {
                let start = self.lex.pos();
                let text = self.lex.read_plain_with(stop, min_indent);
                if text.is_empty() {
                    return Err(self.err_at(start, "expected scalar content"));
                }
                self.token(SyntaxKind::PLAIN_SCALAR, text);
            }
            _ => return Err(self.err_here("expected scalar")),
        }
        self.finish();
        Ok(())
    }

    fn parse_flow_mapping(&mut self) -> Result<(), ParseError> {
        self.start(SyntaxKind::FLOW_MAPPING);
        // L_BRACE
        self.bump_simple();
        self.flow_depth += 1;
        self.eat_trivia();

        if matches!(self.peek(), Some(LexHint::Token(SyntaxKind::R_BRACE))) {
            self.bump_simple();
            self.flow_depth -= 1;
            self.finish();
            return Ok(());
        }

        loop {
            self.parse_flow_mapping_entry()?;
            self.eat_trivia();
            match self.peek() {
                Some(LexHint::Token(SyntaxKind::COMMA)) => {
                    self.bump_simple();
                    self.eat_trivia();
                    if matches!(self.peek(), Some(LexHint::Token(SyntaxKind::R_BRACE))) {
                        break; // trailing comma
                    }
                }
                Some(LexHint::Token(SyntaxKind::R_BRACE)) => break,
                None => return Err(self.err_here("expected '}' in flow mapping")),
                _ => return Err(self.err_here("expected ',' or '}' in flow mapping")),
            }
        }

        self.eat_trivia();
        if !matches!(self.peek(), Some(LexHint::Token(SyntaxKind::R_BRACE))) {
            return Err(self.err_here("expected '}' in flow mapping"));
        }
        self.bump_simple();
        self.flow_depth -= 1;
        self.finish();
        Ok(())
    }

    fn parse_flow_sequence(&mut self) -> Result<(), ParseError> {
        self.start(SyntaxKind::FLOW_SEQUENCE);
        self.bump_simple();
        self.flow_depth += 1;
        self.eat_trivia();

        if matches!(self.peek(), Some(LexHint::Token(SyntaxKind::R_BRACKET))) {
            self.bump_simple();
            self.flow_depth -= 1;
            self.finish();
            return Ok(());
        }

        loop {
            self.parse_flow_sequence_entry()?;
            self.eat_trivia();
            match self.peek() {
                Some(LexHint::Token(SyntaxKind::COMMA)) => {
                    self.bump_simple();
                    self.eat_trivia();
                    if matches!(self.peek(), Some(LexHint::Token(SyntaxKind::R_BRACKET))) {
                        break; // trailing comma
                    }
                }
                Some(LexHint::Token(SyntaxKind::R_BRACKET)) => break,
                None => return Err(self.err_here("expected ']' in flow sequence")),
                _ => return Err(self.err_here("expected ',' or ']' in flow sequence")),
            }
        }

        self.eat_trivia();
        if !matches!(self.peek(), Some(LexHint::Token(SyntaxKind::R_BRACKET))) {
            return Err(self.err_here("expected ']' in flow sequence"));
        }
        self.bump_simple();
        self.flow_depth -= 1;
        self.finish();
        Ok(())
    }

    fn parse_flow_mapping_entry(&mut self) -> Result<(), ParseError> {
        self.start(SyntaxKind::FLOW_MAPPING_ENTRY);
        self.parse_flow_entry_body()?;
        self.eat_inline_trivia();
        self.finish();
        Ok(())
    }

    /// Flow sequence entries can be (a) a scalar, (b) a nested flow
    /// collection, or (c) an implicit single-pair mapping (`a: 1`).
    /// For (c) we wrap the pair inside FLOW_MAPPING_ENTRY, itself a child
    /// of FLOW_SEQUENCE_ENTRY.
    fn parse_flow_sequence_entry(&mut self) -> Result<(), ParseError> {
        self.start(SyntaxKind::FLOW_SEQUENCE_ENTRY);

        // Node properties at entry position.
        if self.at_node_property_start() {
            let was_alias = self.parse_node_properties_with_alias_check()?;
            if was_alias {
                self.eat_inline_trivia();
                self.finish();
                return Ok(());
            }
            self.eat_trivia();
        }

        match self.peek() {
            Some(LexHint::Token(SyntaxKind::L_BRACE)) => {
                self.parse_flow_mapping()?;
            }
            Some(LexHint::Token(SyntaxKind::L_BRACKET)) => {
                self.parse_flow_sequence()?;
            }
            Some(LexHint::PlainStart)
            | Some(LexHint::Token(
                SyntaxKind::SINGLE_QUOTED_SCALAR | SyntaxKind::DOUBLE_QUOTED_SCALAR,
            )) => {
                // Checkpoint so we can retroactively wrap as
                // FLOW_MAPPING_ENTRY if we see a flow-colon indicator.
                let quoted = matches!(
                    self.peek(),
                    Some(LexHint::Token(
                        SyntaxKind::SINGLE_QUOTED_SCALAR | SyntaxKind::DOUBLE_QUOTED_SCALAR
                    ))
                );
                let cp = self.builder.checkpoint();
                self.parse_scalar_as_node(PlainStop::FlowKey, None)?;
                self.eat_trivia();
                let has_colon = if quoted {
                    self.lex.is_flow_colon_indicator_after_quoted()
                } else {
                    self.lex.is_flow_colon_indicator()
                };
                if has_colon {
                    self.start_at(cp, SyntaxKind::FLOW_MAPPING_ENTRY);
                    self.finish_flow_entry_after_key()?;
                    self.eat_inline_trivia();
                    self.finish(); // FLOW_MAPPING_ENTRY
                }
            }
            // Empty value (e.g., after node properties) — accept.
            Some(LexHint::Token(SyntaxKind::COMMA | SyntaxKind::R_BRACKET)) | None => {}
            _ => return Err(self.err_here("expected flow sequence entry")),
        }

        self.eat_inline_trivia();
        self.finish(); // FLOW_SEQUENCE_ENTRY
        Ok(())
    }

    /// Parse a flow mapping entry body: KEY [:] [VALUE]. The FLOW_MAPPING_ENTRY
    /// marker has already been opened. Handles key-only entries (`{a}`).
    fn parse_flow_entry_body(&mut self) -> Result<(), ParseError> {
        // Optional explicit-key indicator in flow: `{? key: value}`.
        if matches!(self.peek(), Some(LexHint::Token(SyntaxKind::QUESTION))) {
            self.bump_simple();
            self.eat_trivia();
        }
        // Node properties on the key.
        if self.at_node_property_start() {
            let was_alias = self.parse_node_properties_with_alias_check()?;
            if was_alias {
                // A bare alias as a complete entry is permitted (`{*a}`).
                self.eat_trivia();
                return Ok(());
            }
            self.eat_trivia();
        }
        let mut quoted_key = false;
        match self.peek() {
            Some(LexHint::Token(SyntaxKind::L_BRACE)) => self.parse_flow_mapping()?,
            Some(LexHint::Token(SyntaxKind::L_BRACKET)) => self.parse_flow_sequence()?,
            Some(LexHint::PlainStart)
            | Some(LexHint::Token(
                SyntaxKind::SINGLE_QUOTED_SCALAR | SyntaxKind::DOUBLE_QUOTED_SCALAR,
            )) => {
                quoted_key = matches!(
                    self.peek(),
                    Some(LexHint::Token(
                        SyntaxKind::SINGLE_QUOTED_SCALAR | SyntaxKind::DOUBLE_QUOTED_SCALAR
                    ))
                );
                self.parse_scalar_as_node(PlainStop::FlowKey, None)?;
            }
            // Empty key (key-only from properties); fall through to value.
            Some(LexHint::Token(SyntaxKind::COLON | SyntaxKind::COMMA | SyntaxKind::R_BRACE))
            | None => {}
            _ => return Err(self.err_here("expected flow mapping key")),
        }

        self.eat_trivia();
        let has_colon = if quoted_key {
            self.lex.is_flow_colon_indicator_after_quoted()
        } else {
            self.lex.is_flow_colon_indicator()
        };
        if has_colon {
            self.finish_flow_entry_after_key()?;
        }
        Ok(())
    }

    /// At a flow-colon indicator. Consume the COLON, any trivia, then the
    /// value (scalar, flow collection) — or nothing if the value is empty.
    fn finish_flow_entry_after_key(&mut self) -> Result<(), ParseError> {
        self.bump_simple(); // COLON
        self.eat_trivia();
        // Node properties on the value.
        if self.at_node_property_start() {
            let was_alias = self.parse_node_properties_with_alias_check()?;
            if was_alias {
                return Ok(());
            }
            self.eat_trivia();
        }
        match self.peek() {
            Some(LexHint::Token(
                SyntaxKind::COMMA | SyntaxKind::R_BRACE | SyntaxKind::R_BRACKET,
            ))
            | None => {
                self.emit_null(); // empty value after `:`
                Ok(())
            }
            Some(LexHint::Token(SyntaxKind::L_BRACE)) => self.parse_flow_mapping(),
            Some(LexHint::Token(SyntaxKind::L_BRACKET)) => self.parse_flow_sequence(),
            Some(LexHint::PlainStart)
            | Some(LexHint::Token(
                SyntaxKind::SINGLE_QUOTED_SCALAR | SyntaxKind::DOUBLE_QUOTED_SCALAR,
            )) => self.parse_scalar_as_node(PlainStop::FlowValue, None),
            Some(LexHint::Token(k)) => {
                Err(self.err_here(format!("unexpected token {k:?} in flow entry value")))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    fn expect_err(src: &'static str, substring: &str) {
        let err = crate::parse(src).expect_err(&format!("expected error for {src:?}"));
        assert!(
            err.message.contains(substring),
            "for {src:?}: expected message to contain {substring:?}, got {:?}",
            err.message
        );
    }

    #[test]
    fn unterminated_single_quoted() {
        expect_err("x: 'oops\n", "unterminated single-quoted");
    }

    #[test]
    fn unterminated_double_quoted() {
        expect_err("x: \"oops\n", "unterminated double-quoted");
    }

    #[test]
    fn tabs_in_block_indent_rejected() {
        // A block mapping with a tab-indented nested entry.
        expect_err("outer:\n\tinner: x\n", "tabs not allowed");
    }

    #[test]
    fn tabs_in_block_sequence_indent_rejected() {
        expect_err("outer:\n\t- x\n", "tabs not allowed");
    }

    #[test]
    fn recover_always_returns_tree_and_round_trips() {
        // An unterminated quote is a hard error for strict parse.
        let src = "a: 1\nb: 'oops\n";
        assert!(crate::parse(src).is_err());
        let (tree, errors) = crate::parse_recover(src);
        assert!(!errors.is_empty(), "expected at least one error");
        assert_eq!(tree.emit(), src, "recovered tree must round-trip");
    }

    #[test]
    fn recover_captures_remainder_as_error_token() {
        use crate::syntax::SyntaxKind;
        let src = "outer:\n\t- x\n";
        let (tree, errors) = crate::parse_recover(src);
        assert!(!errors.is_empty());
        assert_eq!(tree.emit(), src);
        let has_error = tree
            .root()
            .descendants_with_tokens()
            .any(|e| e.kind() == SyntaxKind::ERROR);
        assert!(has_error, "expected an ERROR token in the recovered tree");
    }

    #[test]
    fn recover_clean_input_has_no_errors() {
        let (tree, errors) = crate::parse_recover("a: 1\nb: 2\n");
        assert!(errors.is_empty());
        assert_eq!(tree.emit(), "a: 1\nb: 2\n");
    }
}
