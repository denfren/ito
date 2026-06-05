//! Integration tests for the `hcl::edit` cursor API. Adapted from the
//! `hrs` crate's `tests/functions.rs`, scoped to a single-document root.

use rhai::{Dynamic, Engine};

/// Build an engine with only the `hcl` module registered and evaluate
/// `script`, returning the result as the requested type.
fn eval<T: Clone + 'static>(script: &str) -> T {
    let mut engine = Engine::new();
    ito::scripting::hcl::register(&mut engine);
    engine
        .eval::<T>(script)
        .unwrap_or_else(|e| panic!("eval failed: {e}\nscript:\n{script}"))
}

fn eval_string(script: &str) -> String {
    eval::<String>(script)
}

const DOC: &str = r#"let src = `region = "us-east-1"

resource "aws_instance" "web" {
  ami = "old" # keep
  count = 2
}

resource "aws_instance" "db" {
  ami = "old2"
}
`;
let h = hcl::edit(src);
"#;

#[test]
fn write_preserves_layout_and_comments() {
    let out = eval_string(&format!(
        "{DOC}\n\
         h.block(\"resource\",\"aws_instance\",\"web\").attr(\"ami\").write(\"ami-123\");\n\
         h.to_string()"
    ));
    assert!(out.contains(r#"ami = "ami-123" # keep"#), "got:\n{out}");
    // Untouched lines survive verbatim.
    assert!(out.contains(r#"region = "us-east-1""#), "got:\n{out}");
    assert!(out.contains(r#"ami = "old2""#), "got:\n{out}");
    // Blank line between blocks preserved.
    assert!(out.contains("}\n\nresource"), "got:\n{out}");
}

#[test]
fn read_literal_values() {
    assert_eq!(
        eval_string(&format!("{DOC}\nh[\"region\"].read()")),
        "us-east-1"
    );
    let n: i64 = eval(&format!(
        "{DOC}\nh.block(\"resource\",\"aws_instance\",\"web\")[\"count\"].read()"
    ));
    assert_eq!(n, 2);
}

#[test]
fn block_navigation_and_counts() {
    let count: i64 = eval(&format!(
        "{DOC}\nh.block_count(\"resource\",[\"aws_instance\",\"web\"])"
    ));
    assert_eq!(count, 1);

    let keys: rhai::Array = eval(&format!(
        "{DOC}\nh.block(\"resource\",\"aws_instance\",\"web\").attribute_keys()"
    ));
    let keys: Vec<String> = keys.into_iter().map(|d| d.into_string().unwrap()).collect();
    assert_eq!(keys, vec!["ami".to_string(), "count".to_string()]);

    let types: rhai::Array = eval(&format!("{DOC}\nh.block_types()"));
    let types: Vec<String> = types
        .into_iter()
        .map(|d| d.into_string().unwrap())
        .collect();
    assert_eq!(types, vec!["resource".to_string()]);
}

#[test]
fn ambiguous_block_without_nth_is_error() {
    // Two `resource "aws_instance"` blocks (web, db): selecting by ident
    // alone is ambiguous and must error.
    let mut engine = Engine::new();
    ito::scripting::hcl::register(&mut engine);
    let res = engine.eval::<Dynamic>(&format!(
        "{DOC}\nh.block(\"resource\").attr(\"ami\").read()"
    ));
    assert!(res.is_err(), "expected ambiguity error, got {res:?}");
}

#[test]
fn remove_attribute() {
    let out = eval_string(&format!(
        "{DOC}\n\
         h.block(\"resource\",\"aws_instance\",\"web\")[\"count\"].remove();\n\
         h.to_string()"
    ));
    assert!(!out.contains("count ="), "count not removed:\n{out}");
    assert!(out.contains(r#"ami = "old" # keep"#), "got:\n{out}");
}

#[test]
fn remove_label_addressed_block() {
    // No-arg remove() on a label-addressed block selector removes exactly
    // that block, leaving siblings (and the leading attribute) intact.
    let out = eval_string(&format!(
        "{DOC}\n\
         h.block(\"resource\",\"aws_instance\",\"web\").remove();\n\
         h.to_string()"
    ));
    assert!(!out.contains(r#""web""#), "web block not removed:\n{out}");
    assert!(out.contains(r#""db""#), "db block should remain:\n{out}");
    assert!(out.contains(r#"region = "us-east-1""#), "got:\n{out}");
}

#[test]
fn remove_nth_block_with_same_labels() {
    // Two blocks share ident+labels; remove() at nth=1 drops only the
    // second, keeping the first.
    let src = "let h = hcl::edit(`variable \"x\" {\n  v = 1\n}\n\nvariable \"x\" {\n  v = 2\n}\n`);\n";
    let out = eval_string(&format!(
        "{src}\
         h.block(\"variable\",\"x\",1).remove();\n\
         h.to_string()"
    ));
    assert!(out.contains("v = 1"), "first block should remain:\n{out}");
    assert!(!out.contains("v = 2"), "second block not removed:\n{out}");
}

#[test]
fn add_attribute() {
    let out = eval_string(&format!(
        "{DOC}\n\
         h.block(\"resource\",\"aws_instance\",\"db\").add(\"monitoring\", true);\n\
         h.to_string()"
    ));
    assert!(out.contains("monitoring = true"), "got:\n{out}");
}

#[test]
fn write_raw_expr_and_ident() {
    let src = "let h = hcl::edit(`variable \"x\" {\n  type = string\n}\n`);\n";
    let out = eval_string(&format!(
        "{src}\
         h.block(\"variable\",\"x\").attr(\"type\").write(hcl::expr(\"list(string)\"));\n\
         h.to_string()"
    ));
    assert!(out.contains("type = list(string)"), "got:\n{out}");

    let out = eval_string(&format!(
        "{src}\
         h.block(\"variable\",\"x\").add(\"default\", hcl::ident(\"null\"));\n\
         h.to_string()"
    ));
    assert!(out.contains("default = null"), "got:\n{out}");
}

#[test]
fn read_raw_handles_computed_expressions() {
    // A computed expression (`var.fallback`) cannot `.read()` as a value
    // but `.read_raw()` returns it verbatim and round-trips into `add`.
    let src =
        "let h = hcl::edit(`variable \"x\" {\n  type = list(string)\n  default = var.fallback\n}\n`);\n";

    // try_read on a computed expr is ().
    let unit: Dynamic = eval(&format!(
        "{src}h.block(\"variable\",\"x\").attr(\"default\").try_read()"
    ));
    assert!(unit.is_unit());

    // read_raw round-trips it into a new attribute, verbatim.
    let out = eval_string(&format!(
        "{src}\
         let t = h.block(\"variable\",\"x\").attr(\"type\").read_raw();\n\
         h.block(\"variable\",\"x\").add(\"alias\", t);\n\
         h.to_string()"
    ));
    assert!(out.contains("alias = list(string)"), "got:\n{out}");
}

#[test]
fn try_variants_swallow_errors() {
    // Missing path: try_read -> (), exists -> false, try_write -> false.
    let unit: Dynamic = eval(&format!("{DOC}\nh[\"nope\"].try_read()"));
    assert!(unit.is_unit());

    let exists: bool = eval(&format!("{DOC}\nh[\"nope\"].exists()"));
    assert!(!exists);

    let ok: bool = eval(&format!("{DOC}\nh[\"nope\"].try_write(\"v\")"));
    assert!(!ok);
}

#[test]
fn block_count_no_labels_matches_any() {
    // `block_count(ident)` with no labels argument now counts blocks with
    // ANY labels (both aws_instance "web" and "db").
    let count: i64 = eval(&format!("{DOC}\nh.block_count(\"resource\")"));
    assert_eq!(count, 2);
    // An explicit empty array means *exactly zero* labels — none here.
    let zero: i64 = eval(&format!("{DOC}\nh.block_count(\"resource\", [])"));
    assert_eq!(zero, 0);
}

#[test]
fn block_nth_ignores_labels() {
    // `block(ident, nth)` selects the nth block regardless of labels.
    let a: i64 = eval(&format!(
        "{DOC}\nh.block(\"resource\", 0)[\"count\"].read()"
    ));
    assert_eq!(a, 2); // web block has count = 2
    let ami: String = eval(&format!(
        "{DOC}\nh.block(\"resource\", 1).attr(\"ami\").read()"
    ));
    assert_eq!(ami, "old2"); // db block
}

#[test]
fn label_globs() {
    // `*` matches exactly one label; `**` matches zero or more.
    let star: i64 = eval(&format!(
        "{DOC}\nh.block_count(\"resource\", [\"aws_instance\", \"*\"])"
    ));
    assert_eq!(star, 2);
    let star_star: i64 = eval(&format!(
        "{DOC}\nh.block_count(\"resource\", [\"**\"])"
    ));
    assert_eq!(star_star, 2);
    // First label exact, then anything: still both.
    let prefix: i64 = eval(&format!(
        "{DOC}\nh.block_count(\"resource\", [\"aws_instance\", \"**\"])"
    ));
    assert_eq!(prefix, 2);
    // A single exact label matches neither (each block has two labels).
    let one: i64 = eval(&format!(
        "{DOC}\nh.block_count(\"resource\", [\"aws_instance\"])"
    ));
    assert_eq!(one, 0);
}

#[test]
fn add_attribute_to_block_is_indented() {
    // Regression: an attribute added to a nested block must be indented to
    // the block's depth, not flush-left.
    let out = eval_string(&format!(
        "{DOC}\n\
         h.block(\"resource\",\"aws_instance\",\"db\").add(\"monitoring\", true);\n\
         h.to_string()"
    ));
    assert!(out.contains("\n  monitoring = true"), "not indented:\n{out}");
}

#[test]
fn add_block_to_document() {
    let out = eval_string(&format!(
        "{DOC}\n\
         h.add(hcl::block(\"output\").with_label(\"id\").with_attribute(\"value\", \"x\"));\n\
         h.to_string()"
    ));
    assert!(out.contains("output \"id\" {"), "got:\n{out}");
    assert!(out.contains("\n  value = \"x\""), "inner not indented:\n{out}");
}

#[test]
fn add_nested_block_with_child_is_indented() {
    let out = eval_string(&format!(
        "{DOC}\n\
         let inner = hcl::block(\"lifecycle\").with_attribute(\"create\", true);\n\
         let b = hcl::block(\"ebs\").with_attribute(\"size\", 100).with_block(inner);\n\
         h.block(\"resource\",\"aws_instance\",\"web\").add(b);\n\
         h.to_string()"
    ));
    assert!(out.contains("\n  ebs {"), "outer block not indented:\n{out}");
    assert!(out.contains("\n    size = 100"), "attr not indented:\n{out}");
    assert!(out.contains("\n    lifecycle {"), "child not indented:\n{out}");
    assert!(out.contains("\n      create = true"), "grandchild not indented:\n{out}");
}

#[test]
fn write_replaces_block() {
    let out = eval_string(&format!(
        "{DOC}\n\
         let r = hcl::block(\"resource\").with_label(\"aws_instance\").with_label(\"web\").with_attribute(\"ami\", \"new\");\n\
         h.block(\"resource\",\"aws_instance\",\"web\").write(r);\n\
         h.to_string()"
    ));
    assert!(out.contains(r#"ami = "new""#), "got:\n{out}");
    assert!(!out.contains("count = 2"), "old body should be gone:\n{out}");
    // The sibling db block survives.
    assert!(out.contains(r#""db""#), "sibling lost:\n{out}");
}

#[test]
fn type_checks() {
    assert!(eval::<bool>(&format!("{DOC}\nh.is_body()")));
    assert!(eval::<bool>(&format!(
        "{DOC}\nh.block(\"resource\",\"aws_instance\",\"web\").is_block()"
    )));
    assert!(eval::<bool>(&format!("{DOC}\nh[\"region\"].is_expr()")));
}
