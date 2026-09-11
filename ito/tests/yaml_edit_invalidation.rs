//! Integration tests for cursor validity across edits.
//!
//! `yaml.edit` cursors carry a *path*, not a resolved node, and resolve
//! it against the tree's current root on every call. These tests pin the
//! behaviour that model exists to provide — and the three bugs the
//! previous node-caching model had, each of which is reproduced here as
//! the thing that must no longer happen:
//!
//! 1. a cursor whose sibling was removed wrote to the *wrong* node while
//!    reading the right one (silent corruption);
//! 2. a cursor whose index path no longer resolved panicked inside
//!    yamlito, aborting the process past `pcall`;
//! 3. an `:entries()` array captured before a removal still yielded the
//!    removed entry.

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

// ---- the corruption case ----

/// A cursor held across the removal of an *earlier* sibling must still
/// address its own entry. Under the old model `:read()` answered from
/// the discarded tree (a phantom `3`) while `:write()` resolved against
/// the new root and landed on the *following* entry.
#[test]
fn cursor_held_across_earlier_removal_writes_to_its_own_entry() {
    let got = eval(
        "a: 1\nb: 2\nc: 3\nd: 4\n",
        r#"
        local s = yaml.edit(subject)
        local doc = s:doc()
        local c = doc["c"]
        doc:remove("a")
        c:write(999)
        return s:to_string()
        "#,
    );
    assert_eq!(got, "b: 2\nc: 999\nd: 4\n");
}

/// The read half of the same bug: after an earlier sibling is removed,
/// a held cursor reads the tree as it is *now*.
#[test]
fn cursor_reads_current_tree_after_sibling_removal() {
    let got = eval(
        "a: 1\nb: 2\nc: 3\n",
        r#"
        local doc = yaml.edit(subject):doc()
        local c = doc["c"]
        doc["b"]:write(22)
        doc:remove("a")
        return tostring(c:read()) .. "," .. tostring(doc["b"]:read())
        "#,
    );
    assert_eq!(got, "3,22");
}

// ---- the panic case ----

/// A path that no longer resolves is an ordinary Luau error, catchable
/// with `pcall`. It used to be a Rust panic that unwound out of the
/// script entirely.
#[test]
fn unresolvable_path_is_a_catchable_error_not_a_panic() {
    let got = eval(
        "- one\n- two\n- three\n",
        r#"
        local s = yaml.edit(subject)
        local seq = s:doc()
        local third = seq[3]
        seq:remove_at(1)
        local ok, e = pcall(function() third:write("X") end)
        return tostring(ok)
            .. "|" .. tostring(string.find(tostring(e), "index out of range") ~= nil)
        "#,
    );
    assert_eq!(got, "false|true");
}

/// The error names the path it got through, which is what replaces the
/// pinpoint error traversal used to raise at the navigation site.
#[test]
fn missing_key_error_names_the_traversed_path() {
    let got = eval(
        "db:\n  host: localhost\n",
        r#"
        local doc = yaml.edit(subject):doc()
        local ok, e = pcall(function() return doc["db"]["port"]:read() end)
        local msg = tostring(e)
        return tostring(ok)
            .. "|" .. tostring(string.find(msg, "key not found: port") ~= nil)
            .. "|" .. tostring(string.find(msg, "at db") ~= nil)
        "#,
    );
    assert_eq!(got, "false|true|true");
}

// ---- the phantom-entries case ----

/// `:entries()` hands back key-addressed cursors, so an array captured
/// before a removal reflects it: the removed entry stops existing, its
/// siblings keep working.
#[test]
fn entries_captured_before_a_removal_reflect_it() {
    let got = eval(
        "a: 1\nb: 2\nc: 3\n",
        r#"
        local doc = yaml.edit(subject):doc()
        local captured = {}
        for _, e in ipairs(doc:entries()) do table.insert(captured, e) end
        doc:remove("b")
        local out = {}
        for _, e in ipairs(captured) do
            table.insert(out, e:key() .. "=" .. tostring(e:value():exists()))
        end
        return table.concat(out, ",")
        "#,
    );
    assert_eq!(got, "a=true,b=false,c=true");
}

/// `:values()` is key-addressed for the same reason, so writing through
/// a captured array after an unrelated removal hits the right entries.
#[test]
fn values_captured_before_a_removal_write_to_the_right_entries() {
    let got = eval(
        "a: 1\nb: 2\nc: 3\n",
        r#"
        local s = yaml.edit(subject)
        local doc = s:doc()
        local vals = doc:values()
        doc:remove("a")
        -- vals[2] addressed `b`, vals[3] addressed `c`; both survive.
        vals[2]:write(22)
        vals[3]:write(33)
        return s:to_string()
        "#,
    );
    assert_eq!(got, "b: 22\nc: 33\n");
}

// ---- the cases that already worked: guard against regression ----

/// Adding shifts no earlier index, so this worked before too. It is
/// the common case, which is why the bug stayed hidden.
#[test]
fn cursor_survives_an_add_through_its_parent() {
    let got = eval(
        "a: 1\nb: 2\n",
        r#"
        local s = yaml.edit(subject)
        local doc = s:doc()
        local a = doc["a"]
        doc:add("c", 3)
        a:write(11)
        return s:to_string()
        "#,
    );
    assert_eq!(got, "a: 11\nb: 2\nc: 3\n");
}

/// A cursor into a nested mapping survives an edit to the outer one.
#[test]
fn nested_cursor_survives_an_edit_to_its_parent() {
    let got = eval(
        "m:\n  x: 1\nn: 2\n",
        r#"
        local s = yaml.edit(subject)
        local doc = s:doc()
        local m = doc["m"]
        doc:add("z", 3)
        m:add("y", 9)
        return s:to_string()
        "#,
    );
    assert_eq!(got, "m:\n  x: 1\n  y: 9\nn: 2\nz: 3\n");
}

// ---- coordinate, not identity ----

/// A path is a *coordinate*: an index-addressed cursor names whatever
/// occupies that position now. Asserted as intended behaviour so it
/// cannot change silently — this is the trade that buys the rest.
#[test]
fn index_addressed_cursor_shifts_with_its_siblings() {
    let got = eval(
        "- one\n- two\n- three\n",
        r#"
        local seq = yaml.edit(subject):doc()
        local second = seq[2]
        seq:remove_at(1)
        -- `three` has shifted into position 2.
        return tostring(second:read())
        "#,
    );
    assert_eq!(got, "three");
}

/// Key-addressed cursors, by contrast, are stable under sibling edits —
/// which is why the mapping accessors hand those back.
#[test]
fn key_addressed_cursor_is_stable_under_sibling_edits() {
    let got = eval(
        "a: 1\nb: 2\nc: 3\n",
        r#"
        local doc = yaml.edit(subject):doc()
        local c = doc["c"]
        doc:remove("a")
        doc:add_at(1, "z", 0)
        return tostring(c:read())
        "#,
    );
    assert_eq!(got, "3");
}

// ---- to_string is no longer a snapshot ----

/// A sub-cursor's `:to_string()` resolves fresh, so it shows edits made
/// after the cursor was created. It used to return the node's text as of
/// cursor-creation time. (It still renders *its own node*, not the whole
/// document — that scope rule is unchanged.)
#[test]
fn sub_cursor_to_string_reflects_later_edits() {
    let got = eval(
        "m:\n  x: 1\n",
        r#"
        local m = yaml.edit(subject):doc()["m"]
        m:add("y", 2)
        return m:to_string()
        "#,
    );
    assert_eq!(got, "  x: 1\n  y: 2");
}
