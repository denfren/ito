//! Value-model builder (Phase 3, strategy A).
//!
//! Build a YAML CST by constructing a [`Yaml`] value model, serializing
//! it to canonical YAML **text**, and running that text through
//! [`crate::parse`]. This reuses the parser's indentation and structure
//! rules exactly once, so a built tree is guaranteed to be
//! parse-consistent and to round-trip through `emit()`.
//!
//! For surgical, style-preserving construction use the green-node escape
//! hatch (`edit::mk_node` / `edit::mk_token`) instead.

use crate::ast::value::infer_plain;
use crate::ast::{AstNode, Node, Stream, Value};
use crate::{SyntaxTree, parse};

/// A YAML value to build into a tree. Mapping keys are strings (the only
/// key shape the builder emits); richer keys need the green-node hatch.
#[derive(Debug, Clone, PartialEq)]
pub enum Yaml {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Seq(Vec<Yaml>),
    Map(Vec<(String, Yaml)>),
}

impl Yaml {
    /// Serialize to canonical block YAML text (always ends in `\n`).
    pub fn to_yaml_string(&self) -> String {
        let mut out = String::new();
        write_node(self, 0, &mut out);
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out
    }

    /// Build a full document tree from this value.
    pub fn build(&self) -> SyntaxTree {
        // The serializer only emits canonical YAML, so this never fails.
        parse(&self.to_yaml_string()).expect("builder produced unparseable YAML")
    }

    /// Decode the (single) document of a parsed tree into a `Yaml` value.
    ///
    /// This applies the YAML 1.2 Core schema to scalars (`Scalar::to_value`)
    /// and walks block/flow collections — the read direction (text → tree →
    /// value). It does not aim to round-trip with [`Yaml::to_yaml_string`]:
    /// emitting is free to choose its own canonical layout.
    ///
    /// Errors:
    /// - more than one document in the stream (no single value to return);
    /// - an alias node (yamlito does not resolve anchors into values).
    pub fn from_tree(tree: &SyntaxTree) -> Result<Yaml, String> {
        let stream = Stream::cast(tree.root().clone()).ok_or("not a YAML stream")?;
        let mut docs = stream.documents();
        let Some(doc) = docs.next() else {
            // An empty stream (e.g. comments only) is a null document.
            return Ok(Yaml::Null);
        };
        if docs.next().is_some() {
            return Err("multi-document streams are not supported".to_string());
        }
        doc_value(&doc)
    }

    /// Decode every document of a parsed tree into a `Yaml` value, in
    /// source order. Unlike [`Yaml::from_tree`] this accepts multi-document
    /// streams (`---` separators); each document becomes one element.
    ///
    /// An empty stream (no documents) yields an empty vector. Aliases are
    /// still rejected (see [`Yaml::from_tree`]).
    pub fn from_stream(tree: &SyntaxTree) -> Result<Vec<Yaml>, String> {
        let stream = Stream::cast(tree.root().clone()).ok_or("not a YAML stream")?;
        stream.documents().map(|doc| doc_value(&doc)).collect()
    }

    /// Render a slice of values as a multi-document YAML stream. Each
    /// document is canonical block YAML (as [`Yaml::to_yaml_string`]); the
    /// second and later documents are introduced by a `---` marker line.
    /// An empty slice renders the empty string.
    pub fn to_multi_string(docs: &[Yaml]) -> String {
        let mut out = String::new();
        for (i, doc) in docs.iter().enumerate() {
            if i > 0 {
                out.push_str("---\n");
            }
            out.push_str(&doc.to_yaml_string());
        }
        out
    }
}

/// Decode a single document's root node into a `Yaml` value. An empty
/// document (e.g. `---` with no body) decodes to null.
fn doc_value(doc: &crate::ast::Document) -> Result<Yaml, String> {
    match doc.root_node() {
        Some(node) => from_node(&node),
        None => Ok(Yaml::Null),
    }
}

/// Convert one AST node into a `Yaml` value, recursing into collections.
fn from_node(node: &Node) -> Result<Yaml, String> {
    match node {
        Node::Scalar(s) => match s
            .to_value()
            .map_err(|e| format!("scalar decode error: {e:?}"))?
        {
            Value::Null => Ok(Yaml::Null),
            Value::Bool(b) => Ok(Yaml::Bool(b)),
            Value::Int(n) => Ok(Yaml::Int(n)),
            Value::Float(f) => Ok(Yaml::Float(f)),
            Value::String(t) => Ok(Yaml::Str(t)),
        },
        Node::BlockSequence(_) | Node::FlowSequence(_) => {
            let seq = node.as_sequence().expect("sequence node");
            let mut items = Vec::new();
            for item in seq.iter() {
                items.push(from_node(&item)?);
            }
            Ok(Yaml::Seq(items))
        }
        Node::BlockMapping(_) | Node::FlowMapping(_) => {
            let map = node.as_mapping().expect("mapping node");
            let mut pairs = Vec::new();
            for (key, value) in map.pairs() {
                let key = match &key {
                    Node::Scalar(s) => s
                        .decoded()
                        .map_err(|e| format!("mapping key decode error: {e:?}"))?,
                    _ => return Err("non-scalar mapping keys are not supported".to_string()),
                };
                let value = match value {
                    Some(v) => from_node(&v)?,
                    None => Yaml::Null,
                };
                pairs.push((key, value));
            }
            Ok(Yaml::Map(pairs))
        }
        Node::Alias(_) => Err("alias nodes are not supported".to_string()),
    }
}

// --- serde ---
//
// `Yaml` is the value model used to bridge to/from external data (e.g. a
// Rhai `Dynamic`, via `rhai::serde`). Serialization is a direct mapping of
// the variants; deserialization is a `Visitor` that accepts the data model
// any serde source produces. Mapping keys must be strings — the only key
// shape `Yaml` (and the builder) supports.

impl serde::Serialize for Yaml {
    fn serialize<S: serde::Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        use serde::ser::{SerializeMap, SerializeSeq};
        match self {
            Yaml::Null => ser.serialize_unit(),
            Yaml::Bool(b) => ser.serialize_bool(*b),
            Yaml::Int(n) => ser.serialize_i64(*n),
            Yaml::Float(f) => ser.serialize_f64(*f),
            Yaml::Str(s) => ser.serialize_str(s),
            Yaml::Seq(items) => {
                let mut seq = ser.serialize_seq(Some(items.len()))?;
                for item in items {
                    seq.serialize_element(item)?;
                }
                seq.end()
            }
            Yaml::Map(pairs) => {
                let mut map = ser.serialize_map(Some(pairs.len()))?;
                for (k, v) in pairs {
                    map.serialize_entry(k, v)?;
                }
                map.end()
            }
        }
    }
}

impl<'de> serde::Deserialize<'de> for Yaml {
    fn deserialize<D: serde::Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        de.deserialize_any(YamlVisitor)
    }
}

struct YamlVisitor;

impl<'de> serde::de::Visitor<'de> for YamlVisitor {
    type Value = Yaml;

    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("any YAML value")
    }

    fn visit_bool<E>(self, v: bool) -> Result<Yaml, E> {
        Ok(Yaml::Bool(v))
    }
    fn visit_i64<E>(self, v: i64) -> Result<Yaml, E> {
        Ok(Yaml::Int(v))
    }
    fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<Yaml, E> {
        i64::try_from(v)
            .map(Yaml::Int)
            .map_err(|_| E::custom("integer out of range for i64"))
    }
    fn visit_f64<E>(self, v: f64) -> Result<Yaml, E> {
        Ok(Yaml::Float(v))
    }
    fn visit_str<E>(self, v: &str) -> Result<Yaml, E> {
        Ok(Yaml::Str(v.to_string()))
    }
    fn visit_string<E>(self, v: String) -> Result<Yaml, E> {
        Ok(Yaml::Str(v))
    }
    fn visit_none<E>(self) -> Result<Yaml, E> {
        Ok(Yaml::Null)
    }
    fn visit_unit<E>(self) -> Result<Yaml, E> {
        Ok(Yaml::Null)
    }
    fn visit_some<D: serde::Deserializer<'de>>(self, de: D) -> Result<Yaml, D::Error> {
        de.deserialize_any(YamlVisitor)
    }

    fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<Yaml, A::Error> {
        let mut items = Vec::new();
        while let Some(item) = seq.next_element()? {
            items.push(item);
        }
        Ok(Yaml::Seq(items))
    }

    fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<Yaml, A::Error> {
        let mut pairs = Vec::new();
        while let Some((k, v)) = map.next_entry::<String, Yaml>()? {
            pairs.push((k, v));
        }
        Ok(Yaml::Map(pairs))
    }
}

/// Write `node` as a block-style value. `indent` is the column at which
/// nested block content (map keys, sequence dashes) is placed.
fn write_node(node: &Yaml, indent: usize, out: &mut String) {
    match node {
        Yaml::Null => out.push_str("null"),
        Yaml::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Yaml::Int(n) => out.push_str(&n.to_string()),
        Yaml::Float(f) => out.push_str(&format_float(*f)),
        Yaml::Str(s) => out.push_str(&quote_scalar(s)),
        Yaml::Seq(items) => write_seq(items, indent, out),
        Yaml::Map(pairs) => write_map(pairs, indent, out),
    }
}

fn write_seq(items: &[Yaml], indent: usize, out: &mut String) {
    if items.is_empty() {
        out.push_str("[]");
        return;
    }
    for (i, item) in items.iter().enumerate() {
        if i > 0 || out.ends_with('\n') {
            push_indent(indent, out);
        }
        out.push('-');
        match item {
            // Compact block form: `- key: val` with the first entry on
            // the dash line and the rest aligned two columns in.
            Yaml::Map(pairs) if !pairs.is_empty() => {
                out.push(' ');
                write_map(pairs, indent + 2, out);
            }
            other => write_block_child(other, indent, out),
        }
        out.push('\n');
    }
    // Trim: the loop adds a trailing newline; callers handle separators.
    out.pop();
}

fn write_map(pairs: &[(String, Yaml)], indent: usize, out: &mut String) {
    if pairs.is_empty() {
        out.push_str("{}");
        return;
    }
    for (i, (key, value)) in pairs.iter().enumerate() {
        if i > 0 || out.ends_with('\n') {
            push_indent(indent, out);
        }
        out.push_str(&quote_scalar(key));
        out.push(':');
        write_block_child(value, indent, out);
        out.push('\n');
    }
    out.pop();
}

/// Write the value side of a `-` or `key:` prefix. Scalars and empty
/// collections go inline after a space; non-empty collections go on the
/// following lines indented by two.
fn write_block_child(value: &Yaml, indent: usize, out: &mut String) {
    match value {
        Yaml::Seq(items) if !items.is_empty() => {
            out.push('\n');
            push_indent(indent + 2, out);
            write_seq(items, indent + 2, out);
        }
        Yaml::Map(pairs) if !pairs.is_empty() => {
            out.push('\n');
            push_indent(indent + 2, out);
            write_map(pairs, indent + 2, out);
        }
        scalar_or_empty => {
            out.push(' ');
            write_node(scalar_or_empty, indent + 2, out);
        }
    }
}

fn push_indent(n: usize, out: &mut String) {
    for _ in 0..n {
        out.push(' ');
    }
}

/// Render an `f64` so it round-trips and is unambiguously a float.
fn format_float(f: f64) -> String {
    if f.is_nan() {
        return ".nan".to_string();
    }
    if f.is_infinite() {
        return if f < 0.0 { "-.inf" } else { ".inf" }.to_string();
    }
    let s = f.to_string();
    // Ensure a `.` or `e` is present so the Core schema infers a float.
    if s.contains('.') || s.contains('e') || s.contains('E') {
        s
    } else {
        format!("{s}.0")
    }
}

/// Quote a string scalar when emitting it plain would change its meaning
/// (re-inferred as null/bool/number) or break parsing. Otherwise emit it
/// plain.
fn quote_scalar(s: &str) -> String {
    if needs_quoting(s) {
        single_quote(s)
    } else {
        s.to_string()
    }
}

fn needs_quoting(s: &str) -> bool {
    if s.is_empty() {
        return true;
    }
    // Would a plain scalar re-infer as a non-string? Then it must be quoted.
    if !matches!(infer_plain(s), Value::String(_)) {
        return true;
    }
    // Leading indicator characters or structural bytes make a plain
    // scalar ambiguous or invalid.
    let first = s.as_bytes()[0];
    if matches!(
        first,
        b'-' | b'?'
            | b':'
            | b','
            | b'['
            | b']'
            | b'{'
            | b'}'
            | b'#'
            | b'&'
            | b'*'
            | b'!'
            | b'|'
            | b'>'
            | b'\''
            | b'"'
            | b'%'
            | b'@'
            | b'`'
            | b' '
    ) {
        return true;
    }
    // Interior `: ` (mapping indicator), ` #` (comment), trailing space,
    // or any newline/tab would all break a plain scalar.
    if s.ends_with(' ')
        || s.contains('\n')
        || s.contains('\t')
        || s.contains(": ")
        || s.ends_with(':')
        || s.contains(" #")
    {
        return true;
    }
    false
}

/// Single-quote a string, doubling any embedded single quotes.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{AstNode, Mapping, Node, Sequence, Stream};

    fn build_str(y: &Yaml) -> String {
        y.to_yaml_string()
    }

    /// Build, then re-parse and confirm the tree round-trips.
    fn assert_round_trips(y: &Yaml) -> SyntaxTree {
        let tree = y.build();
        let text = y.to_yaml_string();
        assert_eq!(tree.emit(), text, "built tree must round-trip");
        tree
    }

    #[test]
    fn scalar_kinds() {
        assert_eq!(build_str(&Yaml::Null), "null\n");
        assert_eq!(build_str(&Yaml::Bool(true)), "true\n");
        assert_eq!(build_str(&Yaml::Int(42)), "42\n");
        assert_eq!(build_str(&Yaml::Float(1.5)), "1.5\n");
        assert_eq!(build_str(&Yaml::Str("hello".into())), "hello\n");
    }

    #[test]
    fn float_gets_decimal_point() {
        assert_eq!(build_str(&Yaml::Float(3.0)), "3.0\n");
        assert_eq!(build_str(&Yaml::Float(f64::INFINITY)), ".inf\n");
    }

    #[test]
    fn ambiguous_strings_quoted() {
        assert_eq!(build_str(&Yaml::Str("true".into())), "'true'\n");
        assert_eq!(build_str(&Yaml::Str("123".into())), "'123'\n");
        assert_eq!(build_str(&Yaml::Str("null".into())), "'null'\n");
        assert_eq!(build_str(&Yaml::Str("".into())), "''\n");
        assert_eq!(build_str(&Yaml::Str("a: b".into())), "'a: b'\n");
        assert_eq!(build_str(&Yaml::Str("- x".into())), "'- x'\n");
    }

    #[test]
    fn single_quote_escaping() {
        // A leading apostrophe forces quoting; embedded quotes double.
        assert_eq!(build_str(&Yaml::Str("'tis".into())), "'''tis'\n");
        // Mid-word apostrophe is valid plain — left unquoted.
        assert_eq!(build_str(&Yaml::Str("it's".into())), "it's\n");
    }

    #[test]
    fn simple_map() {
        let y = Yaml::Map(vec![
            ("a".into(), Yaml::Int(1)),
            ("b".into(), Yaml::Str("x".into())),
        ]);
        assert_eq!(build_str(&y), "a: 1\nb: x\n");
        assert_round_trips(&y);
    }

    #[test]
    fn nested_map() {
        let y = Yaml::Map(vec![(
            "outer".into(),
            Yaml::Map(vec![("inner".into(), Yaml::Int(1))]),
        )]);
        assert_eq!(build_str(&y), "outer:\n  inner: 1\n");
        assert_round_trips(&y);
    }

    #[test]
    fn simple_seq() {
        let y = Yaml::Seq(vec![Yaml::Int(1), Yaml::Int(2)]);
        assert_eq!(build_str(&y), "- 1\n- 2\n");
        assert_round_trips(&y);
    }

    #[test]
    fn seq_of_maps() {
        let y = Yaml::Seq(vec![
            Yaml::Map(vec![("host".into(), Yaml::Str("alpha".into()))]),
            Yaml::Map(vec![("host".into(), Yaml::Str("beta".into()))]),
        ]);
        assert_eq!(build_str(&y), "- host: alpha\n- host: beta\n");
        assert_round_trips(&y);
    }

    #[test]
    fn map_with_seq_value() {
        let y = Yaml::Map(vec![(
            "tags".into(),
            Yaml::Seq(vec![Yaml::Str("web".into()), Yaml::Str("api".into())]),
        )]);
        assert_eq!(build_str(&y), "tags:\n  - web\n  - api\n");
        assert_round_trips(&y);
    }

    #[test]
    fn empty_collections_inline() {
        assert_eq!(build_str(&Yaml::Seq(vec![])), "[]\n");
        assert_eq!(build_str(&Yaml::Map(vec![])), "{}\n");
        let y = Yaml::Map(vec![("k".into(), Yaml::Seq(vec![]))]);
        assert_eq!(build_str(&y), "k: []\n");
    }

    #[test]
    fn deeply_nested_round_trips() {
        let y = Yaml::Map(vec![(
            "config".into(),
            Yaml::Map(vec![
                (
                    "servers".into(),
                    Yaml::Seq(vec![Yaml::Map(vec![
                        ("host".into(), Yaml::Str("alpha".into())),
                        ("port".into(), Yaml::Int(80)),
                    ])]),
                ),
                ("tags".into(), Yaml::Seq(vec![Yaml::Str("web".into())])),
            ]),
        )]);
        let tree = assert_round_trips(&y);
        // Spot-check the parsed structure.
        let stream = Stream::cast(tree.root().clone()).unwrap();
        let doc = stream.documents().next().unwrap();
        let Node::BlockMapping(root) = doc.root_node().unwrap() else {
            panic!("expected block mapping root");
        };
        let config = root.get("config").unwrap();
        let Node::BlockMapping(config) = config else {
            panic!("expected nested mapping");
        };
        assert_eq!((&config as &dyn Mapping).len(), 2);
        let servers = config.get("servers").unwrap();
        let Node::BlockSequence(servers) = servers else {
            panic!("expected sequence");
        };
        assert_eq!((&servers as &dyn Sequence).len(), 1);
    }

    fn from_yaml(src: &str) -> Yaml {
        Yaml::from_tree(&crate::parse(src).expect("parse")).expect("from_tree")
    }

    fn from_stream(src: &str) -> Vec<Yaml> {
        Yaml::from_stream(&crate::parse(src).expect("parse")).expect("from_stream")
    }

    #[test]
    fn from_stream_single_document() {
        assert_eq!(
            from_stream("a: 1\n"),
            vec![Yaml::Map(vec![("a".into(), Yaml::Int(1))])]
        );
    }

    #[test]
    fn from_stream_multiple_documents() {
        assert_eq!(
            from_stream("a: 1\n---\nb: 2\n"),
            vec![
                Yaml::Map(vec![("a".into(), Yaml::Int(1))]),
                Yaml::Map(vec![("b".into(), Yaml::Int(2))]),
            ]
        );
    }

    #[test]
    fn from_stream_empty_is_empty_vec() {
        assert_eq!(from_stream("# comment only\n"), Vec::<Yaml>::new());
    }

    #[test]
    fn from_stream_rejects_alias() {
        let tree = crate::parse("a: &x 1\n---\nb: *x\n").unwrap();
        let err = Yaml::from_stream(&tree).unwrap_err();
        assert!(err.contains("alias"), "got {err:?}");
    }

    #[test]
    fn to_multi_string_separates_documents() {
        let docs = vec![
            Yaml::Map(vec![("a".into(), Yaml::Int(1))]),
            Yaml::Map(vec![("b".into(), Yaml::Int(2))]),
        ];
        assert_eq!(Yaml::to_multi_string(&docs), "a: 1\n---\nb: 2\n");
    }

    #[test]
    fn to_multi_string_empty_is_empty() {
        assert_eq!(Yaml::to_multi_string(&[]), "");
    }

    #[test]
    fn multi_value_survives_emit_and_reparse() {
        let docs = from_stream("a: 1\n---\nb:\n  - x\n  - y\n");
        let reparsed = from_stream(&Yaml::to_multi_string(&docs));
        assert_eq!(docs, reparsed);
    }

    #[test]
    fn from_tree_infers_scalar_types() {
        assert_eq!(from_yaml("42\n"), Yaml::Int(42));
        assert_eq!(from_yaml("true\n"), Yaml::Bool(true));
        assert_eq!(from_yaml("1.5\n"), Yaml::Float(1.5));
        assert_eq!(from_yaml("hello\n"), Yaml::Str("hello".into()));
        assert_eq!(from_yaml("null\n"), Yaml::Null);
        // Quoted scalars stay strings even when they look like numbers.
        assert_eq!(from_yaml("'42'\n"), Yaml::Str("42".into()));
    }

    #[test]
    fn from_tree_walks_collections() {
        let y = from_yaml("name: web\nports:\n  - 80\n  - 443\nmeta:\n  a: 1\n");
        assert_eq!(
            y,
            Yaml::Map(vec![
                ("name".into(), Yaml::Str("web".into())),
                (
                    "ports".into(),
                    Yaml::Seq(vec![Yaml::Int(80), Yaml::Int(443)])
                ),
                ("meta".into(), Yaml::Map(vec![("a".into(), Yaml::Int(1))])),
            ])
        );
    }

    #[test]
    fn from_tree_flow_collections() {
        assert_eq!(
            from_yaml("{a: 1, b: [x, y]}\n"),
            Yaml::Map(vec![
                ("a".into(), Yaml::Int(1)),
                (
                    "b".into(),
                    Yaml::Seq(vec![Yaml::Str("x".into()), Yaml::Str("y".into())])
                ),
            ])
        );
    }

    #[test]
    fn from_tree_empty_value_is_null() {
        assert_eq!(
            from_yaml("key:\n"),
            Yaml::Map(vec![("key".into(), Yaml::Null)])
        );
    }

    #[test]
    fn from_tree_empty_stream_is_null() {
        assert_eq!(from_yaml("# just a comment\n"), Yaml::Null);
    }

    #[test]
    fn from_tree_rejects_multi_document() {
        let tree = crate::parse("a: 1\n---\nb: 2\n").unwrap();
        let err = Yaml::from_tree(&tree).unwrap_err();
        assert!(err.contains("multi-document"), "got {err:?}");
    }

    #[test]
    fn from_tree_rejects_alias() {
        let tree = crate::parse("a: &x 1\nb: *x\n").unwrap();
        let err = Yaml::from_tree(&tree).unwrap_err();
        assert!(err.contains("alias"), "got {err:?}");
    }

    #[test]
    fn from_tree_reads_through_anchor() {
        // An anchor decorates the value; the value itself is what we read.
        assert_eq!(
            from_yaml("a: &x 1\n"),
            Yaml::Map(vec![("a".into(), Yaml::Int(1))])
        );
    }

    #[test]
    fn value_survives_emit_and_reparse() {
        // parse/to_string are not byte-stable (the library owns the
        // output format), but a value must re-decode to the same value
        // after being emitted and re-parsed.
        let src = "name: web\nports:\n  - 80\n  - 443\nmeta:\n  a: 1\n";
        let y = from_yaml(src);
        let reparsed = from_yaml(&y.to_yaml_string());
        assert_eq!(y, reparsed);
    }

    #[test]
    fn serde_round_trips_through_json() {
        // `Yaml` serializes/deserializes via serde; round-trip through
        // serde_json proves both directions agree on the data model.
        let y = Yaml::Map(vec![
            ("s".into(), Yaml::Str("x".into())),
            ("n".into(), Yaml::Int(7)),
            ("b".into(), Yaml::Bool(true)),
            ("nil".into(), Yaml::Null),
            (
                "list".into(),
                Yaml::Seq(vec![Yaml::Int(1), Yaml::Str("two".into())]),
            ),
        ]);
        let json = serde_json::to_string(&y).expect("serialize");
        let back: Yaml = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(y, back);
    }
}
