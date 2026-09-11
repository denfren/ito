//! Integration tests for the `hcl.edit` cursor API.

use mlua::{Lua, Value};

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

fn lua() -> Lua {
    let lua = Lua::new();
    ito::scripting::hcl::register(&lua).expect("register hcl");
    lua
}

fn eval<T: mlua::FromLua>(subject: &str, script: &str) -> T {
    let lua = lua();
    lua.globals().set("subject", subject).unwrap();
    lua.load(script)
        .eval::<T>()
        .unwrap_or_else(|e| panic!("eval failed: {e}\nscript:\n{script}"))
}

fn eval_str(subject: &str, script: &str) -> String {
    eval::<String>(subject, script)
}

fn eval_err(subject: &str, script: &str) -> String {
    let lua = lua();
    lua.globals().set("subject", subject).unwrap();
    match lua.load(script).eval::<mlua::Value>() {
        Ok(v) => panic!("expected error, got: {v:?}\nscript:\n{script}"),
        Err(e) => e.to_string(),
    }
}

// ---- layout / snapshot tests ----

#[test]
fn write_preserves_layout_and_comments() {
    insta::assert_snapshot!(eval_str(
        DOC,
        r#"local h = hcl.edit(subject)
           h:block("resource","aws_instance","web"):attr("ami"):write("ami-123")
           return h:to_string()"#
    ));
}

#[test]
fn add_attribute_to_block_is_indented() {
    insta::assert_snapshot!(eval_str(
        DOC,
        r#"local h = hcl.edit(subject)
           h:block("resource","aws_instance","db"):add("monitoring", true)
           return h:to_string()"#
    ));
}

#[test]
fn add_block_to_document() {
    insta::assert_snapshot!(eval_str(
        DOC,
        r#"local h = hcl.edit(subject)
           h:add(hcl.block("output"):with_label("id"):with_attribute("value", "x"))
           return h:to_string()"#
    ));
}

#[test]
fn add_nested_block_with_child_is_indented() {
    insta::assert_snapshot!(eval_str(
        DOC,
        r#"local h = hcl.edit(subject)
           local inner = hcl.block("lifecycle"):with_attribute("create", true)
           local b = hcl.block("ebs"):with_attribute("size", 100):with_block(inner)
           h:block("resource","aws_instance","web"):add(b)
           return h:to_string()"#
    ));
}

#[test]
fn write_replaces_block() {
    insta::assert_snapshot!(eval_str(
        DOC,
        r#"local h = hcl.edit(subject)
           local r = hcl.block("resource"):with_label("aws_instance"):with_label("web"):with_attribute("ami", "new")
           h:block("resource","aws_instance","web"):write(r)
           return h:to_string()"#
    ));
}

#[test]
fn write_raw_expr_and_ident() {
    insta::assert_snapshot!(eval_str(
        VAR_X_TYPE_STRING,
        r#"local h = hcl.edit(subject)
           h:block("variable","x"):attr("type"):write(hcl.expr("list(string)"))
           return h:to_string()"#
    ));
    insta::assert_snapshot!(eval_str(
        VAR_X_TYPE_STRING,
        r#"local h = hcl.edit(subject)
           h:block("variable","x"):add("default", hcl.ident("null"))
           return h:to_string()"#
    ));
}

#[test]
fn raw_handles_computed_expressions() {
    let unit: Value = eval(
        VAR_X_TYPE_LIST_DEFAULT_VAR,
        r#"local h = hcl.edit(subject) return h:block("variable","x"):attr("default"):try_read()"#,
    );
    assert!(unit.is_nil());

    // raw round-trips a computed expression into a new attribute verbatim.
    insta::assert_snapshot!(eval_str(
        VAR_X_TYPE_LIST_DEFAULT_VAR,
        r#"local h = hcl.edit(subject)
           local t = h:block("variable","x"):attr("type"):raw()
           h:block("variable","x"):add("alias", t)
           return h:to_string()"#
    ));
}

// ---- structural / second-pass tests ----

#[test]
fn remove_attribute() {
    let out = eval_str(
        DOC,
        r#"local h = hcl.edit(subject)
           h:block("resource","aws_instance","web")["count"]:remove()
           return h:to_string()"#,
    );
    let removed: bool = eval(
        &out,
        r#"local h = hcl.edit(subject)
           return not h:block("resource","aws_instance","web"):attr("count"):exists()"#,
    );
    assert!(removed, "count attribute should be gone");
    let kept: bool = eval(
        &out,
        r#"local h = hcl.edit(subject)
           return h:block("resource","aws_instance","web"):attr("ami"):exists()"#,
    );
    assert!(kept, "ami attribute should remain");
}

#[test]
fn remove_label_addressed_block() {
    let out = eval_str(
        DOC,
        r#"local h = hcl.edit(subject)
           h:block("resource","aws_instance","web"):remove()
           return h:to_string()"#,
    );
    let web_gone: bool = eval(
        &out,
        r#"local h = hcl.edit(subject)
           return h:block_count("resource", {"aws_instance", "web"}) == 0"#,
    );
    assert!(web_gone, "web block should be removed");
    let db_present: bool = eval(
        &out,
        r#"local h = hcl.edit(subject)
           return h:block_count("resource", {"aws_instance", "db"}) == 1"#,
    );
    assert!(db_present, "db block should remain");
    let region_present: bool = eval(
        &out,
        r#"local h = hcl.edit(subject) return h["region"]:exists()"#,
    );
    assert!(region_present, "region attribute should remain");
}

#[test]
fn remove_nth_block_with_same_labels() {
    let out = eval_str(
        VAR_X_DUPLICATE,
        r#"local h = hcl.edit(subject)
           h:block("variable","x",2):remove()
           return h:to_string()"#,
    );
    let first_kept: bool = eval(
        &out,
        r#"local h = hcl.edit(subject) return h:block("variable","x",1)["v"]:read() == 1"#,
    );
    assert!(first_kept, "first block should remain with v = 1");
    let only_one: bool = eval(
        &out,
        r#"local h = hcl.edit(subject) return h:block_count("variable") == 1"#,
    );
    assert!(only_one, "second block should be removed");
}

#[test]
fn add_attribute() {
    let out = eval_str(
        DOC,
        r#"local h = hcl.edit(subject)
           h:block("resource","aws_instance","db"):add("monitoring", true)
           return h:to_string()"#,
    );
    let present: bool = eval(
        &out,
        r#"local h = hcl.edit(subject)
           return h:block("resource","aws_instance","db")["monitoring"]:read()"#,
    );
    assert!(present);
}

// ---- value / direct assertion tests ----

#[test]
fn read_literal_values() {
    assert_eq!(
        eval_str(
            DOC,
            r#"local h = hcl.edit(subject) return h["region"]:read()"#
        ),
        "us-east-1"
    );
    let n: i64 = eval(
        DOC,
        r#"local h = hcl.edit(subject) return h:block("resource","aws_instance","web")["count"]:read()"#,
    );
    assert_eq!(n, 2);
}

#[test]
fn block_navigation_and_counts() {
    let count: i64 = eval(
        DOC,
        r#"local h = hcl.edit(subject) return h:block_count("resource",{"aws_instance","web"})"#,
    );
    assert_eq!(count, 1);

    let keys: Vec<String> = eval(
        DOC,
        r#"local h = hcl.edit(subject) return h:block("resource","aws_instance","web"):attribute_keys()"#,
    );
    assert_eq!(keys, ["ami", "count"]);

    let types: Vec<String> = eval(DOC, r#"local h = hcl.edit(subject) return h:block_types()"#);
    assert_eq!(types, ["resource"]);
}

#[test]
fn ambiguous_block_without_nth_is_error() {
    let lua = lua();
    lua.globals().set("subject", DOC).unwrap();
    let res: mlua::Result<Value> = lua
        .load(r#"local h = hcl.edit(subject) return h:block("resource"):attr("ami"):read()"#)
        .eval();
    assert!(res.is_err(), "expected ambiguity error, got {res:?}");
}

#[test]
fn try_variants_swallow_errors() {
    let unit: Value = eval(
        DOC,
        r#"local h = hcl.edit(subject) return h["nope"]:try_read()"#,
    );
    assert!(unit.is_nil());

    let exists: bool = eval(
        DOC,
        r#"local h = hcl.edit(subject) return h["nope"]:exists()"#,
    );
    assert!(!exists);

    let ok: bool = eval(
        DOC,
        r#"local h = hcl.edit(subject) return h["nope"]:try_write("v")"#,
    );
    assert!(!ok);
}

#[test]
fn block_count_no_labels_matches_any() {
    let count: i64 = eval(
        DOC,
        r#"local h = hcl.edit(subject) return h:block_count("resource")"#,
    );
    assert_eq!(count, 2);
    let zero: i64 = eval(
        DOC,
        r#"local h = hcl.edit(subject) return h:block_count("resource", {})"#,
    );
    assert_eq!(zero, 0);
}

#[test]
fn block_nth_ignores_labels() {
    let count: i64 = eval(
        DOC,
        r#"local h = hcl.edit(subject) return h:block("resource", 1)["count"]:read()"#,
    );
    assert_eq!(count, 2);
    let ami: String = eval(
        DOC,
        r#"local h = hcl.edit(subject) return h:block("resource", 2):attr("ami"):read()"#,
    );
    assert_eq!(ami, "old2");
}

#[test]
fn label_globs() {
    let star: i64 = eval(
        DOC,
        r#"local h = hcl.edit(subject) return h:block_count("resource", {"aws_instance", "*"})"#,
    );
    assert_eq!(star, 2);
    let star_star: i64 = eval(
        DOC,
        r#"local h = hcl.edit(subject) return h:block_count("resource", {"**"})"#,
    );
    assert_eq!(star_star, 2);
    let prefix: i64 = eval(
        DOC,
        r#"local h = hcl.edit(subject) return h:block_count("resource", {"aws_instance", "**"})"#,
    );
    assert_eq!(prefix, 2);
    let one: i64 = eval(
        DOC,
        r#"local h = hcl.edit(subject) return h:block_count("resource", {"aws_instance"})"#,
    );
    assert_eq!(one, 0);
}

#[test]
fn type_checks() {
    assert!(eval::<bool>(
        DOC,
        r#"local h = hcl.edit(subject) return h:is_body()"#
    ));
    assert!(eval::<bool>(
        DOC,
        r#"local h = hcl.edit(subject) return h:block("resource","aws_instance","web"):is_block()"#
    ));
    assert!(eval::<bool>(
        DOC,
        r#"local h = hcl.edit(subject) return h["region"]:is_expr()"#
    ));
}

// ---- kind ----

#[test]
fn kind_names_each_target() {
    assert_eq!(
        eval::<String>(DOC, r#"local h = hcl.edit(subject) return h:kind()"#),
        "body"
    );
    assert_eq!(
        eval::<String>(
            DOC,
            r#"local h = hcl.edit(subject) return h:block("resource","aws_instance","web"):kind()"#
        ),
        "block"
    );
    assert_eq!(
        eval::<String>(
            DOC,
            r#"local h = hcl.edit(subject) return h["region"]:kind()"#
        ),
        "expr"
    );
}

#[test]
fn try_kind_is_nil_on_a_missing_path() {
    let unit: Value = eval(
        DOC,
        r#"local h = hcl.edit(subject) return h["nope"]:try_kind()"#,
    );
    assert!(unit.is_nil());
}

// ---- add_at ----

#[test]
fn add_at_inserts_at_position() {
    insta::assert_snapshot!(eval_str(
        VAR_X_TYPE_STRING,
        r#"local h = hcl.edit(subject)
           h:block("variable","x"):add_at(1, "default", "dev")
           return h:to_string()"#
    ));
}

#[test]
fn add_at_duplicate_key_errors() {
    let err = eval_err(
        VAR_X_TYPE_STRING,
        r#"local h = hcl.edit(subject)
           h:block("variable","x"):add_at(1, "type", "dev")"#,
    );
    assert!(err.contains("duplicate key: type"), "got: {err}");
}

// ---- update / update_at / update_all ----

#[test]
fn update_self_reads_and_writes_the_attribute() {
    insta::assert_snapshot!(eval_str(
        DOC,
        r#"local h = hcl.edit(subject)
           h:block("resource","aws_instance","web"):attr("ami"):update(function(v) return v .. "!" end)
           return h:to_string()"#
    ));
}

#[test]
fn update_by_key() {
    insta::assert_snapshot!(eval_str(
        DOC,
        r#"local h = hcl.edit(subject)
           h:block("resource","aws_instance","web"):update("ami", function(v) return v .. "!" end)
           return h:to_string()"#
    ));
}

#[test]
fn update_returning_nil_removes_the_attribute() {
    insta::assert_snapshot!(eval_str(
        DOC,
        r#"local h = hcl.edit(subject)
           h:block("resource","aws_instance","web"):update("ami", function() return nil end)
           return h:to_string()"#
    ));
}

#[test]
fn update_at_by_position() {
    insta::assert_snapshot!(eval_str(
        DOC,
        r#"local h = hcl.edit(subject)
           h:block("resource","aws_instance","web"):update_at(1, function(v) return v .. "-x" end)
           return h:to_string()"#
    ));
}

#[test]
fn update_all_applies_to_every_attribute() {
    insta::assert_snapshot!(eval_str(
        DOC,
        r#"local h = hcl.edit(subject)
           h:block("resource","aws_instance","web"):update_all(function(v)
             if type(v) == "string" then return v .. "-all" end
             return v
           end)
           return h:to_string()"#
    ));
}

// ---- upsert / upsert_at ----

#[test]
fn upsert_adds_when_missing() {
    insta::assert_snapshot!(eval_str(
        DOC,
        r#"local h = hcl.edit(subject)
           h:block("resource","aws_instance","web"):upsert("region", function(v)
             if v == nil then return "eu-west-1" end
             return v
           end)
           return h:to_string()"#
    ));
}

#[test]
fn upsert_updates_when_present() {
    insta::assert_snapshot!(eval_str(
        DOC,
        r#"local h = hcl.edit(subject)
           h:block("resource","aws_instance","web"):upsert("ami", function(v) return v .. "!" end)
           return h:to_string()"#
    ));
}

#[test]
fn upsert_at_inserts_at_position_when_missing() {
    insta::assert_snapshot!(eval_str(
        DOC,
        r#"local h = hcl.edit(subject)
           h:block("resource","aws_instance","web"):upsert_at(1, "region", function(v)
             if v == nil then return "eu-west-1" end
             return v
           end)
           return h:to_string()"#
    ));
}

// ---- remove_at ----

#[test]
fn remove_at_by_position() {
    insta::assert_snapshot!(eval_str(
        DOC,
        r#"local h = hcl.edit(subject)
           h:block("resource","aws_instance","web"):remove_at(1)
           return h:to_string()"#
    ));
}
