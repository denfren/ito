//! Integration tests for the `yaml::edit` mapping structural edits
//! (`append` / `append_or_update` / `insert_at` / `remove` / `remove_at`)
//! exposed on a `YamlMapping` cursor.

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

// ---- append ----

#[test]
fn block_append_keeps_indent() {
    let out = eval_string(
        r#"
        let s = yaml::edit("outer:\n  a: 1\n");
        s.doc["outer"].append("b", 2);
        s.to_string()
        "#,
    );
    assert_eq!(out, "outer:\n  a: 1\n  b: 2\n");
}

#[test]
fn append_nested_value() {
    let out = eval_string(
        r#"
        let s = yaml::edit("a: 1\n");
        s.doc.append("b", #{ x: 1, y: 2 });
        s.to_string()
        "#,
    );
    assert_eq!(out, "a: 1\nb:\n  x: 1\n  y: 2\n");
}

#[test]
fn append_duplicate_key_errors() {
    assert!(
        eval_err(r#"yaml::edit("a: 1\n").doc.append("a", 2)"#).contains("duplicate key: a")
    );
}

#[test]
fn flow_append_preserves_spacing() {
    let spaced = eval_string(
        r#"
        let s = yaml::edit("{a: 1, b: 2}\n");
        s.doc.append("c", 3);
        s.to_string()
        "#,
    );
    assert_eq!(spaced, "{a: 1, b: 2, c: 3}\n");

    let tight = eval_string(
        r#"
        let s = yaml::edit("{a: 1,b: 2}\n");
        s.doc.append("c", 3);
        s.to_string()
        "#,
    );
    assert_eq!(tight, "{a: 1,b: 2,c: 3}\n");
}

// ---- insert_at ----

#[test]
fn insert_at_index() {
    let out = eval_string(
        r#"
        let s = yaml::edit("a: 1\nc: 3\n");
        s.doc.insert_at(1, "b", 2);
        s.to_string()
        "#,
    );
    assert_eq!(out, "a: 1\nb: 2\nc: 3\n");
}

#[test]
fn insert_at_out_of_range_appends() {
    let out = eval_string(
        r#"
        let s = yaml::edit("a: 1\n");
        s.doc.insert_at(99, "b", 2);
        s.to_string()
        "#,
    );
    assert_eq!(out, "a: 1\nb: 2\n");
}

#[test]
fn insert_at_duplicate_key_errors() {
    assert!(
        eval_err(r#"yaml::edit("a: 1\n").doc.insert_at(0, "a", 2)"#).contains("duplicate key: a")
    );
}

#[test]
fn insert_at_negative_index_errors() {
    assert!(
        eval_err(r#"yaml::edit("a: 1\n").doc.insert_at(-1, "b", 2)"#).contains("negative index")
    );
}

// ---- append_or_update ----

#[test]
fn append_or_update_appends_new() {
    let out = eval_string(
        r#"
        let s = yaml::edit("a: 1\n");
        s.doc.append_or_update("b", 2);
        s.to_string()
        "#,
    );
    assert_eq!(out, "a: 1\nb: 2\n");
}

#[test]
fn append_or_update_replaces_in_place() {
    let out = eval_string(
        r#"
        let s = yaml::edit("a: 1\nb: 2\n");
        s.doc.append_or_update("a", 99);
        s.to_string()
        "#,
    );
    assert_eq!(out, "a: 99\nb: 2\n");
}

/// Updating the only entry of a block map must NOT collapse it to flow.
#[test]
fn append_or_update_single_entry_stays_block() {
    let out = eval_string(
        r#"
        let s = yaml::edit("a: 1\n");
        s.doc.append_or_update("a", 2);
        s.to_string()
        "#,
    );
    assert_eq!(out, "a: 2\n");
}

// ---- remove / remove_at ----

#[test]
fn remove_by_key() {
    let out = eval_string(
        r#"
        let s = yaml::edit("a: 1\nb: 2\nc: 3\n");
        s.doc.remove("b");
        s.to_string()
        "#,
    );
    assert_eq!(out, "a: 1\nc: 3\n");
}

#[test]
fn remove_returns_found_flag() {
    assert_eq!(
        eval_string(r#"yaml::edit("a: 1\n").doc.remove("a").to_string()"#),
        "true"
    );
    assert_eq!(
        eval_string(r#"yaml::edit("a: 1\n").doc.remove("zzz").to_string()"#),
        "false"
    );
}

#[test]
fn remove_at_index() {
    let out = eval_string(
        r#"
        let s = yaml::edit("a: 1\nb: 2\nc: 3\n");
        s.doc.remove_at(0);
        s.to_string()
        "#,
    );
    assert_eq!(out, "b: 2\nc: 3\n");
}

#[test]
fn remove_at_out_of_range_is_noop() {
    let out = eval_string(
        r#"
        let s = yaml::edit("a: 1\n");
        s.doc.remove_at(9);
        s.to_string()
        "#,
    );
    assert_eq!(out, "a: 1\n");
}

#[test]
fn remove_at_negative_index_errors() {
    assert!(eval_err(r#"yaml::edit("a: 1\n").doc.remove_at(-1)"#).contains("negative index"));
}

/// Removing the only block entry collapses to a flow `{}`; the same
/// cursor then points at that flow node and `append` grows it.
#[test]
fn remove_last_collapses_then_append_grows() {
    let out = eval_string(
        r#"
        let s = yaml::edit("only: 1\n");
        let m = s.doc;
        m.remove("only");
        m.append("x", 1);
        m.append("y", 2);
        s.to_string()
        "#,
    );
    assert_eq!(out, "{x: 1, y: 2}\n");
}

/// Chained edits on the same handle: the cursor refreshes to the live
/// node after each structural edit.
#[test]
fn chained_edits_share_handle() {
    let out = eval_string(
        r#"
        let s = yaml::edit("a: 1\n");
        let m = s.doc;
        m.append("b", 2);
        m.append("c", 3);
        m.append_or_update("b", 99);
        s.to_string()
        "#,
    );
    assert_eq!(out, "a: 1\nb: 99\nc: 3\n");
}
