//! Integration tests for the `yaml.edit` scalar update surface
//! (`update` / `update_at` / `update_all` / `upsert` / `upsert_at`) and
//! the `:raw()` verbatim-text handle.

use mlua::Lua;

fn lua() -> Lua {
    let lua = Lua::new();
    ito::scripting::yaml::register(&lua).expect("register yaml");
    lua
}

fn eval<T: mlua::FromLuaMulti>(subject: &str, script: &str) -> T {
    let lua = lua();
    lua.globals().set("subject", subject).unwrap();
    lua.load(script)
        .eval::<T>()
        .unwrap_or_else(|e| panic!("eval failed: {e}\nscript:\n{script}"))
}

fn eval_err(subject: &str, script: &str) -> String {
    let lua = lua();
    lua.globals().set("subject", subject).unwrap();
    match lua.load(script).eval::<mlua::Value>() {
        Ok(v) => panic!("expected error, got: {v:?}\nscript:\n{script}"),
        Err(e) => e.to_string(),
    }
}

// ---- update ----

#[test]
fn update_self_on_a_scalar() {
    let out: String = eval(
        "a: 1\n",
        r#"local s = yaml.edit(subject) s:doc()["a"]:update(function(v) return v + 1 end) return s:to_string()"#,
    );
    assert_eq!(out, "a: 2\n");
}

#[test]
fn update_by_key_on_a_mapping() {
    let out: String = eval(
        "a: 1\nb: 2\n",
        r#"local s = yaml.edit(subject) s:doc():update("b", function(v) return v * 10 end) return s:to_string()"#,
    );
    assert_eq!(out, "a: 1\nb: 20\n");
}

#[test]
fn update_by_index_on_a_sequence() {
    let out: String = eval(
        "- 1\n- 2\n- 3\n",
        r#"local s = yaml.edit(subject) s:doc():update(2, function(v) return v * 10 end) return s:to_string()"#,
    );
    assert_eq!(out, "- 1\n- 20\n- 3\n");
}

#[test]
fn update_returning_nil_removes_the_entry() {
    let out: String = eval(
        "a: 1\nb: 2\n",
        r#"local s = yaml.edit(subject) s:doc():update("a", function() return nil end) return s:to_string()"#,
    );
    assert_eq!(out, "b: 2\n");
}

#[test]
fn update_on_a_mapping_target_errors() {
    let err = eval_err(
        "a:\n  b: 1\n",
        r#"yaml.edit(subject):doc():update("a", function(v) return v end)"#,
    );
    assert!(err.contains("update: not a scalar"), "got: {err}");
}

// ---- update_at ----

#[test]
fn update_at_on_a_sequence_element() {
    let out: String = eval(
        "- 1\n- 2\n- 3\n",
        r#"local s = yaml.edit(subject) s:doc():update_at(1, function(v) return v + 100 end) return s:to_string()"#,
    );
    assert_eq!(out, "- 101\n- 2\n- 3\n");
}

#[test]
fn update_at_on_a_mapping_entry_by_position() {
    let out: String = eval(
        "a: 1\nb: 2\nc: 3\n",
        r#"local s = yaml.edit(subject) s:doc():update_at(2, function(v) return v + 100 end) return s:to_string()"#,
    );
    assert_eq!(out, "a: 1\nb: 102\nc: 3\n");
}

// ---- update_all ----

#[test]
fn update_all_on_a_mapping() {
    let out: String = eval(
        "a: 1\nb: 2\nc: 3\n",
        r#"local s = yaml.edit(subject) s:doc():update_all(function(v) return v * 10 end) return s:to_string()"#,
    );
    assert_eq!(out, "a: 10\nb: 20\nc: 30\n");
}

#[test]
fn update_all_on_a_sequence() {
    let out: String = eval(
        "- 1\n- 2\n- 3\n",
        r#"local s = yaml.edit(subject) s:doc():update_all(function(v) return v * 10 end) return s:to_string()"#,
    );
    assert_eq!(out, "- 10\n- 20\n- 30\n");
}

#[test]
fn update_all_can_remove_entries() {
    let out: String = eval(
        "a: 1\nb: 2\nc: 3\n",
        r#"local s = yaml.edit(subject) s:doc():update_all(function(v) if v == 2 then return nil end return v end) return s:to_string()"#,
    );
    assert_eq!(out, "a: 1\nc: 3\n");
}

// ---- upsert / upsert_at ----

#[test]
fn upsert_adds_when_missing() {
    let out: String = eval(
        "a: 1\n",
        r#"local s = yaml.edit(subject) s:doc():upsert("b", function(v)
             if v == nil then return "new" end
             return v
           end) return s:to_string()"#,
    );
    assert_eq!(out, "a: 1\nb: new\n");
}

#[test]
fn upsert_updates_when_present() {
    let out: String = eval(
        "a: 1\nb: 2\n",
        r#"local s = yaml.edit(subject) s:doc():upsert("a", function(v) return v + 100 end) return s:to_string()"#,
    );
    assert_eq!(out, "a: 101\nb: 2\n");
}

#[test]
fn upsert_at_inserts_at_position_when_missing() {
    let out: String = eval(
        "a: 1\nc: 3\n",
        r#"local s = yaml.edit(subject) s:doc():upsert_at(2, "b", function(v)
             if v == nil then return 2 end
             return v
           end) return s:to_string()"#,
    );
    assert_eq!(out, "a: 1\nb: 2\nc: 3\n");
}

// ---- raw ----

#[test]
fn raw_returns_the_nodes_source_text() {
    let out: String = eval(
        "a: 'quoted value'\n",
        r#"return tostring(yaml.edit(subject):doc()["a"]:raw())"#,
    );
    assert_eq!(out, "'quoted value'");
}

#[test]
fn try_raw_is_nil_on_a_missing_path() {
    let unit: mlua::Value = eval(
        "a: 1\n",
        r#"return yaml.edit(subject):doc()["nope"]:try_raw()"#,
    );
    assert!(unit.is_nil());
}

#[test]
fn raw_on_a_missing_path_errors() {
    let err = eval_err("a: 1\n", r#"yaml.edit(subject):doc()["nope"]:raw()"#);
    assert!(err.contains("key not found"), "got: {err}");
}
