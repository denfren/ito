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
    out.push_str(&format!(
        "{}{:?}@{}..{}\n",
        indent(depth),
        node.kind(),
        usize::from(range.start()),
        usize::from(range.end()),
    ));
    for child in node.children_with_tokens() {
        match child {
            SyntaxElement::Node(n) => cst_node(&n, depth + 1, out),
            SyntaxElement::Token(t) => {
                let r = t.text_range();
                out.push_str(&format!(
                    "{}{:?}@{}..{} {}\n",
                    indent(depth + 1),
                    t.kind(),
                    usize::from(r.start()),
                    usize::from(r.end()),
                    escape(t.text()),
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
    out.push_str(&format!("{}Document\n", indent(depth)));
    if let Some(dirs) = doc.directives() {
        out.push_str(&format!("{}Directives\n", indent(depth + 1)));
        for d in dirs.directives() {
            out.push_str(&format!("{}Directive {}\n", indent(depth + 2), escape(d.text())));
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
            out.push_str(&format!("{}Alias *{}\n", indent(depth), a.name().unwrap_or_default()));
        }
    }
}

fn ast_scalar(s: &Scalar, depth: usize, out: &mut String) {
    if s.is_null() {
        out.push_str(&format!("{}Null\n", indent(depth)));
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
    out.push_str(&format!("{}Scalar [{style}] {}\n", indent(depth), escape(&raw)));
    if let Some(props) = s.properties() {
        ast_properties(&props, depth + 1, out);
    }
    if let Ok(decoded) = s.decoded() {
        let value = s
            .to_value()
            .ok()
            .map(|v| format!("{v:?}"))
            .unwrap_or_else(|| "?".into());
        out.push_str(&format!(
            "{}decoded={} value={value}\n",
            indent(depth + 1),
            escape(&decoded)
        ));
    }
}

fn ast_mapping<M: Mapping>(label: &str, m: &M, depth: usize, out: &mut String) {
    out.push_str(&format!("{}{label} ({} entries)\n", indent(depth), m.len()));
    for (k, v) in m.pairs() {
        out.push_str(&format!("{}Entry\n", indent(depth + 1)));
        out.push_str(&format!("{}Key\n", indent(depth + 2)));
        ast_node(&k, depth + 3, out);
        match v {
            Some(v) => {
                out.push_str(&format!("{}Value\n", indent(depth + 2)));
                ast_node(&v, depth + 3, out);
            }
            None => out.push_str(&format!("{}Value (none)\n", indent(depth + 2))),
        }
    }
}

fn ast_sequence<S: Sequence>(label: &str, s: &S, depth: usize, out: &mut String) {
    out.push_str(&format!("{}{label} ({} entries)\n", indent(depth), s.len()));
    for v in s.iter() {
        ast_node(&v, depth + 1, out);
    }
}

fn ast_properties(p: &NodeProperties, depth: usize, out: &mut String) {
    out.push_str(&format!("{}NodeProperties\n", indent(depth)));
    if let Some(a) = p.anchor_name() {
        out.push_str(&format!("{}anchor &{a}\n", indent(depth + 1)));
    }
    if let Some(t) = p.tag_value() {
        out.push_str(&format!("{}tag {t:?}\n", indent(depth + 1)));
    }
}

fn indent(depth: usize) -> String {
    "  ".repeat(depth)
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
