//! Integration tests for the `yaml.edit` sequence structural edits
//! (`add` / `add_at` / `remove_at`) exposed on a sequence cursor.

use mlua::Lua;

fn lua() -> Lua {
    let lua = Lua::new();
    ito::scripting::yaml::register(&lua).expect("register yaml");
    lua
}

fn eval(subject: &str, script: &str) -> String {
    let lua = lua();
    lua.globals().set("subject", subject).unwrap();
    lua.load(script)
        .eval::<String>()
        .unwrap_or_else(|e| panic!("eval failed: {e}\nscript:\n{script}"))
}

fn eval_err(subject: &str, script: &str) -> String {
    let lua = lua();
    lua.globals().set("subject", subject).unwrap();
    match lua.load(script).eval::<String>() {
        Ok(v) => panic!("expected error, got: {v}\nscript:\n{script}"),
        Err(e) => e.to_string(),
    }
}

#[test]
fn block_push_keeps_indent() {
    insta::assert_snapshot!(eval(
        "items:\n  - a\n  - b\n",
        r#"local s = yaml.edit(subject) s:doc()["items"]:add("c") return s:to_string()"#
    ));
}

#[test]
fn block_insert_at_index() {
    insta::assert_snapshot!(eval(
        "- a\n- c\n",
        r#"local s = yaml.edit(subject) s:doc():add_at(2, "b") return s:to_string()"#
    ));
}

#[test]
fn block_remove() {
    insta::assert_snapshot!(eval(
        "- a\n- b\n- c\n",
        r#"local s = yaml.edit(subject) s:doc():remove_at(2) return s:to_string()"#
    ));
}

#[test]
fn flow_push_tight_preserves_spacing() {
    insta::assert_snapshot!(eval(
        "[a,b]\n",
        r#"local s = yaml.edit(subject) s:doc():add("c") return s:to_string()"#
    ));
}

#[test]
fn flow_push_spaced_preserves_spacing() {
    insta::assert_snapshot!(eval(
        "[a, b]\n",
        r#"local s = yaml.edit(subject) s:doc():add("c") return s:to_string()"#
    ));
}

#[test]
fn flow_multiline_layout_continues() {
    insta::assert_snapshot!(eval(
        "[\n  a,\n  b,\n]\n",
        r#"local s = yaml.edit(subject) s:doc():add("c") return s:to_string()"#
    ));
}

#[test]
fn nested_collection_value() {
    // A block item serializes its nested collection as canonical block YAML, indented under the dash.
    insta::assert_snapshot!(eval(
        "- a\n",
        r#"local s = yaml.edit(subject) s:doc():add({k = 1, nested = {1, 2}}) return s:to_string()"#
    ));
}

#[test]
fn string_that_looks_scalar_is_quoted() {
    insta::assert_snapshot!(eval(
        "- a\n",
        r#"local s = yaml.edit(subject) s:doc():add("true") return s:to_string()"#
    ));
}

/// A run of edits issued through a single bound cursor variable keeps
/// working: each edit refreshes the cursor to the replacement node.
#[test]
fn chained_edits_through_one_cursor() {
    // a b -> +c -> +d -> insert_at z@1 -> z a b c d -> remove_at@2 -> z b c d
    insta::assert_snapshot!(eval(
        "items:\n  - a\n  - b\n",
        r#"
        local s = yaml.edit(subject)
        local seq = s:doc()["items"]
        seq:add("c")
        seq:add("d")
        seq:add_at(1, "z")
        seq:remove_at(2)
        return s:to_string()
        "#
    ));
}

/// Removing the only block item collapses to a flow `[]`; the same
/// cursor then points at that flow node and `add` grows it.
#[test]
fn collapse_to_empty_then_grow_through_same_cursor() {
    insta::assert_snapshot!(eval(
        "items:\n  - a\n",
        r#"
        local s = yaml.edit(subject)
        local seq = s:doc()["items"]
        seq:remove_at(1)
        seq:add("b")
        seq:add("c")
        return s:to_string()
        "#
    ));
}

#[test]
fn out_of_range_remove_errors() {
    assert!(
        eval_err("- a\n- b\n", r#"yaml.edit(subject):doc():remove_at(9)"#)
            .contains("index out of range: 9")
    );
}

#[test]
fn out_of_range_insert_appends() {
    insta::assert_snapshot!(eval(
        "- a\n",
        r#"local s = yaml.edit(subject) s:doc():add_at(99, "b") return s:to_string()"#
    ));
}

#[test]
fn negative_index_errors() {
    assert!(
        eval_err("[a]\n", r#"yaml.edit(subject):doc():add_at(-1, "x")"#)
            .contains("index must be >= 1")
    );
    assert!(
        eval_err("[a]\n", r#"yaml.edit(subject):doc():remove_at(-1)"#)
            .contains("index must be >= 1")
    );
}

#[test]
fn zero_index_errors() {
    assert!(
        eval_err("[a]\n", r#"yaml.edit(subject):doc():add_at(0, "x")"#)
            .contains("index must be >= 1")
    );
    assert!(
        eval_err("[a]\n", r#"yaml.edit(subject):doc():remove_at(0)"#)
            .contains("index must be >= 1")
    );
}
