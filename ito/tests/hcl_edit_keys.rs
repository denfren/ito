//! `hcl.edit` object keys, removal scope, update/try semantics, builders.

use mlua::Lua;

fn lua() -> Lua {
    let lua = Lua::new();
    ito::scripting::hcl::register(&lua).expect("register hcl");
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
fn quoted_object_keys_are_found_not_panicked_on() {
    let doc = "tags = {\n  \"a.b\" = 1\n  plain = 2\n}\n";
    let out = eval(
        doc,
        r#"local h = hcl.edit(subject)
        local t = h["tags"]
        t["a.b"]:write(10)
        t:add("c d", 3)
        t:remove("plain")
        return h:to_string() .. table.concat(t:attr_keys(), ",")"#,
    );
    assert_eq!(out, "tags = {\n  \"a.b\" = 10\n  \"c d\" = 3\n}\na.b,c d");
}

#[test]
fn keys_are_not_silently_rewritten() {
    let e = eval_err("a = 1\n", r#"hcl.edit(subject):add("a.b", 1)"#);
    assert!(e.contains("invalid attribute name"), "{e}");
    let out = eval("", r#"return hcl.to_string({t = {["a.b"] = 1}})"#);
    assert!(out.contains("\"a.b\""), "{out}");
}

#[test]
fn remove_key_keeps_labelled_blocks() {
    let doc = "x = 1\nresource \"a\" \"b\" {\n}\nresource \"c\" \"d\" {\n}\n";
    let out = eval(
        doc,
        r#"local h = hcl.edit(subject) h:try_remove("resource") return h:to_string()"#,
    );
    assert!(out.contains("resource \"a\""), "{out}");
    assert!(out.contains("resource \"c\""), "{out}");
}

#[test]
fn blocks_without_arguments_returns_all_blocks() {
    let doc = "resource \"a\" \"b\" {\n}\nlocals {\n}\n";
    let out = eval(doc, r#"return tostring(#hcl.edit(subject):blocks())"#);
    assert_eq!(out, "2");
}

#[test]
fn update_by_position_on_a_body() {
    let out = eval(
        "a = 1\nb = 2\n",
        r#"local h = hcl.edit(subject) h:update(2, function(v) return v * 10 end) return h:to_string()"#,
    );
    assert_eq!(out, "a = 1\nb = 20\n");
}

#[test]
fn update_all_passes_computed_values_raw_and_is_atomic() {
    let out = eval(
        "a = 1\nb = var.x\n",
        r#"local h = hcl.edit(subject)
        h:update_all(function(v) if type(v) == "number" then return v + 1 end return v end)
        return h:to_string()"#,
    );
    assert_eq!(out, "a = 2\nb = var.x\n");
    let out = eval(
        "a = 1\nb = 2\n",
        r#"local h = hcl.edit(subject)
        pcall(h.update_all, h, function(v) if v == 2 then error("boom") end return v + 100 end)
        return h:to_string()"#,
    );
    assert_eq!(out, "a = 1\nb = 2\n");
}

#[test]
fn try_update_does_not_swallow_callback_errors() {
    let e = eval_err("a = 1\n", r#"hcl.edit(subject):try_update("a", function() error("boom") end)"#);
    assert!(e.contains("boom"), "{e}");
    let out = eval("a = 1\n", r#"return tostring(hcl.edit(subject):try_update("zz", function(v) return v end))"#);
    assert_eq!(out, "false");
}

#[test]
fn builder_duplicates_labels_and_block_to_string() {
    let e = eval_err("", r#"hcl.builder():with_attribute("a", 1):with_attribute("a", 2)"#);
    assert!(e.contains("duplicate attribute"), "{e}");
    let out = eval("", r#"return hcl.block("x"):with_label("a${b}"):to_string()"#);
    assert!(out.contains("\"a$${b}\""), "{out}");
}

#[test]
fn written_block_keeps_builder_comment() {
    let out = eval(
        "r \"a\" {\n}\n",
        r##"local h = hcl.edit(subject)
        h:block("r", "a"):write(hcl.block("r"):with_label("a"):with_attribute("k", 1):with_comment("# hi"))
        return h:to_string()"##,
    );
    assert!(out.contains("# hi"), "{out}");
}

#[test]
fn not_found_names_the_path() {
    let e = eval_err("a = 1\n", r#"hcl.edit(subject)["nope"]:read()"#);
    assert!(e.contains("not found: nope"), "{e}");
}
