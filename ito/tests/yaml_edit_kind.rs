//! Integration tests for `yaml.edit` cursor `:kind()` and its `:is_*()`
//! predicates.

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
                tostring(c:is_null()),
                tostring(c:is_mapping()),
                tostring(c:is_sequence()),
                tostring(c:is_scalar()),
                tostring(c:is_stream())
            )
        end
        return row(s) .. " | " .. row(m) .. " | " .. row(sc)
        "#,
    );
    assert_eq!(
        got,
        "stream:falsefalsefalsefalsetrue | mapping:falsetruefalsefalsefalse | scalar:falsefalsefalsetruefalse"
    );
}
