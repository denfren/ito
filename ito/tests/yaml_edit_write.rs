//! `yaml.edit` `write` over any node, null handling, arity and range checks.

use mlua::Lua;

fn lua() -> Lua {
    let lua = Lua::new();
    ito::scripting::yaml::register(&lua).expect("register yaml");
    ito::scripting::null::register(&lua).expect("register null");
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
    match lua.load(script).eval::<mlua::Value>() {
        Ok(v) => panic!("expected error, got: {v:?}\nscript:\n{script}"),
        Err(e) => e.to_string(),
    }
}

#[test]
fn write_on_implicit_null() {
    let out = eval("a:\nb: 1\n", r#"local s = yaml.edit(subject) s:doc()["a"]:write(5) return s:to_string()"#);
    assert_eq!(out, "a: 5\nb: 1\n");
}

#[test]
fn write_null_sentinel_and_read_it_back() {
    let out = eval(
        "a: 1\n",
        r#"local s = yaml.edit(subject) s:doc()["a"]:write(null)
        return s:to_string() .. tostring(s:doc()["a"]:read() == null)"#,
    );
    assert_eq!(out, "a: null\ntrue");
}

#[test]
fn write_replaces_a_mapping_with_a_scalar_and_back() {
    let out = eval(
        "a:\n  x: 1\nb: 2\n",
        r#"local s = yaml.edit(subject)
        s:doc()["a"]:write("flat")
        s:doc()["b"]:write({k = {1, 2}})
        return s:to_string()"#,
    );
    assert_eq!(out, "a: flat\nb:\n  k:\n    - 1\n    - 2\n");
}

#[test]
fn write_keeps_quote_style_and_formats_floats() {
    let out = eval(
        "a: 'x'\nb: \"y\"\nc: 1\n",
        r#"local s = yaml.edit(subject)
        s:doc()["a"]:write("it's")
        s:doc()["b"]:write("z")
        s:doc()["c"]:write(1.5)
        return s:to_string()"#,
    );
    assert_eq!(out, "a: 'it''s'\nb: \"z\"\nc: 1.5\n");
}

#[test]
fn key_node_write_renames_and_rejects_duplicates() {
    let out = eval(
        "a: 1\nb: 2\n",
        r#"local s = yaml.edit(subject) s:doc()["a"]:write(1) s:doc():entries()[1]:key_node():write("z") return s:to_string()"#,
    );
    assert_eq!(out, "z: 1\nb: 2\n");
    let e = eval_err("a: 1\nb: 2\n", r#"yaml.edit(subject):doc():rename_key("a", "b")"#);
    assert!(e.contains("duplicate key"));
}

#[test]
fn add_checks_arity() {
    assert!(eval_err("a: 1\n", r#"yaml.edit(subject):doc():add("k")"#).contains("expected (key, value)"));
    assert!(eval_err("- 1\n", r#"yaml.edit(subject):doc():add(1, 2)"#).contains("too many"));
    let out = eval("- 1\n", r#"local s = yaml.edit(subject) s:doc():add(null) return s:to_string()"#);
    assert_eq!(out, "- 1\n- null\n");
}

#[test]
fn remove_reports_out_of_range() {
    let out = eval(
        "- 1\n",
        r#"local s = yaml.edit(subject) local c = s:doc()[5] return tostring(c:try_remove())"#,
    );
    assert_eq!(out, "false");
}

#[test]
fn update_all_visits_sequences_in_order_and_can_remove() {
    let out = eval(
        "- 1\n- 2\n- 3\n",
        r#"local seen = {}
        local s = yaml.edit(subject)
        s:doc():update_all(function(v) table.insert(seen, v) if v == 2 then return nil end return v end)
        return table.concat(seen, ",") .. "|" .. s:to_string()"#,
    );
    assert_eq!(out, "1,2,3|- 1\n- 3\n");
}

#[test]
fn stream_add_creates_documents() {
    let out = eval("", r#"local s = yaml.edit(subject) s:add({a = 1}) s:add(2) return s:len() .. "|" .. s:to_string()"#);
    assert_eq!(out, "2|a: 1\n---\n2\n");
}

#[test]
fn integer_on_a_mapping_explains() {
    let e = eval_err("1: x\n", r#"return yaml.edit(subject):doc()[1]:read()"#);
    assert!(e.contains("address an entry by key"), "{e}");
}

#[test]
fn sequence_values() {
    let out = eval("- 1\n- 2\n", r#"local v = yaml.edit(subject):doc():values() return tostring(#v) .. v[2]:read()"#);
    assert_eq!(out, "22");
}
