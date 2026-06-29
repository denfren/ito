//! Integration tests for the `yaml.edit` mapping structural edits
//! (`append` / `append_or_update` / `insert_at` / `remove` / `remove_at`)
//! exposed on a mapping cursor.

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

// ---- append ----

#[test]
fn block_append_keeps_indent() {
    insta::assert_snapshot!(eval(
        "outer:\n  a: 1\n",
        r#"local s = yaml.edit(subject) s:doc()["outer"]:append("b", 2) return s:to_string()"#
    ));
}

#[test]
fn append_nested_value() {
    // A Lua table has no guaranteed key order; the emitted key order here
    // reflects Luau's table iteration order for this literal, not a
    // documented contract.
    insta::assert_snapshot!(eval(
        "a: 1\n",
        r#"local s = yaml.edit(subject) s:doc():append("b", {x = 1, y = 2}) return s:to_string()"#
    ));
}

#[test]
fn append_duplicate_key_errors() {
    assert!(
        eval_err("a: 1\n", r#"yaml.edit(subject):doc():append("a", 2)"#)
            .contains("duplicate key: a")
    );
}

#[test]
fn flow_append_spaced_preserves_spacing() {
    insta::assert_snapshot!(eval(
        "{a: 1, b: 2}\n",
        r#"local s = yaml.edit(subject) s:doc():append("c", 3) return s:to_string()"#
    ));
}

#[test]
fn flow_append_tight_preserves_spacing() {
    insta::assert_snapshot!(eval(
        "{a: 1,b: 2}\n",
        r#"local s = yaml.edit(subject) s:doc():append("c", 3) return s:to_string()"#
    ));
}

// ---- insert_at ----

#[test]
fn insert_at_index() {
    insta::assert_snapshot!(eval(
        "a: 1\nc: 3\n",
        r#"local s = yaml.edit(subject) s:doc():insert_at(2, "b", 2) return s:to_string()"#
    ));
}

#[test]
fn insert_at_out_of_range_appends() {
    insta::assert_snapshot!(eval(
        "a: 1\n",
        r#"local s = yaml.edit(subject) s:doc():insert_at(99, "b", 2) return s:to_string()"#
    ));
}

#[test]
fn insert_at_duplicate_key_errors() {
    assert!(
        eval_err("a: 1\n", r#"yaml.edit(subject):doc():insert_at(1, "a", 2)"#)
            .contains("duplicate key: a")
    );
}

#[test]
fn insert_at_negative_index_errors() {
    assert!(
        eval_err(
            "a: 1\n",
            r#"yaml.edit(subject):doc():insert_at(-1, "b", 2)"#
        )
        .contains("index must be >= 1")
    );
}

#[test]
fn insert_at_zero_index_errors() {
    assert!(
        eval_err("a: 1\n", r#"yaml.edit(subject):doc():insert_at(0, "b", 2)"#)
            .contains("index must be >= 1")
    );
}

// ---- append_or_update ----

#[test]
fn append_or_update_appends_new() {
    insta::assert_snapshot!(eval(
        "a: 1\n",
        r#"local s = yaml.edit(subject) s:doc():append_or_update("b", 2) return s:to_string()"#
    ));
}

#[test]
fn append_or_update_replaces_in_place() {
    insta::assert_snapshot!(eval(
        "a: 1\nb: 2\n",
        r#"local s = yaml.edit(subject) s:doc():append_or_update("a", 99) return s:to_string()"#
    ));
}

/// Updating the only entry of a block map must NOT collapse it to flow.
#[test]
fn append_or_update_single_entry_stays_block() {
    insta::assert_snapshot!(eval(
        "a: 1\n",
        r#"local s = yaml.edit(subject) s:doc():append_or_update("a", 2) return s:to_string()"#
    ));
}

// ---- remove / remove_at ----

#[test]
fn remove_by_key() {
    insta::assert_snapshot!(eval(
        "a: 1\nb: 2\nc: 3\n",
        r#"local s = yaml.edit(subject) s:doc():remove("b") return s:to_string()"#
    ));
}

#[test]
fn remove_returns_found_flag() {
    assert_eq!(
        eval(
            "a: 1\n",
            r#"return tostring(yaml.edit(subject):doc():remove("a"))"#
        ),
        "true"
    );
    assert_eq!(
        eval(
            "a: 1\n",
            r#"return tostring(yaml.edit(subject):doc():remove("zzz"))"#
        ),
        "false"
    );
}

#[test]
fn remove_at_index() {
    insta::assert_snapshot!(eval(
        "a: 1\nb: 2\nc: 3\n",
        r#"local s = yaml.edit(subject) s:doc():remove_at(1) return s:to_string()"#
    ));
}

#[test]
fn remove_at_out_of_range_is_noop() {
    insta::assert_snapshot!(eval(
        "a: 1\n",
        r#"local s = yaml.edit(subject) s:doc():remove_at(9) return s:to_string()"#
    ));
}

#[test]
fn remove_at_negative_index_errors() {
    assert!(
        eval_err("a: 1\n", r#"yaml.edit(subject):doc():remove_at(-1)"#)
            .contains("index must be >= 1")
    );
}

/// Removing the only block entry collapses to a flow `{}`; the same
/// cursor then points at that flow node and `append` grows it.
#[test]
fn remove_last_collapses_then_append_grows() {
    insta::assert_snapshot!(eval(
        "only: 1\n",
        r#"
        local s = yaml.edit(subject)
        local m = s:doc()
        m:remove("only")
        m:append("x", 1)
        m:append("y", 2)
        return s:to_string()
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
        local s = yaml.edit(subject)
        local m = s:doc()
        m:append("b", 2)
        m:append("c", 3)
        m:append_or_update("b", 99)
        return s:to_string()
        "#
    ));
}
