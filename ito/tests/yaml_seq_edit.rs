//! Integration tests for the `yaml::edit` sequence structural edits
//! (`append` / `insert_at` / `remove_at`) exposed on a `YamlSequence` cursor.

use rhai::Engine;

/// Build an engine with only the `yaml` module registered and evaluate
/// `script`, returning its `String` result.
fn eval_string(script: &str) -> String {
    let mut engine = Engine::new();
    ito::scripting::yaml::register(&mut engine);
    engine
        .eval::<String>(script)
        .unwrap_or_else(|e| panic!("eval failed: {e}\nscript:\n{script}"))
}

/// Evaluate `script` expecting it to error; return the error message.
fn eval_err(script: &str) -> String {
    let mut engine = Engine::new();
    ito::scripting::yaml::register(&mut engine);
    match engine.eval::<String>(script) {
        Ok(v) => panic!("expected error, got: {v}\nscript:\n{script}"),
        Err(e) => e.to_string(),
    }
}

#[test]
fn block_push_keeps_indent() {
    let out = eval_string(
        r#"
        let s = yaml::edit("items:\n  - a\n  - b\n");
        s.doc["items"].append("c");
        s.to_string()
        "#,
    );
    assert_eq!(out, "items:\n  - a\n  - b\n  - c\n");
}

#[test]
fn block_insert_at_index() {
    let out = eval_string(
        r#"
        let s = yaml::edit("- a\n- c\n");
        s.doc.insert_at(1, "b");
        s.to_string()
        "#,
    );
    assert_eq!(out, "- a\n- b\n- c\n");
}

#[test]
fn block_remove() {
    let out = eval_string(
        r#"
        let s = yaml::edit("- a\n- b\n- c\n");
        s.doc.remove_at(1);
        s.to_string()
        "#,
    );
    assert_eq!(out, "- a\n- c\n");
}

#[test]
fn flow_push_preserves_spacing() {
    let tight = eval_string(
        r#"
        let s = yaml::edit("[a,b]\n");
        s.doc.append("c");
        s.to_string()
        "#,
    );
    assert_eq!(tight, "[a,b,c]\n");

    let spaced = eval_string(
        r#"
        let s = yaml::edit("[a, b]\n");
        s.doc.append("c");
        s.to_string()
        "#,
    );
    assert_eq!(spaced, "[a, b, c]\n");
}

#[test]
fn flow_multiline_layout_continues() {
    let out = eval_string(
        r#"
        let s = yaml::edit("[\n  a,\n  b,\n]\n");
        s.doc.append("c");
        s.to_string()
        "#,
    );
    assert_eq!(out, "[\n  a,\n  b,\n  c,\n]\n");
}

#[test]
fn nested_collection_value() {
    let out = eval_string(
        r#"
        let s = yaml::edit("- a\n");
        s.doc.append(#{ k: 1, nested: [1, 2] });
        s.to_string()
        "#,
    );
    // A block item serializes its nested collection as canonical block
    // YAML, indented under the dash.
    assert_eq!(out, "- a\n- k: 1\n  nested:\n    - 1\n    - 2\n");
}

#[test]
fn string_that_looks_scalar_is_quoted() {
    let out = eval_string(
        r#"
        let s = yaml::edit("- a\n");
        s.doc.append("true");
        s.to_string()
        "#,
    );
    assert_eq!(out, "- a\n- 'true'\n");
}

/// A run of edits issued through a single bound cursor variable keeps
/// working: each edit refreshes the cursor to the replacement node.
#[test]
fn chained_edits_through_one_cursor() {
    let out = eval_string(
        r#"
        let s = yaml::edit("items:\n  - a\n  - b\n");
        let seq = s.doc["items"];
        seq.append("c");
        seq.append("d");
        seq.insert_at(0, "z");
        seq.remove_at(1);
        s.to_string()
        "#,
    );
    // a b -> +c -> +d -> insert_at z@0 -> z a b c d -> remove_at@1 -> z b c d
    assert_eq!(out, "items:\n  - z\n  - b\n  - c\n  - d\n");
}

/// Removing the only block item collapses to a flow `[]`; the same
/// cursor then points at that flow node and `append` grows it.
#[test]
fn collapse_to_empty_then_grow_through_same_cursor() {
    let out = eval_string(
        r#"
        let s = yaml::edit("items:\n  - a\n");
        let seq = s.doc["items"];
        seq.remove_at(0);
        seq.append("b");
        seq.append("c");
        s.to_string()
        "#,
    );
    assert_eq!(out, "items: [b, c]\n");
}

#[test]
fn out_of_range_remove_is_noop() {
    let out = eval_string(
        r#"
        let s = yaml::edit("- a\n- b\n");
        s.doc.remove_at(9);
        s.to_string()
        "#,
    );
    assert_eq!(out, "- a\n- b\n");
}

#[test]
fn out_of_range_insert_appends() {
    let out = eval_string(
        r#"
        let s = yaml::edit("- a\n");
        s.doc.insert_at(99, "b");
        s.to_string()
        "#,
    );
    assert_eq!(out, "- a\n- b\n");
}

#[test]
fn negative_index_errors() {
    assert!(
        eval_err(r#"yaml::edit("[a]\n").doc.insert_at(-1, "x")"#).contains("negative index"),
    );
    assert!(eval_err(r#"yaml::edit("[a]\n").doc.remove_at(-1)"#).contains("negative index"));
}
