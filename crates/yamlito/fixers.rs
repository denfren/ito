use rowan::{NodeOrToken, WalkEvent};

use crate::SyntaxTree;
use crate::edit::{GreenElement, TreeEdit, apply_edit, mk_node, mk_token, splice_children_green};
use crate::lexer::{
    as_plain_scalar, as_single_quoted, as_yaml_1_1_bool, is_yaml_1_1_octal, is_yaml_1_1_sexagesimal,
};
use crate::syntax::tree_utils::{
    contains_block_scalar, is_block_entry, is_key_position, is_trailing_whitespace,
    observed_block_col,
};
use crate::syntax::{SyntaxKind, SyntaxNode, SyntaxToken};

/// Rewrite boolean words: `true/false` in value position, single-quote
/// in key position.
pub fn yaml11_bool_to_yaml12_bool(tree: &mut SyntaxTree) -> usize {
    enum Target {
        Boolean(SyntaxNode, bool),
        Quote(SyntaxNode, String),
    }
    let mut targets: Vec<Target> = Vec::new();
    for ev in tree.root().preorder_with_tokens() {
        let WalkEvent::Enter(NodeOrToken::Token(t)) = ev else {
            continue;
        };
        if t.kind() != SyntaxKind::PLAIN_SCALAR {
            continue;
        }
        let Some(b) = as_yaml_1_1_bool(t.text()) else {
            continue;
        };
        let Some(scalar) = t.parent() else { continue };
        if scalar.kind() != SyntaxKind::SCALAR {
            continue;
        }
        if is_key_position(&scalar) {
            targets.push(Target::Quote(scalar, t.text().to_string()));
        } else {
            targets.push(Target::Boolean(scalar, b));
        }
    }
    let n = targets.len();
    for t in targets {
        match t {
            Target::Boolean(scalar, b) => {
                let text = if b { "true" } else { "false" };
                let new = mk_node(
                    SyntaxKind::SCALAR,
                    [NodeOrToken::Token(mk_token(SyntaxKind::PLAIN_SCALAR, text))],
                );
                apply_edit(
                    tree,
                    TreeEdit::ReplaceNode {
                        target: scalar,
                        new,
                    },
                );
            }
            Target::Quote(scalar, text) => single_quote_scalar(tree, scalar, &text),
        }
    }
    n
}

/// Rewrite `0[0-7]+` plain scalars to single-quoted strings. Skips
/// bare `0` (already 1.2-safe) and anything containing `8` or `9`.
pub fn yaml11_octal_to_quoted(tree: &mut SyntaxTree) -> usize {
    quote_matching_plain_scalars(tree, is_yaml_1_1_octal)
}

/// Rewrite sexagesimal numbers (`1:30`, `-12:34:56.78`) to
/// single-quoted strings. Each `:`-separated segment after the first
/// must be 0–59.
pub fn yaml11_sexagesimal_to_quoted(tree: &mut SyntaxTree) -> usize {
    quote_matching_plain_scalars(tree, is_yaml_1_1_sexagesimal)
}

/// Collect the SCALAR nodes whose PLAIN_SCALAR token satisfies `pred`,
/// paired with the token text.
fn collect_plain_scalar_targets(
    tree: &SyntaxTree,
    pred: impl Fn(&str) -> bool,
) -> Vec<(SyntaxNode, String)> {
    let mut targets = Vec::new();
    for ev in tree.root().preorder_with_tokens() {
        let WalkEvent::Enter(NodeOrToken::Token(t)) = ev else {
            continue;
        };
        if t.kind() != SyntaxKind::PLAIN_SCALAR || !pred(t.text()) {
            continue;
        }
        let Some(scalar) = t.parent() else { continue };
        if scalar.kind() != SyntaxKind::SCALAR {
            continue;
        }
        targets.push((scalar, t.text().to_string()));
    }
    targets
}

/// Replace a SCALAR node with a single-quoted scalar wrapping `text`.
fn single_quote_scalar(tree: &mut SyntaxTree, scalar: SyntaxNode, text: &str) {
    let new = mk_node(
        SyntaxKind::SCALAR,
        [NodeOrToken::Token(mk_token(
            SyntaxKind::SINGLE_QUOTED_SCALAR,
            &format!("'{text}'"),
        ))],
    );
    apply_edit(
        tree,
        TreeEdit::ReplaceNode {
            target: scalar,
            new,
        },
    );
}

fn quote_matching_plain_scalars(tree: &mut SyntaxTree, pred: fn(&str) -> bool) -> usize {
    let targets = collect_plain_scalar_targets(tree, pred);
    let n = targets.len();
    for (scalar, text) in targets {
        single_quote_scalar(tree, scalar, &text);
    }
    n
}

pub fn trim_trailing_whitespace(tree: &mut SyntaxTree) -> usize {
    enum Target {
        RemoveWhitespace(SyntaxToken),
        TrimComment(SyntaxToken, String),
    }
    let mut targets: Vec<Target> = Vec::new();
    for ev in tree.root().preorder_with_tokens() {
        let WalkEvent::Enter(NodeOrToken::Token(t)) = ev else {
            continue;
        };
        match t.kind() {
            SyntaxKind::WHITESPACE => {
                if is_trailing_whitespace(&t) {
                    targets.push(Target::RemoveWhitespace(t));
                }
            }
            SyntaxKind::COMMENT => {
                let text = t.text();
                let trimmed = text.trim_end_matches([' ', '\t']);
                if trimmed.len() != text.len() {
                    targets.push(Target::TrimComment(t.clone(), trimmed.to_string()));
                }
            }
            _ => {}
        }
    }
    let n = targets.len();
    for target in targets {
        match target {
            Target::RemoveWhitespace(t) => {
                apply_edit(tree, TreeEdit::RemoveToken { target: t });
            }
            Target::TrimComment(t, trimmed) => {
                let new = mk_token(SyntaxKind::COMMENT, &trimmed);
                apply_edit(tree, TreeEdit::ReplaceToken { target: t, new });
            }
        }
    }
    n
}

pub fn quote_yaml_1_1_bool(tree: &mut SyntaxTree) -> usize {
    quote_matching_plain_scalars(tree, |s| as_yaml_1_1_bool(s).is_some())
}

#[allow(clippy::enum_variant_names)]
enum IndentTarget {
    /// Replace an existing WHITESPACE token's text with `spaces`.
    ReplaceWs(SyntaxToken, String),
    /// Remove a WHITESPACE whose desired width is 0.
    RemoveWs(SyntaxToken),
    /// Insert a WHITESPACE token into `parent` at index `at` with
    /// `spaces` content. Used when no WHITESPACE exists between the
    /// NEWLINE and the following entry / block.
    InsertWs {
        parent: SyntaxNode,
        at: usize,
        spaces: String,
    },
}

pub fn reindent(tree: &mut SyntaxTree) -> usize {
    let mut targets: Vec<IndentTarget> = Vec::new();
    if let Some(doc_root) = find_document_root(tree.root()) {
        collect_indent_targets(&doc_root, 0, &mut targets);
    }

    let n = targets.len();
    for t in targets {
        match t {
            IndentTarget::ReplaceWs(tok, spaces) => {
                let new = mk_token(SyntaxKind::WHITESPACE, &spaces);
                apply_edit(tree, TreeEdit::ReplaceToken { target: tok, new });
            }
            IndentTarget::RemoveWs(tok) => {
                apply_edit(tree, TreeEdit::RemoveToken { target: tok });
            }
            IndentTarget::InsertWs { parent, at, spaces } => {
                let ws = NodeOrToken::Token(mk_token(SyntaxKind::WHITESPACE, &spaces));
                let new_green = splice_children_green(&parent, at..at, vec![ws as GreenElement]);
                apply_edit(
                    tree,
                    TreeEdit::ReplaceNode {
                        target: parent,
                        new: new_green,
                    },
                );
            }
        }
    }
    n
}

/// Find the first block-collection node under STREAM → DOCUMENT. Returns
/// `None` for documents whose root is a scalar, alias, or flow collection.
fn find_document_root(stream: &SyntaxNode) -> Option<SyntaxNode> {
    if stream.kind() != SyntaxKind::STREAM {
        return None;
    }
    for doc in stream.children() {
        if doc.kind() != SyntaxKind::DOCUMENT {
            continue;
        }
        for child in doc.children() {
            if matches!(
                child.kind(),
                SyntaxKind::BLOCK_MAPPING | SyntaxKind::BLOCK_SEQUENCE
            ) {
                return Some(child);
            }
        }
    }
    None
}

/// Walk `block` (a BLOCK_MAPPING or BLOCK_SEQUENCE) whose entries
/// belong at column `col`. Record indent-fix targets and recurse into
/// nested block collections.
///
/// Entries whose subtree contains a block scalar (LITERAL_SCALAR or
/// FOLDED_SCALAR) are never re-indented: block-scalar content lives at
/// fixed absolute columns, and shifting the surrounding key would
/// decouple the visual alignment without changing the parsed value.
fn collect_indent_targets(block: &SyntaxNode, col: usize, out: &mut Vec<IndentTarget>) {
    debug_assert!(matches!(
        block.kind(),
        SyntaxKind::BLOCK_MAPPING | SyntaxKind::BLOCK_SEQUENCE
    ));

    // Normalize the leading indent WHITESPACE of every entry that lives
    // on its own line. Per the entry-indent ownership invariant, a
    // line-start entry owns its indent as its first child. An entry is
    // "on its own line" when:
    //   (a) it has a WHITESPACE token as its first child (owns its
    //       indent), OR
    //   (b) the token immediately before it at the collection level is
    //       a NEWLINE (subsequent entries always fall here).
    let children: Vec<_> = block.children_with_tokens().collect();
    for (i, el) in children.iter().enumerate() {
        let NodeOrToken::Node(entry) = el else {
            continue;
        };
        if !is_block_entry(entry) {
            continue;
        }
        if contains_block_scalar(entry) {
            continue;
        }
        let entry_starts_with_ws = matches!(
            entry.children_with_tokens().next(),
            Some(NodeOrToken::Token(ref t)) if t.kind() == SyntaxKind::WHITESPACE
        );
        let preceded_by_newline = i > 0
            && matches!(
                children.get(i - 1),
                Some(NodeOrToken::Token(t)) if t.kind() == SyntaxKind::NEWLINE
            );
        if entry_starts_with_ws || preceded_by_newline {
            plan_entry_indent_fix(entry, col, out);
        }
    }

    // Recurse into each entry, looking for a child-launch block.
    for el in &children {
        let NodeOrToken::Node(entry) = el else {
            continue;
        };
        if !is_block_entry(entry) {
            continue;
        }
        visit_entry(entry, col, out);
    }
}

/// Plan the edit needed to make the leading indent WHITESPACE of `entry`
/// (its first child token, if any) equal to `col` spaces.
fn plan_entry_indent_fix(entry: &SyntaxNode, col: usize, out: &mut Vec<IndentTarget>) {
    let entry_children: Vec<_> = entry.children_with_tokens().collect();
    match entry_children.first() {
        Some(NodeOrToken::Token(ws)) if ws.kind() == SyntaxKind::WHITESPACE => {
            let current = ws.text().len();
            if current == col {
                return;
            }
            if col == 0 {
                out.push(IndentTarget::RemoveWs(ws.clone()));
            } else {
                out.push(IndentTarget::ReplaceWs(ws.clone(), " ".repeat(col)));
            }
        }
        _ => {
            // No leading WHITESPACE token. Insert one if col > 0.
            if col > 0 {
                out.push(IndentTarget::InsertWs {
                    parent: entry.clone(),
                    at: 0,
                    spaces: " ".repeat(col),
                });
            }
        }
    }
}

/// Inside a block entry, find a nested BLOCK_MAPPING/BLOCK_SEQUENCE that
/// was launched on a fresh line (preceded by NEWLINE [WHITESPACE?]), fix
/// its launch indent, and recurse.
fn visit_entry(entry: &SyntaxNode, parent_col: usize, out: &mut Vec<IndentTarget>) {
    let entry_has_block_scalar = contains_block_scalar(entry);
    let children: Vec<_> = entry.children_with_tokens().collect();

    // Collapse intra-entry separator whitespace (between the entry marker
    // DASH/COLON and its immediate value token) to a single space.
    // This fires for scalar values as well as nested block collections.
    if !entry_has_block_scalar {
        collapse_intra_entry_whitespace(&children, out);
    }

    for (i, el) in children.iter().enumerate() {
        let NodeOrToken::Node(child) = el else {
            continue;
        };
        let k = child.kind();
        if !matches!(k, SyntaxKind::BLOCK_MAPPING | SyntaxKind::BLOCK_SEQUENCE) {
            continue;
        }
        // Is this block launched on its own line?
        if let Some((_nl_idx, ws_idx)) = preceding_newline_and_ws(&children, i) {
            // Determine the canonical column for entries of this child block.
            // Because each entry owns its leading indent WHITESPACE,
            // `observed_block_col` returns 0 for the collection itself.
            // Instead read the actual indent from the first entry.
            // `None` means the first entry has no leading WHITESPACE — it's
            // a genuinely indentless sequence at col 0, keep it there.
            // `Some(_)` means it's explicitly indented — normalise to parent+2.
            let child_col = match first_entry_col(child) {
                None => 0,
                Some(_) => parent_col + 2,
            };
            // Don't re-indent the launch column when a block scalar lives
            // somewhere in this entry's subtree; its fixed-column content
            // could become visually detached.
            if !entry_has_block_scalar {
                if ws_idx != i {
                    // WS sibling exists at parent level — adjust it.
                    plan_indent_fix(entry, &children, ws_idx, i, child_col, out);
                }
                // When ws_idx == i there is no WS at this level; the first
                // entry's own leading WHITESPACE will be normalized by
                // collect_indent_targets below.
                collect_indent_targets(child, child_col, out);
            } else {
                // The entry contains a block scalar so we must not shift
                // anything — pass the *actual* column from the first entry's
                // own leading whitespace so collect_indent_targets sees no
                // diff and leaves every child alone. `observed_block_col`
                // returns 0 here because entries own their indent whitespace
                // (it is no longer a sibling at the collection level).
                // `observed_block_col` would return 0 here because each
                // entry owns its indent whitespace (not a sibling at the
                // collection level), so read it from the first entry.
                let observed = first_entry_col(child).unwrap_or(0);
                collect_indent_targets(child, observed, out);
            }
        } else {
            // Inline continuation (e.g. `- - x`, `- key: val`).
            // The intra-entry separator is already handled by
            // collapse_intra_entry_whitespace above.
            let child_col = if entry_has_block_scalar {
                observed_block_col(child)
            } else {
                // Inline child: walk preceding tokens within the entry
                // on the same line to measure its offset from the entry
                // marker (DASH or key), collapsing any WHITESPACE > 1 to
                // a single space.
                //
                // The first child may be a leading indent WHITESPACE.
                // Skip it when measuring intra-entry offset since
                // `parent_col` already represents the entry marker column.
                let leading_ws_skip = matches!(
                    children.first(),
                    Some(NodeOrToken::Token(t)) if t.kind() == SyntaxKind::WHITESPACE
                );
                let intra_start = if leading_ws_skip { 1 } else { 0 };
                let mut intra = 0usize;
                for el in children[intra_start..i].iter() {
                    let width = match el {
                        NodeOrToken::Token(t) => {
                            if t.kind() == SyntaxKind::NEWLINE {
                                intra = 0;
                                0
                            } else if t.kind() == SyntaxKind::WHITESPACE && t.text().len() > 1 {
                                1
                            } else {
                                t.text().len()
                            }
                        }
                        NodeOrToken::Node(n) => n.text().to_string().len(),
                    };
                    intra += width;
                }
                parent_col + intra
            };
            collect_indent_targets(child, child_col, out);
        }
    }
}

/// Collapse any WHITESPACE token that immediately follows the entry marker
/// (DASH for sequences, COLON for mappings) to a single space, but only
/// when the value is inline (no NEWLINE between marker and value).
fn collapse_intra_entry_whitespace(
    children: &[rowan::NodeOrToken<SyntaxNode, SyntaxToken>],
    out: &mut Vec<IndentTarget>,
) {
    // Find the marker token (DASH or COLON), then check what follows.
    let mut saw_marker = false;
    for el in children {
        match el {
            NodeOrToken::Token(t) if matches!(t.kind(), SyntaxKind::DASH | SyntaxKind::COLON) => {
                saw_marker = true;
            }
            NodeOrToken::Token(t) if saw_marker && t.kind() == SyntaxKind::NEWLINE => {
                // Value is on the next line — don't touch the spacing here.
                break;
            }
            NodeOrToken::Token(t)
                if saw_marker && t.kind() == SyntaxKind::WHITESPACE && t.text().len() > 1 =>
            {
                out.push(IndentTarget::ReplaceWs(t.clone(), " ".to_string()));
                break;
            }
            _ if saw_marker => break,
            _ => {}
        }
    }
}

/// Return the leading-indent column of the first block entry inside `block`,
/// or `None` if the first entry has no leading WHITESPACE (genuinely at col 0,
/// i.e. an indentless sequence). `Some(n)` means the entry is explicitly
/// indented with `n` spaces and should be normalised; `None` means keep at 0.
fn first_entry_col(block: &SyntaxNode) -> Option<usize> {
    for el in block.children() {
        if !is_block_entry(&el) {
            continue;
        }
        return match el.children_with_tokens().next() {
            Some(NodeOrToken::Token(t)) if t.kind() == SyntaxKind::WHITESPACE => {
                Some(t.text().len())
            }
            _ => None,
        };
    }
    None
}

/// Given `children[i]` is the target element, look backwards for the
/// most recent NEWLINE token. Return `(newline_index, ws_index)` where
/// `ws_index` points at a WHITESPACE token between newline and target
/// if present, else equals `i` (meaning "no whitespace — insert here").
/// Returns `None` if no NEWLINE is found (target is inline).
fn preceding_newline_and_ws(
    children: &[rowan::NodeOrToken<SyntaxNode, SyntaxToken>],
    i: usize,
) -> Option<(usize, usize)> {
    if i == 0 {
        return None;
    }
    let prev = &children[i - 1];
    // Case A: NEWLINE immediately before target — no whitespace, needs insert.
    if let NodeOrToken::Token(t) = prev {
        if t.kind() == SyntaxKind::NEWLINE {
            return Some((i - 1, i));
        }
        // Case B: WHITESPACE, then we need a NEWLINE one step further back.
        if t.kind() == SyntaxKind::WHITESPACE
            && i >= 2
            && let NodeOrToken::Token(prev2) = &children[i - 2]
            && prev2.kind() == SyntaxKind::NEWLINE
        {
            return Some((i - 2, i - 1));
        }
    }
    None
}

/// Record the edit needed to make the indent before `children[target_idx]`
/// be exactly `col` spaces. `ws_idx == target_idx` means there's no
/// existing WHITESPACE and we must insert.
fn plan_indent_fix(
    parent: &SyntaxNode,
    children: &[rowan::NodeOrToken<SyntaxNode, SyntaxToken>],
    ws_idx: usize,
    target_idx: usize,
    col: usize,
    out: &mut Vec<IndentTarget>,
) {
    if ws_idx == target_idx {
        // No existing WHITESPACE. Insert one if col > 0.
        if col > 0 {
            out.push(IndentTarget::InsertWs {
                parent: parent.clone(),
                at: target_idx,
                spaces: " ".repeat(col),
            });
        }
        return;
    }
    let NodeOrToken::Token(ws) = &children[ws_idx] else {
        return;
    };
    debug_assert_eq!(ws.kind(), SyntaxKind::WHITESPACE);
    let current = ws.text().len();
    if current == col {
        return;
    }
    if col == 0 {
        out.push(IndentTarget::RemoveWs(ws.clone()));
    } else {
        out.push(IndentTarget::ReplaceWs(ws.clone(), " ".repeat(col)));
    }
}

/// Remove unnecessary quotes from single- and double-quoted scalars whose
/// content is safe as a plain scalar.
///
/// Single-quoted: safe to unquote iff the content contains no `''` (which
/// encodes a literal `'` and can never be plain).
/// Double-quoted: safe to unquote iff the content contains no `\` (escape
/// sequences encode characters that would otherwise require quoting).
pub fn unquote_scalars(tree: &mut SyntaxTree) -> usize {
    let mut targets: Vec<(SyntaxNode, String)> = Vec::new();
    for ev in tree.root().preorder_with_tokens() {
        let WalkEvent::Enter(NodeOrToken::Token(t)) = ev else {
            continue;
        };
        let plain = match t.kind() {
            SyntaxKind::SINGLE_QUOTED_SCALAR => {
                let inner = t.text().trim_matches('\'');
                if inner.contains("''") {
                    continue;
                }
                inner.to_string()
            }
            SyntaxKind::DOUBLE_QUOTED_SCALAR => {
                let inner = t.text().trim_matches('"');
                if inner.contains('\\') {
                    continue;
                }
                inner.to_string()
            }
            _ => continue,
        };
        let Some(plain) = as_plain_scalar(&plain).map(str::to_string) else {
            continue;
        };
        let Some(scalar) = t.parent() else { continue };
        if scalar.kind() != SyntaxKind::SCALAR {
            continue;
        }
        targets.push((scalar, plain));
    }
    let n = targets.len();
    for (scalar, plain) in targets {
        let new = mk_node(
            SyntaxKind::SCALAR,
            [NodeOrToken::Token(mk_token(
                SyntaxKind::PLAIN_SCALAR,
                &plain,
            ))],
        );
        apply_edit(
            tree,
            TreeEdit::ReplaceNode {
                target: scalar,
                new,
            },
        );
    }
    n
}

/// Convert double-quoted scalars to single-quoted where possible.
///
/// Safe when the decoded content contains no `'` and no control characters.
/// Double-quoted scalars with `\` escape sequences are skipped (the decoded
/// value may differ from the raw text).
pub fn prefer_single_quotes(tree: &mut SyntaxTree) -> usize {
    let mut targets: Vec<(SyntaxNode, String)> = Vec::new();
    for ev in tree.root().preorder_with_tokens() {
        let WalkEvent::Enter(NodeOrToken::Token(t)) = ev else {
            continue;
        };
        if t.kind() != SyntaxKind::DOUBLE_QUOTED_SCALAR {
            continue;
        }
        let inner = t.text().trim_matches('"');
        if inner.contains('\\') {
            continue;
        }
        let Some(quoted) = as_single_quoted(inner) else {
            continue;
        };
        let Some(scalar) = t.parent() else { continue };
        if scalar.kind() != SyntaxKind::SCALAR {
            continue;
        }
        targets.push((scalar, quoted));
    }
    let n = targets.len();
    for (scalar, quoted) in targets {
        let new = mk_node(
            SyntaxKind::SCALAR,
            [NodeOrToken::Token(mk_token(
                SyntaxKind::SINGLE_QUOTED_SCALAR,
                &quoted,
            ))],
        );
        apply_edit(
            tree,
            TreeEdit::ReplaceNode {
                target: scalar,
                new,
            },
        );
    }
    n
}
