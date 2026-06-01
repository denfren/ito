use crate::ast::{
    AstNode, Document, Mapping, Node, NodeProperties, Scalar, ScalarStyle, Sequence, Stream,
};
use crate::{SyntaxElement, SyntaxNode, SyntaxTree};

pub fn cst_string(tree: &SyntaxTree) -> String {
    let mut out = String::new();
    cst_node(tree.root(), 0, &mut out);
    out
}

fn cst_node(node: &SyntaxNode, depth: usize, out: &mut String) {
    let range = node.text_range();
    let indent = "  ".repeat(depth);
    out.push_str(&format!(
        "{indent}{kind:?}@{start}..{end}\n",
        kind = node.kind(),
        start = usize::from(range.start()),
        end = usize::from(range.end()),
    ));
    for child in node.children_with_tokens() {
        match child {
            SyntaxElement::Node(n) => cst_node(&n, depth + 1, out),
            SyntaxElement::Token(t) => {
                let r = t.text_range();
                let child_indent = "  ".repeat(depth + 1);
                out.push_str(&format!(
                    "{child_indent}{kind:?}@{start}..{end} {text}\n",
                    kind = t.kind(),
                    start = usize::from(r.start()),
                    end = usize::from(r.end()),
                    text = escape(t.text()),
                ));
            }
        }
    }
}

pub fn ast_string(tree: &SyntaxTree) -> String {
    let mut out = String::new();
    let Some(stream) = Stream::cast(tree.root().clone()) else {
        return "Stream (no root)\n".into();
    };
    out.push_str("Stream\n");
    for doc in stream.documents() {
        ast_document(&doc, 1, &mut out);
    }
    out
}

fn ast_document(doc: &Document, depth: usize, out: &mut String) {
    let indent = "  ".repeat(depth);
    out.push_str(&format!("{indent}Document\n"));
    if let Some(dirs) = doc.directives() {
        let child_indent = "  ".repeat(depth + 1);
        out.push_str(&format!("{child_indent}Directives\n"));
        for d in dirs.directives() {
            let dd_indent = "  ".repeat(depth + 2);
            out.push_str(&format!("{dd_indent}Directive {}\n", escape(d.text())));
        }
    }
    if let Some(root) = doc.root_node() {
        ast_node(&root, depth + 1, out);
    }
}

fn ast_node(node: &Node, depth: usize, out: &mut String) {
    match node {
        Node::Scalar(s) => ast_scalar(s, depth, out),
        Node::BlockMapping(m) => ast_mapping("BlockMapping", m, depth, out),
        Node::FlowMapping(m) => ast_mapping("FlowMapping", m, depth, out),
        Node::BlockSequence(s) => ast_sequence("BlockSequence", s, depth, out),
        Node::FlowSequence(s) => ast_sequence("FlowSequence", s, depth, out),
        Node::Alias(a) => {
            let indent = "  ".repeat(depth);
            let name = a.name().unwrap_or_default();
            out.push_str(&format!("{indent}Alias *{name}\n"));
        }
    }
}

fn ast_scalar(s: &Scalar, depth: usize, out: &mut String) {
    let indent = "  ".repeat(depth);
    if s.is_null() {
        out.push_str(&format!("{indent}Null\n"));
        if let Some(props) = s.properties() {
            ast_properties(&props, depth + 1, out);
        }
        return;
    }
    let style = match s.style() {
        Some(ScalarStyle::Plain) => "plain",
        Some(ScalarStyle::SingleQuoted) => "single",
        Some(ScalarStyle::DoubleQuoted) => "double",
        Some(ScalarStyle::Literal) => "literal",
        Some(ScalarStyle::Folded) => "folded",
        None => "?",
    };
    let raw = s.raw_text().unwrap_or_default();
    out.push_str(&format!("{indent}Scalar [{style}] {}\n", escape(&raw)));
    if let Some(props) = s.properties() {
        ast_properties(&props, depth + 1, out);
    }
    if let Ok(decoded) = s.decoded() {
        let value = s
            .to_value()
            .ok()
            .map(|v| format!("{v:?}"))
            .unwrap_or_else(|| "?".into());
        let child_indent = "  ".repeat(depth + 1);
        out.push_str(&format!(
            "{child_indent}decoded={} value={value}\n",
            escape(&decoded)
        ));
    }
}

fn ast_mapping<M: Mapping>(label: &str, m: &M, depth: usize, out: &mut String) {
    let indent = "  ".repeat(depth);
    out.push_str(&format!("{indent}{label} ({} entries)\n", m.len()));
    let entry_indent = "  ".repeat(depth + 1);
    for (k, v) in m.pairs() {
        out.push_str(&format!("{entry_indent}Entry\n"));
        let kv_indent = "  ".repeat(depth + 2);
        out.push_str(&format!("{kv_indent}Key\n"));
        ast_node(&k, depth + 3, out);
        match v {
            Some(v) => {
                out.push_str(&format!("{kv_indent}Value\n"));
                ast_node(&v, depth + 3, out);
            }
            None => out.push_str(&format!("{kv_indent}Value (none)\n")),
        }
    }
}

fn ast_sequence<S: Sequence>(label: &str, s: &S, depth: usize, out: &mut String) {
    let indent = "  ".repeat(depth);
    out.push_str(&format!("{indent}{label} ({} entries)\n", s.len()));
    for v in s.iter() {
        ast_node(&v, depth + 1, out);
    }
}

fn ast_properties(p: &NodeProperties, depth: usize, out: &mut String) {
    let indent = "  ".repeat(depth);
    out.push_str(&format!("{indent}NodeProperties\n"));
    let child_indent = "  ".repeat(depth + 1);
    if let Some(a) = p.anchor_name() {
        out.push_str(&format!("{child_indent}anchor &{a}\n"));
    }
    if let Some(t) = p.tag_value() {
        out.push_str(&format!("{child_indent}tag {t:?}\n"));
    }
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\x{:02x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
