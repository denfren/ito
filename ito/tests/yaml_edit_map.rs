//! Integration tests for the `yaml::edit` mapping structural edits
//! (`append` / `append_or_update` / `insert_at` / `remove` / `remove_at`)
//! exposed on a `YamlMapping` cursor.

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

// ---- append ----

#[test]
fn block_append_keeps_indent() {
    insta::assert_snapshot!(eval(
        "outer:\n  a: 1\n",
        r#"let s = yaml::edit(subject); s.doc["outer"].append("b", 2); s.to_string()"#
    ));
}

#[test]
fn append_nested_value() {
    insta::assert_snapshot!(eval(
        "a: 1\n",
        r#"let s = yaml::edit(subject); s.doc.append("b", #{ x: 1, y: 2 }); s.to_string()"#
    ));
}

#[test]
fn append_duplicate_key_errors() {
    assert!(eval_err(
        "a: 1\n",
        r#"yaml::edit(subject).doc.append("a", 2)"#
    )
    .contains("duplicate key: a"));
}

#[test]
fn flow_append_spaced_preserves_spacing() {
    insta::assert_snapshot!(eval(
        "{a: 1, b: 2}\n",
        r#"let s = yaml::edit(subject); s.doc.append("c", 3); s.to_string()"#
    ));
}

#[test]
fn flow_append_tight_preserves_spacing() {
    insta::assert_snapshot!(eval(
        "{a: 1,b: 2}\n",
        r#"let s = yaml::edit(subject); s.doc.append("c", 3); s.to_string()"#
    ));
}

// ---- insert_at ----

#[test]
fn insert_at_index() {
    insta::assert_snapshot!(eval(
        "a: 1\nc: 3\n",
        r#"let s = yaml::edit(subject); s.doc.insert_at(1, "b", 2); s.to_string()"#
    ));
}

#[test]
fn insert_at_out_of_range_appends() {
    insta::assert_snapshot!(eval(
        "a: 1\n",
        r#"let s = yaml::edit(subject); s.doc.insert_at(99, "b", 2); s.to_string()"#
    ));
}

#[test]
fn insert_at_duplicate_key_errors() {
    assert!(eval_err(
        "a: 1\n",
        r#"yaml::edit(subject).doc.insert_at(0, "a", 2)"#
    )
    .contains("duplicate key: a"));
}

#[test]
fn insert_at_negative_index_errors() {
    assert!(eval_err(
        "a: 1\n",
        r#"yaml::edit(subject).doc.insert_at(-1, "b", 2)"#
    )
    .contains("negative index"));
}

// ---- append_or_update ----

#[test]
fn append_or_update_appends_new() {
    insta::assert_snapshot!(eval(
        "a: 1\n",
        r#"let s = yaml::edit(subject); s.doc.append_or_update("b", 2); s.to_string()"#
    ));
}

#[test]
fn append_or_update_replaces_in_place() {
    insta::assert_snapshot!(eval(
        "a: 1\nb: 2\n",
        r#"let s = yaml::edit(subject); s.doc.append_or_update("a", 99); s.to_string()"#
    ));
}

/// Updating the only entry of a block map must NOT collapse it to flow.
#[test]
fn append_or_update_single_entry_stays_block() {
    insta::assert_snapshot!(eval(
        "a: 1\n",
        r#"let s = yaml::edit(subject); s.doc.append_or_update("a", 2); s.to_string()"#
    ));
}

// ---- remove / remove_at ----

#[test]
fn remove_by_key() {
    insta::assert_snapshot!(eval(
        "a: 1\nb: 2\nc: 3\n",
        r#"let s = yaml::edit(subject); s.doc.remove("b"); s.to_string()"#
    ));
}

#[test]
fn remove_returns_found_flag() {
    assert_eq!(
        eval("a: 1\n", r#"yaml::edit(subject).doc.remove("a").to_string()"#),
        "true"
    );
    assert_eq!(
        eval("a: 1\n", r#"yaml::edit(subject).doc.remove("zzz").to_string()"#),
        "false"
    );
}

#[test]
fn remove_at_index() {
    insta::assert_snapshot!(eval(
        "a: 1\nb: 2\nc: 3\n",
        r#"let s = yaml::edit(subject); s.doc.remove_at(0); s.to_string()"#
    ));
}

#[test]
fn remove_at_out_of_range_is_noop() {
    insta::assert_snapshot!(eval(
        "a: 1\n",
        r#"let s = yaml::edit(subject); s.doc.remove_at(9); s.to_string()"#
    ));
}

#[test]
fn remove_at_negative_index_errors() {
    assert!(
        eval_err("a: 1\n", r#"yaml::edit(subject).doc.remove_at(-1)"#).contains("negative index")
    );
}

/// Removing the only block entry collapses to a flow `{}`; the same
/// cursor then points at that flow node and `append` grows it.
#[test]
fn remove_last_collapses_then_append_grows() {
    insta::assert_snapshot!(eval(
        "only: 1\n",
        r#"
        let s = yaml::edit(subject);
        let m = s.doc;
        m.remove("only");
        m.append("x", 1);
        m.append("y", 2);
        s.to_string()
        "#
    ));
}

/// Chained edits on the same handle: the cursor refreshes to the live
/// node after each structural edit.
#[test]
fn chained_edits_share_handle() {
    insta::assert_snapshot!(eval(
        "a: 1\n",
        r#"
        let s = yaml::edit(subject);
        let m = s.doc;
        m.append("b", 2);
        m.append("c", 3);
        m.append_or_update("b", 99);
        s.to_string()
        "#
    ));
}
