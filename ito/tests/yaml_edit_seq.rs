//! Integration tests for the `yaml::edit` sequence structural edits
//! (`append` / `insert_at` / `remove_at`) exposed on a `YamlSequence` cursor.

use rhai::{Engine, Scope};

fn engine() -> Engine {
    let mut e = Engine::new();
    ito::scripting::yaml::register(&mut e);
    e
}

fn eval(subject: &str, script: &str) -> String {
    let mut scope = Scope::new();
    scope.push("subject", subject.to_string());
    engine()
        .eval_with_scope::<String>(&mut scope, script)
        .unwrap_or_else(|e| panic!("eval failed: {e}\nscript:\n{script}"))
}

fn eval_err(subject: &str, script: &str) -> String {
    let mut scope = Scope::new();
    scope.push("subject", subject.to_string());
    match engine().eval_with_scope::<String>(&mut scope, script) {
        Ok(v) => panic!("expected error, got: {v}\nscript:\n{script}"),
        Err(e) => e.to_string(),
    }
}

#[test]
fn block_push_keeps_indent() {
    insta::assert_snapshot!(eval(
        "items:\n  - a\n  - b\n",
        r#"let s = yaml::edit(subject); s.doc["items"].append("c"); s.to_string()"#
    ));
}

#[test]
fn block_insert_at_index() {
    insta::assert_snapshot!(eval(
        "- a\n- c\n",
        r#"let s = yaml::edit(subject); s.doc.insert_at(1, "b"); s.to_string()"#
    ));
}

#[test]
fn block_remove() {
    insta::assert_snapshot!(eval(
        "- a\n- b\n- c\n",
        r#"let s = yaml::edit(subject); s.doc.remove_at(1); s.to_string()"#
    ));
}

#[test]
fn flow_push_tight_preserves_spacing() {
    insta::assert_snapshot!(eval(
        "[a,b]\n",
        r#"let s = yaml::edit(subject); s.doc.append("c"); s.to_string()"#
    ));
}

#[test]
fn flow_push_spaced_preserves_spacing() {
    insta::assert_snapshot!(eval(
        "[a, b]\n",
        r#"let s = yaml::edit(subject); s.doc.append("c"); s.to_string()"#
    ));
}

#[test]
fn flow_multiline_layout_continues() {
    insta::assert_snapshot!(eval(
        "[\n  a,\n  b,\n]\n",
        r#"let s = yaml::edit(subject); s.doc.append("c"); s.to_string()"#
    ));
}

#[test]
fn nested_collection_value() {
    // A block item serializes its nested collection as canonical block YAML, indented under the dash.
    insta::assert_snapshot!(eval(
        "- a\n",
        r#"let s = yaml::edit(subject); s.doc.append(#{ k: 1, nested: [1, 2] }); s.to_string()"#
    ));
}

#[test]
fn string_that_looks_scalar_is_quoted() {
    insta::assert_snapshot!(eval(
        "- a\n",
        r#"let s = yaml::edit(subject); s.doc.append("true"); s.to_string()"#
    ));
}

/// A run of edits issued through a single bound cursor variable keeps
/// working: each edit refreshes the cursor to the replacement node.
#[test]
fn chained_edits_through_one_cursor() {
    // a b -> +c -> +d -> insert_at z@0 -> z a b c d -> remove_at@1 -> z b c d
    insta::assert_snapshot!(eval(
        "items:\n  - a\n  - b\n",
        r#"
        let s = yaml::edit(subject);
        let seq = s.doc["items"];
        seq.append("c");
        seq.append("d");
        seq.insert_at(0, "z");
        seq.remove_at(1);
        s.to_string()
        "#
    ));
}

/// Removing the only block item collapses to a flow `[]`; the same
/// cursor then points at that flow node and `append` grows it.
#[test]
fn collapse_to_empty_then_grow_through_same_cursor() {
    insta::assert_snapshot!(eval(
        "items:\n  - a\n",
        r#"
        let s = yaml::edit(subject);
        let seq = s.doc["items"];
        seq.remove_at(0);
        seq.append("b");
        seq.append("c");
        s.to_string()
        "#
    ));
}

#[test]
fn out_of_range_remove_is_noop() {
    insta::assert_snapshot!(eval(
        "- a\n- b\n",
        r#"let s = yaml::edit(subject); s.doc.remove_at(9); s.to_string()"#
    ));
}

#[test]
fn out_of_range_insert_appends() {
    insta::assert_snapshot!(eval(
        "- a\n",
        r#"let s = yaml::edit(subject); s.doc.insert_at(99, "b"); s.to_string()"#
    ));
}

#[test]
fn negative_index_errors() {
    assert!(
        eval_err("[a]\n", r#"yaml::edit(subject).doc.insert_at(-1, "x")"#)
            .contains("negative index")
    );
    assert!(
        eval_err("[a]\n", r#"yaml::edit(subject).doc.remove_at(-1)"#).contains("negative index")
    );
}
