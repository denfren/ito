//! Integration tests for `yaml.edit` cursor `:kind()` and its `:is_*()`
//! predicates.

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

#[test]
fn stream_kind() {
    let got = eval("a: 1", "return subject and yaml.edit(subject):kind()");
    assert_eq!(got, "stream");
}

#[test]
fn mapping_kind() {
    let got = eval("a: 1", "return yaml.edit(subject):doc():kind()");
    assert_eq!(got, "mapping");
}

#[test]
fn sequence_kind() {
    let got = eval("- 1\n- 2\n", "return yaml.edit(subject):doc():kind()");
    assert_eq!(got, "sequence");
}

#[test]
fn scalar_kind() {
    let got = eval("a: 1", "return yaml.edit(subject):doc()['a']:kind()");
    assert_eq!(got, "scalar");
}

#[test]
fn is_predicates_match_kind_exactly() {
    let got = eval(
        "a: 1",
        r#"
        local s = yaml.edit(subject)
        local m = s:doc()
        local sc = m["a"]
        local function row(c)
            return string.format(
                "%s:%s%s%s%s%s",
                c:kind(),
                tostring(c:is_mapping()),
                tostring(c:is_sequence()),
                tostring(c:is_scalar()),
                tostring(c:is_stream()),
                tostring(c:is_alias())
            )
        end
        return row(s) .. " | " .. row(m) .. " | " .. row(sc)
        "#,
    );
    assert_eq!(
        got,
        "stream:falsefalsefalsetruefalse | mapping:truefalsefalsefalsefalse | scalar:falsefalsetruefalsefalse"
    );
}

/// An implicit null (`a:`) is a scalar that reads as `null`, not a
/// kind of its own — `NULL_SCALAR` is still a scalar node.
#[test]
fn implicit_null_is_a_scalar_reading_null() {
    let got = eval(
        "a:",
        r#"
        local v = yaml.edit(subject):doc()["a"]
        return v:kind() .. ":" .. tostring(v:read() == null)
        "#,
    );
    assert_eq!(got, "scalar:true");
}

/// An alias node gets its own kind: it carries no scalar token, so every
/// scalar method would fail on it, and reporting `"scalar"` would be a
/// lie the very next call exposes.
#[test]
fn alias_has_its_own_kind() {
    let got = eval(
        "a: &x 1\nb: *x\n",
        r#"
        local v = yaml.edit(subject):doc()["b"]
        return v:kind() .. ":" .. tostring(v:is_alias())
        "#,
    );
    assert_eq!(got, "alias:true");
}

/// `:kind()` resolves, so it raises on a path that isn't there.
/// `:exists()` and `:try_kind()` are the non-raising forms.
#[test]
fn kind_raises_on_missing_path_but_exists_answers() {
    let got = eval(
        "a: 1",
        r#"
        local m = yaml.edit(subject):doc()
        local missing = m["nope"]
        local ok = pcall(function() return missing:kind() end)
        return string.format(
            "%s|%s|%s",
            tostring(ok),
            tostring(missing:exists()),
            tostring(missing:try_kind())
        )
        "#,
    );
    assert_eq!(got, "false|false|nil");
}
