//! Integration tests for the `hcl::edit` cursor API.

use rhai::{Dynamic, Engine, Scope};

const DOC: &str = r#"region = "us-east-1"

resource "aws_instance" "web" {
  ami = "old" # keep
  count = 2
}

resource "aws_instance" "db" {
  ami = "old2"
}
"#;

const VAR_X_TYPE_STRING: &str = r#"variable "x" {
  type = string
}
"#;

const VAR_X_TYPE_LIST_DEFAULT_VAR: &str = r#"variable "x" {
  type = list(string)
  default = var.fallback
}
"#;

const VAR_X_DUPLICATE: &str = r#"variable "x" {
  v = 1
}

variable "x" {
  v = 2
}
"#;

fn engine() -> Engine {
    let mut e = Engine::new();
    ito::scripting::hcl::register(&mut e);
    e
}

fn eval<T: Clone + 'static>(subject: &str, script: &str) -> T {
    let mut scope = Scope::new();
    scope.push("subject", subject.to_string());
    engine()
        .eval_with_scope::<T>(&mut scope, script)
        .unwrap_or_else(|e| panic!("eval failed: {e}\nscript:\n{script}"))
}

fn eval_str(subject: &str, script: &str) -> String {
    eval::<String>(subject, script)
}

// ---- layout / snapshot tests ----

#[test]
fn write_preserves_layout_and_comments() {
    insta::assert_snapshot!(eval_str(
        DOC,
        r#"let h = hcl::edit(subject);
           h.block("resource","aws_instance","web").attr("ami").write("ami-123");
           h.to_string()"#
    ));
}

#[test]
fn add_attribute_to_block_is_indented() {
    insta::assert_snapshot!(eval_str(
        DOC,
        r#"let h = hcl::edit(subject);
           h.block("resource","aws_instance","db").add("monitoring", true);
           h.to_string()"#
    ));
}

#[test]
fn add_block_to_document() {
    insta::assert_snapshot!(eval_str(
        DOC,
        r#"let h = hcl::edit(subject);
           h.add(hcl::block("output").with_label("id").with_attribute("value", "x"));
           h.to_string()"#
    ));
}

#[test]
fn add_nested_block_with_child_is_indented() {
    insta::assert_snapshot!(eval_str(
        DOC,
        r#"let h = hcl::edit(subject);
           let inner = hcl::block("lifecycle").with_attribute("create", true);
           let b = hcl::block("ebs").with_attribute("size", 100).with_block(inner);
           h.block("resource","aws_instance","web").add(b);
           h.to_string()"#
    ));
}

#[test]
fn write_replaces_block() {
    insta::assert_snapshot!(eval_str(
        DOC,
        r#"let h = hcl::edit(subject);
           let r = hcl::block("resource").with_label("aws_instance").with_label("web").with_attribute("ami", "new");
           h.block("resource","aws_instance","web").write(r);
           h.to_string()"#
    ));
}

#[test]
fn write_raw_expr_and_ident() {
    insta::assert_snapshot!(eval_str(
        VAR_X_TYPE_STRING,
        r#"let h = hcl::edit(subject);
           h.block("variable","x").attr("type").write(hcl::expr("list(string)"));
           h.to_string()"#
    ));
    insta::assert_snapshot!(eval_str(
        VAR_X_TYPE_STRING,
        r#"let h = hcl::edit(subject);
           h.block("variable","x").add("default", hcl::ident("null"));
           h.to_string()"#
    ));
}

#[test]
fn read_raw_handles_computed_expressions() {
    let unit: Dynamic = eval(
        VAR_X_TYPE_LIST_DEFAULT_VAR,
        r#"let h = hcl::edit(subject); h.block("variable","x").attr("default").try_read()"#,
    );
    assert!(unit.is_unit());

    // read_raw round-trips a computed expression into a new attribute verbatim.
    insta::assert_snapshot!(eval_str(
        VAR_X_TYPE_LIST_DEFAULT_VAR,
        r#"let h = hcl::edit(subject);
           let t = h.block("variable","x").attr("type").read_raw();
           h.block("variable","x").add("alias", t);
           h.to_string()"#
    ));
}

// ---- structural / second-pass tests ----

#[test]
fn remove_attribute() {
    let out = eval_str(
        DOC,
        r#"let h = hcl::edit(subject);
           h.block("resource","aws_instance","web")["count"].remove();
           h.to_string()"#,
    );
    let removed: bool = eval(
        &out,
        r#"let h = hcl::edit(subject);
           !h.block("resource","aws_instance","web").attr("count").exists()"#,
    );
    assert!(removed, "count attribute should be gone");
    let kept: bool = eval(
        &out,
        r#"let h = hcl::edit(subject);
           h.block("resource","aws_instance","web").attr("ami").exists()"#,
    );
    assert!(kept, "ami attribute should remain");
}

#[test]
fn remove_label_addressed_block() {
    let out = eval_str(
        DOC,
        r#"let h = hcl::edit(subject);
           h.block("resource","aws_instance","web").remove();
           h.to_string()"#,
    );
    let web_gone: bool = eval(
        &out,
        r#"let h = hcl::edit(subject);
           h.block_count("resource", ["aws_instance", "web"]) == 0"#,
    );
    assert!(web_gone, "web block should be removed");
    let db_present: bool = eval(
        &out,
        r#"let h = hcl::edit(subject);
           h.block_count("resource", ["aws_instance", "db"]) == 1"#,
    );
    assert!(db_present, "db block should remain");
    let region_present: bool = eval(&out, r#"let h = hcl::edit(subject); h["region"].exists()"#);
    assert!(region_present, "region attribute should remain");
}

#[test]
fn remove_nth_block_with_same_labels() {
    let out = eval_str(
        VAR_X_DUPLICATE,
        r#"let h = hcl::edit(subject);
           h.block("variable","x",1).remove();
           h.to_string()"#,
    );
    let first_kept: bool = eval(
        &out,
        r#"let h = hcl::edit(subject); h.block("variable","x",0)["v"].read() == 1"#,
    );
    assert!(first_kept, "first block should remain with v = 1");
    let only_one: bool = eval(
        &out,
        r#"let h = hcl::edit(subject); h.block_count("variable") == 1"#,
    );
    assert!(only_one, "second block should be removed");
}

#[test]
fn add_attribute() {
    let out = eval_str(
        DOC,
        r#"let h = hcl::edit(subject);
           h.block("resource","aws_instance","db").add("monitoring", true);
           h.to_string()"#,
    );
    let present: bool = eval(
        &out,
        r#"let h = hcl::edit(subject);
           h.block("resource","aws_instance","db")["monitoring"].read()"#,
    );
    assert!(present);
}

// ---- value / direct assertion tests ----

#[test]
fn read_literal_values() {
    assert_eq!(eval_str(DOC, r#"let h = hcl::edit(subject); h["region"].read()"#), "us-east-1");
    let n: i64 = eval(
        DOC,
        r#"let h = hcl::edit(subject); h.block("resource","aws_instance","web")["count"].read()"#,
    );
    assert_eq!(n, 2);
}

#[test]
fn block_navigation_and_counts() {
    let count: i64 = eval(
        DOC,
        r#"let h = hcl::edit(subject); h.block_count("resource",["aws_instance","web"])"#,
    );
    assert_eq!(count, 1);

    let keys: rhai::Array = eval(
        DOC,
        r#"let h = hcl::edit(subject); h.block("resource","aws_instance","web").attribute_keys()"#,
    );
    let keys: Vec<String> = keys.into_iter().map(|d| d.into_string().unwrap()).collect();
    assert_eq!(keys, ["ami", "count"]);

    let types: rhai::Array = eval(DOC, r#"let h = hcl::edit(subject); h.block_types()"#);
    let types: Vec<String> = types.into_iter().map(|d| d.into_string().unwrap()).collect();
    assert_eq!(types, ["resource"]);
}

#[test]
fn ambiguous_block_without_nth_is_error() {
    let mut scope = Scope::new();
    scope.push("subject", DOC.to_string());
    let res = engine().eval_with_scope::<Dynamic>(
        &mut scope,
        r#"let h = hcl::edit(subject); h.block("resource").attr("ami").read()"#,
    );
    assert!(res.is_err(), "expected ambiguity error, got {res:?}");
}

#[test]
fn try_variants_swallow_errors() {
    let unit: Dynamic = eval(DOC, r#"let h = hcl::edit(subject); h["nope"].try_read()"#);
    assert!(unit.is_unit());

    let exists: bool = eval(DOC, r#"let h = hcl::edit(subject); h["nope"].exists()"#);
    assert!(!exists);

    let ok: bool = eval(DOC, r#"let h = hcl::edit(subject); h["nope"].try_write("v")"#);
    assert!(!ok);
}

#[test]
fn block_count_no_labels_matches_any() {
    let count: i64 = eval(DOC, r#"let h = hcl::edit(subject); h.block_count("resource")"#);
    assert_eq!(count, 2);
    let zero: i64 = eval(DOC, r#"let h = hcl::edit(subject); h.block_count("resource", [])"#);
    assert_eq!(zero, 0);
}

#[test]
fn block_nth_ignores_labels() {
    let count: i64 = eval(
        DOC,
        r#"let h = hcl::edit(subject); h.block("resource", 0)["count"].read()"#,
    );
    assert_eq!(count, 2);
    let ami: String = eval(
        DOC,
        r#"let h = hcl::edit(subject); h.block("resource", 1).attr("ami").read()"#,
    );
    assert_eq!(ami, "old2");
}

#[test]
fn label_globs() {
    let star: i64 = eval(
        DOC,
        r#"let h = hcl::edit(subject); h.block_count("resource", ["aws_instance", "*"])"#,
    );
    assert_eq!(star, 2);
    let star_star: i64 = eval(
        DOC,
        r#"let h = hcl::edit(subject); h.block_count("resource", ["**"])"#,
    );
    assert_eq!(star_star, 2);
    let prefix: i64 = eval(
        DOC,
        r#"let h = hcl::edit(subject); h.block_count("resource", ["aws_instance", "**"])"#,
    );
    assert_eq!(prefix, 2);
    let one: i64 = eval(
        DOC,
        r#"let h = hcl::edit(subject); h.block_count("resource", ["aws_instance"])"#,
    );
    assert_eq!(one, 0);
}

#[test]
fn type_checks() {
    assert!(eval::<bool>(DOC, r#"let h = hcl::edit(subject); h.is_body()"#));
    assert!(eval::<bool>(
        DOC,
        r#"let h = hcl::edit(subject); h.block("resource","aws_instance","web").is_block()"#
    ));
    assert!(eval::<bool>(DOC, r#"let h = hcl::edit(subject); h["region"].is_expr()"#));
}
