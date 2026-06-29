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
///
/// Recovery is abandon-on-first-error, so the returned `Vec` holds at
/// most one error (0 on success, 1 on the first failure). The `Vec`
/// return type is kept deliberately: it is the stable public shape and
/// leaves room for multi-error recovery without a signature break.
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
    let root = SyntaxNode::new_root(green);
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

    /// Consume trivia between block-collection entries, but leave the
    /// WHITESPACE immediately preceding the next non-trivia token in place:
    /// that is the next entry's leading indent, which the entry node must
    /// own (see the entry-indent ownership invariant). A WHITESPACE is thus
    /// eaten only when more trivia follows it.
    fn eat_trivia_except_entry_indent(&mut self) {
        loop {
            match self.lex.peek() {
                Some(LexHint::Token(SyntaxKind::NEWLINE | SyntaxKind::COMMENT)) => {
                    self.bump_simple();
                }
                Some(LexHint::Token(SyntaxKind::WHITESPACE)) => {
                    let after = self.peek_after_token();
                    match after {
                        // More trivia follows: eat the whitespace now.
                        Some(LexHint::Token(
                            SyntaxKind::WHITESPACE | SyntaxKind::NEWLINE | SyntaxKind::COMMENT,
                        ))
                        | None => {
                            self.bump_simple();
                        }
                        // Non-trivia follows: this is the entry-leading indent.
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
        // Whether the previous document closed with a `...` end marker. A
        // `...` marks the end of a document and, per YAML 1.2, a *bare*
        // (no `---`) document may follow it — `"a\n...\nb\n"` is a valid
        // two-document stream. Without a preceding `...`, a non-first
        // document still requires `---`/directives.
        let mut prev_ended_with_doc_end = false;
        while !self.lex.at_eof() {
            // Directives (only valid before `---` of the next doc) and the
            // `---`/`...` markers all live INSIDE the DOCUMENT node, so the
            // `%YAML`/`...` tokens travel with their document and the
            // `Document::directives()`/`document_end_token()` accessors —
            // which search the document's own children — see them.
            let has_directives =
                matches!(self.lex.peek(), Some(LexHint::Token(SyntaxKind::DIRECTIVE)));

            if !has_directives
                && !self.at_document_start_marker()
                && !first
                && !prev_ended_with_doc_end
            {
                // No directives, no `---`, not the first document, and the
                // previous document was not closed by `...` — must be EOF or
                // trailing trivia. Any other content here would be silently
                // discarded; that mangles round-tripping (e.g. ansible-vault
                // bodies whose header parses as a bare plain scalar).
                return Err(
                    self.err_here("unexpected content after document; expected '---' or '...'")
                );
            }

            self.start(SyntaxKind::DOCUMENT);

            if has_directives {
                self.parse_directives()?;
                // `---` is required after directives.
                if !self.at_document_start_marker() {
                    return Err(self.err_here("expected '---' after directives"));
                }
            }

            let had_marker = self.at_document_start_marker();
            if had_marker {
                self.bump_directives_end();
                self.eat_trivia();
            }
            self.parse_document_body_if_any(had_marker)?;

            // Optional `...` end marker, inside the document. Only absorb
            // the inter-trivia into the document when a `...` actually
            // follows; otherwise the trailing trivia is document-trailing
            // and belongs at the STREAM level (consumed after `finish`).
            let ends_with_doc_end = self.document_end_marker_follows_trivia();
            if ends_with_doc_end {
                self.eat_trivia();
                self.bump_document_end();
            }

            self.finish(); // DOCUMENT
            self.eat_trivia();

            first = false;
            prev_ended_with_doc_end = ends_with_doc_end;
        }

        self.finish();
        Ok(())
    }

    fn parse_directives(&mut self) -> Result<(), ParseError> {
        self.start(SyntaxKind::DIRECTIVES);
        while matches!(self.lex.peek(), Some(LexHint::Token(SyntaxKind::DIRECTIVE))) {
            self.bump_simple();
            self.eat_trivia();
        }
        self.finish();
        Ok(())
    }

    fn parse_document_body_if_any(&mut self, had_marker: bool) -> Result<(), ParseError> {
        // A document body may be empty (just `---\n`).
        if self.lex.at_eof() {
            return Ok(());
        }
        // Stop at next document boundary.
        if self.at_document_start_marker() || self.at_document_end_marker() {
            return Ok(());
        }
        // After an explicit `---` marker the document root is always at
        // indent 0: the marker itself starts at column 0 and the body may
        // appear at any indentation (e.g. `--- >` puts the `>` at column 4
        // but its content must only be indented > 0). For bare documents
        // (no `---`) we use the actual column of the first content token
        // so that a leading-whitespace sequence (` - a`) is detected at its
        // true indent level and subsequent entries are validated correctly.
        let indent = if had_marker { 0 } else { self.lex.column() };
        // At the document root the plain-scalar continuation barrier is −1
        // (document-level indent per YAML 1.2 §7.3.3), so a column-0 line
        // folds into a root plain scalar (`"a\nb\n"` → one scalar "a b").
        // A following `key:` line is still NOT folded: the continuation
        // gate rejects lines that look like a mapping key.
        self.parse_block_node(indent, -1)?;
        Ok(())
    }

    fn at_document_start_marker(&self) -> bool {
        self.at_line_start_marker(b"---")
    }

    fn at_document_end_marker(&self) -> bool {
        self.at_line_start_marker(b"...")
    }

    /// Reversible lookahead: after skipping trivia (whitespace, newlines,
    /// comments) from the current position, is there a `...` document-end
    /// marker at column 0? Does not advance the lexer or commit tokens.
    fn document_end_marker_follows_trivia(&self) -> bool {
        let bytes = self.lex.source().as_bytes();
        let mut i = self.lex.pos();
        let mut col = self.lex.column();
        loop {
            match bytes.get(i) {
                Some(b' ' | b'\t') => {
                    i += 1;
                    col += 1;
                }
                Some(b'\n') => {
                    i += 1;
                    col = 0;
                }
                Some(b'\r') => {
                    i += 1;
                    if bytes.get(i) == Some(&b'\n') {
                        i += 1;
                    }
                    col = 0;
                }
                Some(b'#') => {
                    while let Some(&c) = bytes.get(i) {
                        if c == b'\n' || c == b'\r' {
                            break;
                        }
                        i += 1;
                    }
                }
                _ => break,
            }
        }
        col == 0
            && bytes.get(i..i + 3) == Some(b"...")
            && matches!(bytes.get(i + 3), None | Some(b' ' | b'\t' | b'\n' | b'\r'))
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
        scalar_min_indent: isize,
    ) -> Result<(), ParseError> {
        // A tab is rejected only where the leading white space is genuine
        // block *indentation* — i.e. it indents a block collection (a
        // DASH-led sequence or a mapping key). Where the white space merely
        // *separates* the node from its surroundings (before a scalar value
        // or a flow node), a tab is valid separation, not indentation. The
        // dispatch arms below apply the check at the points that commit to a
        // block collection; `parse_block_node` itself does not (YAML 1.2
        // §6.1 / §6.2).
        // Node properties (anchor/alias/tag) may precede any block node.
        // If the property group is an alias, it IS the node — return.
        //
        // Capture a checkpoint *before* the properties. When the node that
        // follows on the SAME line turns out to be a block mapping key
        // (`&a k: v`), the properties belong to the KEY scalar — YAML 1.2
        // and libyaml/serde_yaml attach them to the key, not the mapping.
        // Promoting BLOCK_MAPPING/BLOCK_MAPPING_ENTRY from this checkpoint
        // pulls the NODE_PROPERTIES inside the entry, as a preceding
        // sibling of the key scalar (where `Scalar::properties()` finds
        // them). For a lone scalar the checkpoint is unused: properties
        // stay a preceding sibling of the SCALAR, unchanged.
        let props_cp = self.builder.checkpoint();
        // Properties may sit behind the entry-indent WHITESPACE left in place
        // by `eat_trivia_except_entry_indent` (e.g. a mapping value on the
        // next line: `top:\n  &a key: v`). Consume that leading WHITESPACE
        // first — inside the checkpoint — so the property is the current
        // token and any later entry promotion still encloses the indent.
        let had_props = self.at_node_property_start() || self.property_follows_leading_whitespace();
        if had_props {
            self.eat_leading_whitespace();
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
                //
                // A block sequence is the exception: it may be indented at
                // its parent container's column (YAML 1.2 §8.2.1), which can
                // be *less* than the property's own column (`seq:\n &a\n- x`).
                // For that case the floor is `scalar_min_indent`, not `indent`.
                // A block scalar value (`folded:\n   !foo\n  >1\n value`) gets
                // the same relaxed floor: its sigil may sit below the property.
                if let Some(col) = self.column_past_trivia()
                    && (col >= indent
                        || ((self.peek_past_trivia_is_dash()
                            || self.peek_past_trivia_is_block_scalar())
                            && col as isize >= scalar_min_indent))
                {
                    self.eat_trivia();
                    if self.at_document_start_marker() || self.at_document_end_marker() {
                        return Ok(());
                    }
                    // When the decorated value sits *less* indented than the
                    // property, its real parent indent is the container bound
                    // (`scalar_min_indent`), not the property column — a block
                    // scalar's indent indicator is taken relative to that.
                    let node_indent = if col < indent && scalar_min_indent >= 0 {
                        scalar_min_indent as usize
                    } else {
                        col
                    };
                    return self.parse_block_node(node_indent, scalar_min_indent);
                }
                return Ok(());
            }
        }
        // When properties precede a same-line mapping key, the entry must
        // wrap them; pass the pre-properties checkpoint so the mapping/
        // entry promotion encloses the NODE_PROPERTIES.
        let entry_cp = if had_props { Some(props_cp) } else { None };
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
                self.parse_block_mapping_or_scalar_ext(indent, scalar_min_indent, entry_cp)
            }
            Some(LexHint::Token(SyntaxKind::LITERAL_SCALAR | SyntaxKind::FOLDED_SCALAR)) => {
                self.eat_leading_whitespace();
                // The scalar is this node's own content; its content indent is
                // bounded by the enclosing container (`scalar_min_indent`; −1
                // at the document root so column-0 content is valid), not by
                // the node's own column.
                self.parse_block_scalar(scalar_min_indent)
            }
            Some(LexHint::Token(SyntaxKind::SINGLE_QUOTED_SCALAR))
            | Some(LexHint::Token(SyntaxKind::DOUBLE_QUOTED_SCALAR))
            | Some(LexHint::PlainStart) => {
                self.parse_block_mapping_or_scalar_ext(indent, scalar_min_indent, entry_cp)
            }
            // Block-scope empty key: `: value`. The mapping's first entry
            // has an implicit (null) key; emit a zero-width NULL_SCALAR key
            // so the index-based key()/value() accessors line up — the same
            // invariant the flow (`{: v}`) and explicit (`? \n: v`) empty-key
            // paths already maintain.
            Some(LexHint::Token(SyntaxKind::COLON)) => {
                // A lone `:` with neither a value nor a following key is an
                // empty node (null), not an empty-key mapping (`{null: null}`).
                if entry_cp.is_none() && self.colon_has_no_value_or_tail() {
                    self.bump_simple(); // COLON
                    self.emit_null();
                    Ok(())
                } else {
                    self.parse_block_mapping_empty_first_key(indent, entry_cp)
                }
            }
            Some(LexHint::Token(k)) => Err(self.err_here(format!("unexpected token {k:?}"))),
        }
    }

    /// Parse a block mapping whose first entry has an empty (implicit null)
    /// key — the line begins with `:` (`: value`). Positioned at the
    /// leading indent WHITESPACE or the COLON.
    fn parse_block_mapping_empty_first_key(
        &mut self,
        indent: usize,
        props_cp: Option<rowan::Checkpoint>,
    ) -> Result<(), ParseError> {
        let cp = props_cp.unwrap_or_else(|| self.builder.checkpoint());
        // Emit any leading entry-indent WHITESPACE inside the checkpoint so
        // it lands inside BLOCK_MAPPING_ENTRY after checkpoint promotion.
        self.eat_leading_whitespace();
        self.start_at(cp, SyntaxKind::BLOCK_MAPPING);
        self.start_at(cp, SyntaxKind::BLOCK_MAPPING_ENTRY);
        self.emit_null(); // implicit empty key
        self.finish_keyed_mapping_entry(indent)?;
        self.parse_block_mapping_tail(indent)?;
        self.finish(); // BLOCK_MAPPING
        Ok(())
    }

    fn at_node_property_start(&self) -> bool {
        matches!(
            self.lex.peek(),
            Some(LexHint::Token(
                SyntaxKind::ANCHOR | SyntaxKind::ALIAS | SyntaxKind::TAG
            ))
        )
    }

    /// True iff the current token is a WHITESPACE (entry indent) immediately
    /// followed by a node-property token (anchor/alias/tag).
    fn property_follows_leading_whitespace(&mut self) -> bool {
        matches!(
            self.lex.peek(),
            Some(LexHint::Token(SyntaxKind::WHITESPACE))
        ) && matches!(
            self.peek_after_token(),
            Some(LexHint::Token(
                SyntaxKind::ANCHOR | SyntaxKind::ALIAS | SyntaxKind::TAG
            ))
        )
    }

    fn at_line_end_or_eof(&self) -> bool {
        matches!(
            self.lex.peek(),
            None | Some(LexHint::Token(SyntaxKind::NEWLINE | SyntaxKind::COMMENT))
        )
    }

    /// Consume a run of node properties. Returns `Ok(true)` if the run
    /// was a bare alias (which IS the node — caller must not try to
    /// consume further content). Returns `Ok(false)` if the run was
    /// anchors/tags only (caller should continue into node dispatch).
    fn parse_node_properties_with_alias_check(&mut self) -> Result<bool, ParseError> {
        // Bare alias: emit ALIAS_NODE and return.
        if matches!(self.lex.peek(), Some(LexHint::Token(SyntaxKind::ALIAS))) {
            self.start(SyntaxKind::ALIAS_NODE);
            self.consume_alias()?;
            self.finish();
            return Ok(true);
        }
        self.start(SyntaxKind::NODE_PROPERTIES);
        loop {
            match self.lex.peek() {
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
            if matches!(
                self.lex.peek(),
                Some(LexHint::Token(SyntaxKind::WHITESPACE))
            ) && self.next_non_ws_byte_is_property_sigil()
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

    /// Read a scalar that may be a mapping key or a lone scalar: scan it,
    /// then on seeing a `:` retroactively promote it to a BLOCK_MAPPING via
    /// a checkpoint. `scalar_min_indent` is the continuation barrier for a
    /// lone multi-line plain scalar — typically the parent container's
    /// indent (the dash column of a sequence entry, or the key's indent for
    /// a mapping value).
    fn parse_block_mapping_or_scalar_ext(
        &mut self,
        indent: usize,
        scalar_min_indent: isize,
        props_cp: Option<rowan::Checkpoint>,
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
            // An explicit `?` key is a block mapping: leading white space is
            // indentation, so a tab in it is rejected.
            self.reject_tab_indent_at_line_start()?;
            self.start(SyntaxKind::BLOCK_MAPPING);
            self.parse_explicit_block_mapping_entry(indent)?;
            self.parse_block_mapping_tail(indent)?;
            self.finish();
            return Ok(());
        }
        // When node properties precede a same-line mapping key, promote the
        // mapping/entry from *before* those properties (props_cp) so the
        // NODE_PROPERTIES land inside the entry and decorate the key scalar
        // (D4). Otherwise promote from here.
        let cp = props_cp.unwrap_or_else(|| self.builder.checkpoint());
        // Before consuming the leading indent, record whether it holds a tab.
        // It only matters if this turns out to be a block mapping key, where
        // the leading white space is genuine indentation (a tab is illegal).
        // For a lone scalar the same white space is mere separation, so the
        // recorded error is discarded.
        let indent_tab_err = self.reject_tab_indent_at_line_start().err();
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
            if let Some(e) = indent_tab_err {
                return Err(e);
            }
            // Commit scalar as a single-line key, then promote the span
            // since `cp` into BLOCK_MAPPING + BLOCK_MAPPING_ENTRY (so any
            // leading WHITESPACE / NODE_PROPERTIES land inside the entry).
            self.commit_scalar(kind_and_span);
            self.start_at(cp, SyntaxKind::BLOCK_MAPPING);
            self.start_at(cp, SyntaxKind::BLOCK_MAPPING_ENTRY);
            self.finish_keyed_mapping_entry(indent)?;
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
    /// Returns the token kind and the *start* position where scanning
    /// began (the lexer is left advanced past the token text, so the end
    /// is `self.lex.pos()`); `commit_scalar` slices `[start..pos]`.
    fn scan_scalar_first_line(
        &mut self,
        stop: PlainStop,
    ) -> Result<(ScalarTokenKind, usize), ParseError> {
        match self.lex.peek() {
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
            // A leading `:` that is not a colon indicator for this context
            // is the first byte of a plain scalar (`:,`, `::x`).
            Some(LexHint::PlainStart) | Some(LexHint::Token(SyntaxKind::COLON))
                if !self.colon_is_indicator_for(stop) =>
            {
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

    /// Complete an open BLOCK_MAPPING_ENTRY whose key SCALAR has just been
    /// committed: skip inline trivia, require the `:` indicator, parse the
    /// value via `finish_block_mapping_entry_body`, then close the entry.
    /// Shared by the first-entry promotion path and the tail loop.
    fn finish_keyed_mapping_entry(&mut self, indent: usize) -> Result<(), ParseError> {
        self.eat_inline_trivia();
        if !self.lex.is_block_colon_indicator() {
            return Err(self.err_here("expected ':' after mapping key"));
        }
        self.finish_block_mapping_entry_body(indent)?;
        self.eat_inline_trivia();
        self.finish(); // BLOCK_MAPPING_ENTRY
        Ok(())
    }

    /// At this point the mapping entry is open (BLOCK_MAPPING_ENTRY
    /// started via start_node_at). The key SCALAR is inside it, followed
    /// by any inline trivia. We're now positioned at the COLON.
    /// Consumes the colon, optional value, and any indented block value.
    /// Does NOT close BLOCK_MAPPING_ENTRY.
    fn finish_block_mapping_entry_body(&mut self, indent: usize) -> Result<(), ParseError> {
        self.bump_simple(); // COLON
        self.eat_inline_trivia();

        // Node properties on the value.
        if self.at_node_property_start() {
            let was_alias = self.parse_node_properties_with_alias_check()?;
            if was_alias {
                return Ok(());
            }
            self.eat_inline_trivia();
        }

        match self.lex.peek() {
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
                        self.parse_block_node(col, indent as isize)?;
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
            )) => self.parse_scalar_as_node(PlainStop::BlockValue, Some(indent as isize)),
            // A `:` not acting as an indicator (`k: :foo`) is the first byte
            // of a plain scalar value.
            Some(LexHint::Token(SyntaxKind::COLON))
                if !self.colon_is_indicator_for(PlainStop::BlockValue) =>
            {
                self.parse_scalar_as_node(PlainStop::BlockValue, Some(indent as isize))
            }
            Some(LexHint::Token(SyntaxKind::LITERAL_SCALAR | SyntaxKind::FOLDED_SCALAR)) => {
                // Same-line block scalar value (`k: |`): its content must be
                // indented strictly more than the key's column (`indent`), so
                // a same-column sibling key ends it.
                self.parse_block_scalar(indent as isize)
            }
            Some(LexHint::Token(k)) => Err(self.err_here(format!("unexpected token {k:?}"))),
        }
    }

    /// Parse a `? key\n: value` explicit-key mapping entry at `indent`.
    /// Positioned at the leading indent WHITESPACE or the `?` token.
    fn parse_explicit_block_mapping_entry(&mut self, indent: usize) -> Result<(), ParseError> {
        self.start(SyntaxKind::BLOCK_MAPPING_ENTRY);
        self.eat_leading_whitespace();
        self.bump_simple(); // `?`
        self.eat_inline_trivia();
        // Key: single-line scalar, or newline-then-indented block, or empty.
        // An empty key emits a zero-width NULL_SCALAR stand-in so the
        // index-based key()/value() accessors line up (the key is always
        // the first Node child of the entry).
        match self.lex.peek() {
            None => self.emit_null(),
            Some(LexHint::Token(SyntaxKind::NEWLINE)) => {
                // Empty key on this line; look for nested block key.
                if let Some(col) = self.column_past_trivia()
                    && col > indent
                {
                    self.eat_trivia();
                    self.parse_block_node(col, col as isize)?;
                } else {
                    self.emit_null();
                }
            }
            Some(LexHint::PlainStart)
            | Some(LexHint::Token(
                SyntaxKind::SINGLE_QUOTED_SCALAR | SyntaxKind::DOUBLE_QUOTED_SCALAR,
            )) => {
                // An explicit key may be a multi-line plain scalar
                // (`? a\n  true` → key `a true`). Continuation lines fold
                // when indented past the entry's `?` column (`indent`); the
                // matching `:` at `indent` ends the key.
                self.parse_scalar_as_node(PlainStop::BlockValue, Some(indent as isize))?;
            }
            Some(LexHint::Token(SyntaxKind::L_BRACE)) => self.parse_flow_mapping()?,
            Some(LexHint::Token(SyntaxKind::L_BRACKET)) => self.parse_flow_sequence()?,
            Some(LexHint::Token(SyntaxKind::LITERAL_SCALAR | SyntaxKind::FOLDED_SCALAR)) => {
                // Block literal/folded scalar as the explicit key (`? |`);
                // content must be indented past the `?` column (`indent`).
                self.parse_block_scalar(indent as isize)?;
            }
            Some(LexHint::Token(SyntaxKind::QUESTION)) => {
                // Nested explicit-key mapping as the key (`? ? nested ...`).
                // Recurse as a block node at this column.
                let col = self.lex.column();
                self.parse_block_node(col, col as isize)?;
            }
            Some(LexHint::Token(SyntaxKind::ANCHOR | SyntaxKind::ALIAS | SyntaxKind::TAG)) => {
                // Node properties on the explicit key (`? !!str a`). Delegate
                // to the block-node path so the property group is consumed and
                // attached to the key it decorates.
                let col = self.lex.column();
                self.parse_block_node(col, col as isize)?;
            }
            Some(LexHint::Token(k)) => {
                return Err(self.err_here(format!("unexpected token {k:?} after '?'")));
            }
        }
        // Find the matching `:` at `indent`.
        self.eat_trivia();
        if !self.lex.at_eof() {
            let col = self.lex.column();
            if col == indent && matches!(self.lex.peek(), Some(LexHint::Token(SyntaxKind::COLON))) {
                self.finish_block_mapping_entry_body(indent)?;
            }
            // If no matching `:`, it's a key-only entry (empty value).
        }
        self.eat_inline_trivia();
        self.finish(); // BLOCK_MAPPING_ENTRY
        Ok(())
    }

    fn parse_block_scalar(&mut self, parent_indent: isize) -> Result<(), ParseError> {
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
            let key_has_props = matches!(
                key_hint,
                Some(LexHint::Token(
                    SyntaxKind::ANCHOR | SyntaxKind::TAG | SyntaxKind::ALIAS
                ))
            );
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
                _ if key_has_props => {}
                _ => break, // not a key; let caller surface the error
            }

            // Reject tab indentation at the start of a continuation line.
            self.reject_tab_indent_at_line_start()?;

            self.start(SyntaxKind::BLOCK_MAPPING_ENTRY);
            // Consume leading indent whitespace inside the entry.
            self.eat_leading_whitespace();
            // Node properties (anchor/tag) preceding the key land inside the
            // entry as a preceding sibling of the key scalar.
            if key_has_props {
                self.parse_node_properties_with_alias_check()?;
                self.eat_inline_trivia();
            }
            self.parse_scalar_as_node(PlainStop::BlockKey, None)?;
            self.finish_keyed_mapping_entry(indent)?;
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

    /// Skip a trivia run (whitespace, newlines, comments) in `bytes`
    /// starting at byte index `i`, tracking column in `col`. Returns the
    /// `(index, column)` of the first non-trivia byte, or `None` at EOF.
    /// The two callers below differ only in whether they need the column,
    /// so this folds the shared `#`-to-EOL and CRLF byte loops in one place.
    fn skip_trivia_bytes(bytes: &[u8], mut i: usize, mut col: usize) -> Option<(usize, usize)> {
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
                    // comment to end of line; let the outer loop handle the newline
                    while let Some(&c) = bytes.get(i) {
                        if c == b'\n' || c == b'\r' {
                            break;
                        }
                        i += 1;
                    }
                }
                _ => return Some((i, col)),
            }
        }
        None
    }

    /// Return the column of the next non-trivia token, without advancing
    /// the lexer. Returns `None` at EOF.
    fn column_past_trivia(&self) -> Option<usize> {
        let bytes = self.lex.source().as_bytes();
        Self::skip_trivia_bytes(bytes, self.lex.pos(), self.lex.column()).map(|(_, col)| col)
    }

    /// At a block `:` indicator (`self.lex` positioned on the COLON).
    /// True iff there is no value after the colon and no following
    /// mapping key — i.e. the colon is the entire remaining body (a lone
    /// `:` document). Reversible byte lookahead. Used to decode `:` as a
    /// null node rather than an empty-key `{null: null}` mapping.
    fn colon_has_no_value_or_tail(&self) -> bool {
        let bytes = self.lex.source().as_bytes();
        // Skip the `:` itself, then all trivia (whitespace/newlines/
        // comments). If only trivia remains, there is neither a value nor a
        // following key.
        match Self::skip_trivia_bytes(bytes, self.lex.pos() + 1, self.lex.column() + 1) {
            None => true,
            Some((i, _)) => {
                // A document-end `...` marker also means no further content.
                bytes.get(i..i + 3) == Some(b"...")
                    && matches!(bytes.get(i + 3), None | Some(b' ' | b'\t' | b'\n' | b'\r'))
            }
        }
    }

    /// True iff, after skipping trivia, the next byte is a DASH acting
    /// as a block-sequence indicator.
    fn peek_past_trivia_is_dash(&self) -> bool {
        let bytes = self.lex.source().as_bytes();
        match Self::skip_trivia_bytes(bytes, self.lex.pos(), 0) {
            Some((i, _)) if bytes.get(i) == Some(&b'-') => {
                matches!(bytes.get(i + 1), None | Some(b' ' | b'\t' | b'\n' | b'\r'))
            }
            _ => false,
        }
    }

    /// Whether the next significant token (past trivia) starts a block
    /// scalar (`|` or `>`). Like a block sequence, a block scalar value may
    /// be indented less than a preceding property on its own line — down to
    /// the enclosing container's bound — so it needs the same relaxed floor.
    fn peek_past_trivia_is_block_scalar(&self) -> bool {
        let bytes = self.lex.source().as_bytes();
        matches!(
            Self::skip_trivia_bytes(bytes, self.lex.pos(), 0),
            Some((i, _)) if matches!(bytes.get(i), Some(b'|' | b'>'))
        )
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
            let at_dash = match self.lex.peek() {
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

        // Leading indent whitespace is absent for the first entry: the
        // parent already consumed it before opening the sequence.
        self.eat_leading_whitespace();

        self.bump_simple(); // DASH
        self.eat_inline_trivia();

        // Node properties on the entry's value. Checkpoint *before* them so
        // that if the value turns out to be a same-line mapping key
        // (`- &a k: v`), the inner mapping/entry promotion can enclose the
        // NODE_PROPERTIES and attach them to the key scalar — consistent
        // with the bare-mapping `&a k: v` D4 behavior.
        let props_cp = self.builder.checkpoint();
        let mut entry_cp = None;
        if self.at_node_property_start() {
            let was_alias = self.parse_node_properties_with_alias_check()?;
            if was_alias {
                self.eat_inline_trivia();
                self.finish();
                return Ok(());
            }
            self.eat_inline_trivia();
            entry_cp = Some(props_cp);
        }

        match self.lex.peek() {
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
                        self.parse_block_node(col, col as isize)?;
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
                // Same-line block scalar entry (`- |`): content must be
                // indented past the dash column (`indent`).
                self.parse_block_scalar(indent as isize)?;
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
                self.parse_block_mapping_or_scalar_ext(child_indent, indent as isize, entry_cp)?;
            }
            // A `:` not acting as a separator (`- :,`, `- ::x`) starts a
            // plain scalar entry. (A `:` that *is* an indicator means an
            // empty-key mapping entry and is handled elsewhere.)
            Some(LexHint::Token(SyntaxKind::COLON)) if !self.lex.is_block_colon_indicator() => {
                let child_indent = self.lex.column();
                self.parse_block_mapping_or_scalar_ext(child_indent, indent as isize, entry_cp)?;
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
    /// contain a tab: tabs are not allowed in block indentation.
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

    /// Whether the `:` at the current position acts as a key/value colon
    /// indicator in the given context (so it must NOT be read as the start
    /// of a plain scalar). For a non-`:` position this is always false.
    fn colon_is_indicator_for(&self, stop: PlainStop) -> bool {
        if self.lex.peek() != Some(LexHint::Token(SyntaxKind::COLON)) {
            return false;
        }
        match stop {
            PlainStop::FlowKey | PlainStop::FlowValue => self.lex.is_flow_colon_indicator(),
            PlainStop::BlockKey | PlainStop::BlockValue => self.lex.is_block_colon_indicator(),
        }
    }

    fn parse_scalar_as_node(
        &mut self,
        stop: PlainStop,
        min_indent: Option<isize>,
    ) -> Result<(), ParseError> {
        self.start(SyntaxKind::SCALAR);
        match self.lex.peek() {
            Some(LexHint::Token(SyntaxKind::SINGLE_QUOTED_SCALAR))
            | Some(LexHint::Token(SyntaxKind::DOUBLE_QUOTED_SCALAR)) => {
                self.bump_quoted()?;
            }
            // A leading `:` that is NOT a colon indicator for this context
            // starts a plain scalar (`:x`, `::vector`, `:,` in a sequence
            // entry). The lexer always classifies `:` as a COLON token, so
            // detect the non-indicator case here and scan it as plain.
            Some(LexHint::PlainStart) | Some(LexHint::Token(SyntaxKind::COLON))
                if !self.colon_is_indicator_for(stop) =>
            {
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
        self.parse_flow_collection(
            SyntaxKind::FLOW_MAPPING,
            SyntaxKind::R_BRACE,
            "}",
            Self::parse_flow_mapping_entry,
        )
    }

    fn parse_flow_sequence(&mut self) -> Result<(), ParseError> {
        self.parse_flow_collection(
            SyntaxKind::FLOW_SEQUENCE,
            SyntaxKind::R_BRACKET,
            "]",
            Self::parse_flow_sequence_entry,
        )
    }

    fn parse_flow_collection(
        &mut self,
        container: SyntaxKind,
        close: SyntaxKind,
        close_ch: &str,
        parse_entry: fn(&mut Self) -> Result<(), ParseError>,
    ) -> Result<(), ParseError> {
        self.start(container);
        self.bump_simple(); // opening delimiter
        self.eat_trivia();

        if matches!(self.lex.peek(), Some(LexHint::Token(k)) if k == close) {
            self.bump_simple();
            self.finish();
            return Ok(());
        }

        loop {
            parse_entry(self)?;
            self.eat_trivia();
            match self.lex.peek() {
                Some(LexHint::Token(SyntaxKind::COMMA)) => {
                    self.bump_simple();
                    self.eat_trivia();
                    if matches!(self.lex.peek(), Some(LexHint::Token(k)) if k == close) {
                        break; // trailing comma
                    }
                }
                Some(LexHint::Token(k)) if k == close => break,
                None => {
                    return Err(self.err_here(format!("expected '{close_ch}' in flow collection")));
                }
                _ => {
                    return Err(
                        self.err_here(format!("expected ',' or '{close_ch}' in flow collection"))
                    );
                }
            }
        }

        self.eat_trivia();
        if !matches!(self.lex.peek(), Some(LexHint::Token(k)) if k == close) {
            return Err(self.err_here(format!("expected '{close_ch}' in flow collection")));
        }
        self.bump_simple();
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

        match self.lex.peek() {
            Some(LexHint::Token(SyntaxKind::L_BRACE)) => {
                self.parse_flow_mapping()?;
            }
            Some(LexHint::Token(SyntaxKind::L_BRACKET)) => {
                self.parse_flow_sequence()?;
            }
            // Explicit-key entry (`[? foo : baz]`): a single-pair flow
            // mapping. `parse_flow_entry_body` consumes the `?`, key, `:`,
            // and value; wrap it as FLOW_MAPPING > FLOW_MAPPING_ENTRY so the
            // AST layer sees a mapping value of the sequence entry.
            Some(LexHint::Token(SyntaxKind::QUESTION)) => {
                self.start(SyntaxKind::FLOW_MAPPING);
                self.start(SyntaxKind::FLOW_MAPPING_ENTRY);
                self.parse_flow_entry_body()?;
                self.eat_inline_trivia();
                self.finish(); // FLOW_MAPPING_ENTRY
                self.finish(); // FLOW_MAPPING
            }
            Some(LexHint::PlainStart)
            | Some(LexHint::Token(
                SyntaxKind::SINGLE_QUOTED_SCALAR | SyntaxKind::DOUBLE_QUOTED_SCALAR,
            )) => {
                // Checkpoint so we can retroactively wrap as a single-pair
                // FLOW_MAPPING if we see a flow-colon indicator. The pair
                // must be a FLOW_MAPPING_ENTRY *inside* a FLOW_MAPPING node
                // so the AST layer (`Node::cast`/`as_mapping`) recognizes it
                // as a mapping value of the sequence entry.
                let cp = self.builder.checkpoint();
                if self.parse_flow_scalar_then_colon()? {
                    self.start_at(cp, SyntaxKind::FLOW_MAPPING);
                    self.start_at(cp, SyntaxKind::FLOW_MAPPING_ENTRY);
                    self.finish_flow_entry_after_key()?;
                    self.eat_inline_trivia();
                    self.finish(); // FLOW_MAPPING_ENTRY
                    self.finish(); // FLOW_MAPPING
                }
            }
            // A `:` not acting as a separator (`[:x]`, `[::vector]`) starts a
            // plain scalar entry. Use the same scalar-then-colon path so a
            // trailing `: v` still promotes to a single-pair mapping.
            Some(LexHint::Token(SyntaxKind::COLON))
                if !self.colon_is_indicator_for(PlainStop::FlowKey) =>
            {
                let cp = self.builder.checkpoint();
                if self.parse_flow_scalar_then_colon()? {
                    self.start_at(cp, SyntaxKind::FLOW_MAPPING);
                    self.start_at(cp, SyntaxKind::FLOW_MAPPING_ENTRY);
                    self.finish_flow_entry_after_key()?;
                    self.eat_inline_trivia();
                    self.finish(); // FLOW_MAPPING_ENTRY
                    self.finish(); // FLOW_MAPPING
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
        if matches!(self.lex.peek(), Some(LexHint::Token(SyntaxKind::QUESTION))) {
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
        match self.lex.peek() {
            Some(LexHint::Token(SyntaxKind::L_BRACE)) => {
                self.parse_flow_mapping()?;
                self.eat_trivia();
                if self.lex.is_flow_colon_indicator() {
                    self.finish_flow_entry_after_key()?;
                }
            }
            Some(LexHint::Token(SyntaxKind::L_BRACKET)) => {
                self.parse_flow_sequence()?;
                self.eat_trivia();
                if self.lex.is_flow_colon_indicator() {
                    self.finish_flow_entry_after_key()?;
                }
            }
            Some(LexHint::PlainStart)
            | Some(LexHint::Token(
                SyntaxKind::SINGLE_QUOTED_SCALAR | SyntaxKind::DOUBLE_QUOTED_SCALAR,
            )) => {
                if self.parse_flow_scalar_then_colon()? {
                    self.finish_flow_entry_after_key()?;
                }
            }
            // Empty key (key-only from properties); fall through to value.
            // Emit a NULL_SCALAR stand-in so index-based key()/value()
            // accessors line up (the empty key is the first Node child).
            Some(LexHint::Token(SyntaxKind::COLON | SyntaxKind::COMMA | SyntaxKind::R_BRACE))
            | None => {
                self.emit_null();
                self.eat_trivia();
                if self.lex.is_flow_colon_indicator() {
                    self.finish_flow_entry_after_key()?;
                }
            }
            _ => return Err(self.err_here("expected flow mapping key")),
        }
        Ok(())
    }

    /// Parse a flow scalar at key position (`parse_scalar_as_node` with
    /// `FlowKey` stop), then eat trivia and report whether a flow-colon
    /// indicator follows — picking the quoted vs plain colon probe based
    /// on the scalar kind. Shared by `parse_flow_sequence_entry` and
    /// `parse_flow_entry_body`.
    fn parse_flow_scalar_then_colon(&mut self) -> Result<bool, ParseError> {
        let quoted = matches!(
            self.lex.peek(),
            Some(LexHint::Token(
                SyntaxKind::SINGLE_QUOTED_SCALAR | SyntaxKind::DOUBLE_QUOTED_SCALAR
            ))
        );
        self.parse_scalar_as_node(PlainStop::FlowKey, None)?;
        self.eat_trivia();
        Ok(if quoted {
            self.lex.is_flow_colon_indicator_after_quoted()
        } else {
            self.lex.is_flow_colon_indicator()
        })
    }

    /// At a flow-colon indicator. Consume the COLON, any trivia, then the
    /// value (scalar, flow collection) — or nothing if the value is empty.
    fn finish_flow_entry_after_key(&mut self) -> Result<(), ParseError> {
        // The `:` here may not be followed by a space (`{a:1}`); emit it
        // as a COLON token directly rather than via `bump_simple` (whose
        // `peek()` would classify such a `:` as a plain-scalar start).
        let (k, t) = self.lex.bump_colon();
        self.token(k, t);
        self.eat_trivia();
        // Node properties on the value.
        if self.at_node_property_start() {
            let was_alias = self.parse_node_properties_with_alias_check()?;
            if was_alias {
                return Ok(());
            }
            self.eat_trivia();
        }
        match self.lex.peek() {
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
            // A `:` not acting as a separator (`{x: :x}`) starts a plain
            // scalar value.
            Some(LexHint::Token(SyntaxKind::COLON))
                if !self.colon_is_indicator_for(PlainStop::FlowValue) =>
            {
                self.parse_scalar_as_node(PlainStop::FlowValue, None)
            }
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

    // --- B4: flow plain values must terminate at ": " ---

    #[test]
    fn b4_flow_plain_value_rejects_colon_space() {
        // A plain scalar in flow context may not contain ": " (YAML 1.2);
        // `{k: v: w}` must be a parse error rather than value "v: w".
        assert!(crate::parse("{k: v: w}\n").is_err());
    }

    #[test]
    fn b4_legal_flow_value_still_parses() {
        use crate::ast::{AstNode, Stream};
        let tree = crate::parse("{k: v}\n").expect("legal flow mapping");
        let stream = Stream::cast(tree.root().clone()).unwrap();
        let doc = stream.documents().next().unwrap();
        let root = doc.root_node().unwrap();
        let mapping = root.as_mapping().unwrap();
        let (_k, v) = mapping.pairs().next().unwrap();
        let v = v.expect("value present");
        assert_eq!(v.syntax().text().to_string(), "v");
    }

    #[test]
    fn b4_flow_value_with_colon_no_space_ok() {
        // `:` not followed by space/flow-terminator stays plain content.
        let tree = crate::parse("{k: a:b}\n").expect("colon-without-space is content");
        assert_eq!(tree.emit(), "{k: a:b}\n");
    }

    // --- D3: multi-line plain scalars in flow context fold to a space ---

    fn scalar_decoded(n: &crate::ast::Node) -> String {
        match n {
            crate::ast::Node::Scalar(s) => s.decoded().unwrap(),
            other => panic!("expected scalar, got {:?}", other.syntax().kind()),
        }
    }

    #[test]
    fn d3_flow_sequence_multiline_plain_folds() {
        use crate::ast::{AstNode, Stream};
        let src = "[a\nb]\n";
        let tree = crate::parse(src).expect("multi-line plain in flow seq");
        assert_eq!(tree.emit(), src);
        let stream = Stream::cast(tree.root().clone()).unwrap();
        let doc = stream.documents().next().unwrap();
        let root = doc.root_node().unwrap();
        let seq = root.as_sequence().unwrap();
        let item = seq.iter().next().unwrap();
        assert_eq!(scalar_decoded(&item), "a b");
    }

    #[test]
    fn d3_flow_mapping_multiline_plain_value_folds() {
        use crate::ast::{AstNode, Stream};
        let src = "{k: a\nb}\n";
        let tree = crate::parse(src).expect("multi-line plain flow value");
        assert_eq!(tree.emit(), src);
        let stream = Stream::cast(tree.root().clone()).unwrap();
        let doc = stream.documents().next().unwrap();
        let root = doc.root_node().unwrap();
        let mapping = root.as_mapping().unwrap();
        let (_k, v) = mapping.pairs().next().unwrap();
        assert_eq!(scalar_decoded(&v.unwrap()), "a b");
    }

    #[test]
    fn d3_b4_still_enforced_after_multiline() {
        // B4 must remain: `: ` inside a flow plain value is still illegal.
        assert!(crate::parse("{k: v: w}\n").is_err());
    }

    // --- D4: anchor/tag before a key attaches to the KEY scalar ---

    #[test]
    fn d4_anchor_before_key_decorates_key_scalar() {
        use crate::ast::{AstNode, Stream};
        let src = "&a k: v\n";
        let tree = crate::parse(src).expect("anchor before key");
        assert_eq!(tree.emit(), src);
        let stream = Stream::cast(tree.root().clone()).unwrap();
        let doc = stream.documents().next().unwrap();
        let root = doc.root_node().unwrap();
        // The mapping itself carries no properties.
        assert!(
            root.properties().is_none(),
            "mapping must NOT carry the key's anchor"
        );
        let mapping = root.as_mapping().unwrap();
        let (k, v) = mapping.pairs().next().unwrap();
        let props = k.properties().expect("key scalar carries properties");
        assert_eq!(props.anchor_name().as_deref(), Some("a"));
        assert_eq!(scalar_decoded(&k), "k");
        assert_eq!(scalar_decoded(&v.unwrap()), "v");
    }

    #[test]
    fn d4_value_anchor_still_decorates_value() {
        // The seq_value_anchor shape: anchor on a value (nested mapping)
        // must remain attached to the value, not the key.
        use crate::ast::{AstNode, Stream};
        let src = "base: &base\n  host: alpha\n";
        let tree = crate::parse(src).expect("value anchor");
        assert_eq!(tree.emit(), src);
        let stream = Stream::cast(tree.root().clone()).unwrap();
        let doc = stream.documents().next().unwrap();
        let root = doc.root_node().unwrap();
        let mapping = root.as_mapping().unwrap();
        let (k, v) = mapping.pairs().next().unwrap();
        assert!(k.properties().is_none(), "key must not carry value anchor");
        let v = v.unwrap();
        let props = v.properties().expect("value carries anchor");
        assert_eq!(props.anchor_name().as_deref(), Some("base"));
    }

    // --- D1: root plain scalar continuation at column 0 ---

    fn decoded_root_scalar(src: &str) -> String {
        use crate::ast::{AstNode, Stream};
        let tree = crate::parse(src).unwrap_or_else(|e| panic!("parse {src:?}: {e:?}"));
        assert_eq!(tree.emit(), src, "round-trip for {src:?}");
        let stream = Stream::cast(tree.root().clone()).unwrap();
        let doc = stream.documents().next().unwrap();
        let root = doc.root_node().unwrap();
        scalar_decoded(&root)
    }

    #[test]
    fn d1_root_plain_continues_at_column_zero() {
        assert_eq!(decoded_root_scalar("a\nb\n"), "a b");
        assert_eq!(decoded_root_scalar("a\nb\nc\n"), "a b c");
    }

    #[test]
    fn d1_does_not_merge_separate_mapping_lines() {
        use crate::ast::{AstNode, Stream};
        let src = "a: 1\nb: 2\n";
        let tree = crate::parse(src).expect("two-entry mapping");
        assert_eq!(tree.emit(), src);
        let stream = Stream::cast(tree.root().clone()).unwrap();
        let doc = stream.documents().next().unwrap();
        let root = doc.root_node().unwrap();
        let mapping = root.as_mapping().unwrap();
        assert_eq!(mapping.pairs().count(), 2, "must stay a 2-entry mapping");
    }

    #[test]
    fn d1_root_continuation_stops_at_document_markers_and_eof() {
        // A `...` terminates the root scalar rather than folding.
        use crate::ast::{AstNode, Stream};
        let src = "a\n...\n";
        let tree = crate::parse(src).expect("scalar then doc-end");
        assert_eq!(tree.emit(), src);
        let stream = Stream::cast(tree.root().clone()).unwrap();
        assert_eq!(stream.documents().count(), 1);
    }

    #[test]
    fn d1_multi_doc_still_parses() {
        use crate::ast::{AstNode, Stream};
        let src = "a\nb\n---\nc\nd\n";
        let tree = crate::parse(src).expect("multi-doc with folded roots");
        assert_eq!(tree.emit(), src);
        let stream = Stream::cast(tree.root().clone()).unwrap();
        assert_eq!(stream.documents().count(), 2);
    }

    // --- D2: bare document after `...` ---

    #[test]
    fn d2_bare_document_after_doc_end() {
        use crate::ast::{AstNode, Stream};
        let src = "a\n...\nb\n";
        let tree = crate::parse(src).expect("bare doc after ...");
        assert_eq!(tree.emit(), src);
        let stream = Stream::cast(tree.root().clone()).unwrap();
        let docs: Vec<_> = stream.documents().collect();
        assert_eq!(docs.len(), 2, "expected two documents");
        assert_eq!(scalar_decoded(&docs[0].root_node().unwrap()), "a");
        assert_eq!(scalar_decoded(&docs[1].root_node().unwrap()), "b");
    }

    #[test]
    fn d2_still_rejects_bare_doc_without_marker() {
        // Without a `...` (or `---`), a second bare document is still an
        // error — D1 root folding only applies to *continuation* lines,
        // and `b: 2` here is a separate mapping line that cannot fold into
        // the scalar `a`.
        assert!(crate::parse("a\nb: 2\n").is_err());
    }

    // --- B6: empty keys emit a NULL_SCALAR so key()/value() line up ---

    #[test]
    fn b6_flow_empty_key() {
        use crate::ast::{AstNode, Stream};
        let tree = crate::parse("{: v}\n").expect("empty flow key");
        let stream = Stream::cast(tree.root().clone()).unwrap();
        let doc = stream.documents().next().unwrap();
        let root = doc.root_node().unwrap();
        let entry = root
            .syntax()
            .children()
            .find_map(crate::ast::FlowMappingEntry::cast)
            .expect("flow mapping entry");
        let key = entry.key().expect("key node present");
        // The empty key decodes to the null scalar (empty string).
        assert_eq!(key.syntax().text().to_string(), "");
        let value = entry.value().expect("value node present");
        assert_eq!(value.syntax().text().to_string(), "v");
    }

    #[test]
    fn b6_block_scope_empty_key() {
        use crate::ast::{AstNode, Stream};
        let src = ": v\n";
        let tree = crate::parse(src).expect("block-scope empty key");
        assert_eq!(tree.emit(), src);
        let stream = Stream::cast(tree.root().clone()).unwrap();
        let doc = stream.documents().next().unwrap();
        let root = doc.root_node().unwrap();
        let entry = root
            .syntax()
            .children()
            .find_map(crate::ast::BlockMappingEntry::cast)
            .expect("block mapping entry");
        let key = entry.key().expect("key node present");
        assert_eq!(key.syntax().text().to_string(), "");
        let value = entry.value().expect("value node present");
        assert_eq!(value.syntax().text().to_string(), "v");
    }

    #[test]
    fn b6_block_empty_key_with_following_entry() {
        use crate::ast::{AstNode, Stream};
        let src = ": v\nk: w\n";
        let tree = crate::parse(src).expect("empty key then normal entry");
        assert_eq!(tree.emit(), src);
        let stream = Stream::cast(tree.root().clone()).unwrap();
        let doc = stream.documents().next().unwrap();
        let root = doc.root_node().unwrap();
        let mapping = root.as_mapping().unwrap();
        assert_eq!(mapping.pairs().count(), 2);
    }

    #[test]
    fn b6_explicit_block_empty_key() {
        use crate::ast::{AstNode, Stream};
        let tree = crate::parse("? \n: v\n").expect("explicit empty key");
        let stream = Stream::cast(tree.root().clone()).unwrap();
        let doc = stream.documents().next().unwrap();
        let root = doc.root_node().unwrap();
        let entry = root
            .syntax()
            .children()
            .find_map(crate::ast::BlockMappingEntry::cast)
            .expect("block mapping entry");
        let key = entry.key().expect("key node present");
        assert_eq!(key.syntax().text().to_string(), "");
        let value = entry.value().expect("value node present");
        assert_eq!(value.syntax().text().to_string(), "v");
    }

    // --- A1: directives/document-end live inside DOCUMENT ---

    #[test]
    fn a1_directives_and_document_end_inside_document() {
        use crate::ast::{AstNode, Stream};
        let src = "%YAML 1.2\n---\na: 1\n...\n";
        let tree = crate::parse(src).expect("directives + doc end");
        assert_eq!(tree.emit(), src);
        let stream = Stream::cast(tree.root().clone()).unwrap();
        let doc = stream.documents().next().unwrap();
        assert!(doc.directives().is_some(), "directives() should be Some");
        assert!(
            doc.document_end_token().is_some(),
            "document_end_token() should be Some"
        );
        assert!(
            doc.directives_end_token().is_some(),
            "directives_end_token() should be Some"
        );
    }
}
