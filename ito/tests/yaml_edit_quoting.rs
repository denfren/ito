//! Integration tests for the `yaml.edit` scalar quoting surface:
//! `:quote_style()` and the four verb-named converters (`unquote`,
//! `single_quote`, `double_quote`, `auto_quote`), each returning a bool
//! for whether the conversion happened.
//!
//! The converters go through the scalar's *decoded* value rather than
//! shuffling its raw text, so the interesting property — and what most
//! of these assert — is that restyling never changes what the document
//! means. `round_trips_through_every_style` is the general statement;
//! the individual tests pin the cases where the style choice itself is
//! the point.

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
        Ok(_) => panic!("expected error\nscript:\n{script}"),
        Err(e) => e.to_string(),
    }
}

/// Restyle `subject`'s `a` key with `method` and return the emitted doc.
fn restyle(subject: &str, method: &str) -> String {
    eval(
        subject,
        &format!(
            r#"local s = yaml.edit(subject) local c = s:doc()["a"] c:{method}() return s:to_string()"#
        ),
    )
}

/// Restyle and also capture the bool the converter returned.
fn try_restyle(subject: &str, method: &str) -> (bool, String) {
    eval(
        subject,
        &format!(
            r#"local s = yaml.edit(subject) local c = s:doc()["a"]
               local ok = c:{method}() return ok, s:to_string()"#
        ),
    )
}

// ---------------------------------------------------------------------------
// quote_style
// ---------------------------------------------------------------------------

#[test]
fn quote_style_names_each_style() {
    let src = "p: plain\ns: 'single'\nd: \"double\"\nl: |\n  lit\nf: >\n  fold\n";
    let got: Vec<String> = eval(
        src,
        r#"local d = yaml.edit(subject):doc()
           local out = {}
           for _, k in ipairs({"p","s","d","l","f"}) do
             table.insert(out, d[k]:quote_style())
           end
           return out"#,
    );
    assert_eq!(got, ["plain", "single", "double", "literal", "folded"]);
}

#[test]
fn quote_style_on_non_scalar_errors() {
    let err = eval_err(
        "a:\n  b: 1\n",
        r#"return yaml.edit(subject):doc():quote_style()"#,
    );
    assert!(err.contains("quote_style: not a scalar"), "got: {err}");
}

// ---------------------------------------------------------------------------
// The four converters
// ---------------------------------------------------------------------------

#[test]
fn single_quote_from_plain() {
    assert_eq!(restyle("a: plain\n", "single_quote"), "a: 'plain'\n");
}

#[test]
fn double_quote_from_plain() {
    assert_eq!(restyle("a: plain\n", "double_quote"), "a: \"plain\"\n");
}

#[test]
fn unquote_from_quoted() {
    assert_eq!(restyle("a: 'safe'\n", "unquote"), "a: safe\n");
}

/// Escapes decode first, so every style pair is reachable.
#[test]
fn escapes_convert_between_quote_styles() {
    assert_eq!(restyle("a: 'it''s'\n", "double_quote"), "a: \"it's\"\n");
    assert_eq!(restyle("a: \"it's\"\n", "single_quote"), "a: 'it''s'\n");
    assert_eq!(
        restyle("a: \"say \\\"hi\\\"\"\n", "single_quote"),
        "a: 'say \"hi\"'\n"
    );
}

/// Asking for the style a scalar already has is a no-op, not a failure.
#[test]
fn converting_to_current_style_is_a_no_op() {
    assert_eq!(restyle("a: 'x'\n", "single_quote"), "a: 'x'\n");
    assert_eq!(restyle("a: \"x\"\n", "double_quote"), "a: \"x\"\n");
    assert_eq!(restyle("a: x\n", "unquote"), "a: x\n");
}

// ---------------------------------------------------------------------------
// Auto
// ---------------------------------------------------------------------------

#[test]
fn auto_drops_unnecessary_quotes() {
    assert_eq!(restyle("a: 'quoted'\n", "auto_quote"), "a: quoted\n");
}

/// Auto keeps quotes when dropping them would change the *type* — a
/// plain `true`/`42` re-reads as a bool/number, not a string.
#[test]
fn auto_keeps_quotes_that_carry_meaning() {
    assert_eq!(restyle("a: 'true'\n", "auto_quote"), "a: 'true'\n");
    assert_eq!(restyle("a: '42'\n", "auto_quote"), "a: '42'\n");
    // A YAML 1.1 reader would see a bool / octal / sexagesimal here.
    assert_eq!(restyle("a: 'yes'\n", "auto_quote"), "a: 'yes'\n");
    assert_eq!(restyle("a: '010'\n", "auto_quote"), "a: '010'\n");
    assert_eq!(restyle("a: '1:30'\n", "auto_quote"), "a: '1:30'\n");
}

/// Auto narrows to single quotes where they suffice, but escalates to
/// double where single-quoted style would lose data.
#[test]
fn auto_picks_the_narrowest_style_that_survives() {
    // `: ` needs quoting, but single quotes carry it fine.
    assert_eq!(restyle("a: \"a: b\"\n", "auto_quote"), "a: 'a: b'\n");
    // A tab has no single-quoted spelling.
    assert_eq!(
        restyle("a: \"has\\ttab\"\n", "auto_quote"),
        "a: \"has\\ttab\"\n"
    );
    // Quoted scalars are line-folded, so `' pad '` would lose its
    // padding; double quotes preserve it.
    assert_eq!(restyle("a: ' pad '\n", "auto_quote"), "a: \" pad \"\n");
}

// ---------------------------------------------------------------------------
// bool return and the failures it reports
// ---------------------------------------------------------------------------

#[test]
fn reports_success_and_converts() {
    assert_eq!(
        try_restyle("a: 'safe'\n", "unquote"),
        (true, "a: safe\n".to_string())
    );
}

/// A refused conversion leaves the document byte-for-byte alone and
/// returns `false` instead of raising.
#[test]
fn reports_failure_and_leaves_source_untouched() {
    // Would re-read as a boolean if unquoted.
    assert_eq!(
        try_restyle("a: 'true'\n", "unquote"),
        (false, "a: 'true'\n".to_string())
    );
    // A raw tab is not a valid plain scalar...
    assert_eq!(
        try_restyle("a: \"has\\ttab\"\n", "unquote"),
        (false, "a: \"has\\ttab\"\n".to_string())
    );
    // ...and has no single-quoted spelling either.
    assert_eq!(
        try_restyle("a: \"has\\ttab\"\n", "single_quote"),
        (false, "a: \"has\\ttab\"\n".to_string())
    );
    // Edge whitespace is dropped by the fold, so single quotes would
    // silently lose it -- a refusal, not a lossy no-op.
    assert_eq!(
        try_restyle("a: \" pad \"\n", "single_quote"),
        (false, "a: \" pad \"\n".to_string())
    );
}

/// Block scalars are left alone: their decoded value is multi-line text
/// whose indentation is layout, so re-emitting it as a single quoted
/// token would reflow the document rather than restyle a value.
#[test]
fn block_scalars_are_declined() {
    for method in ["unquote", "single_quote"] {
        let (ok, out) = try_restyle("a: |\n  block\n  text\n", method);
        assert!(!ok, "{method} should decline a block scalar");
        assert_eq!(out, "a: |\n  block\n  text\n");
    }
}

#[test]
fn converters_on_non_scalar_error() {
    for method in ["unquote", "single_quote", "double_quote", "auto_quote"] {
        let err = eval_err(
            "a:\n  b: 1\n",
            &format!(r#"return yaml.edit(subject):doc():{method}()"#),
        );
        assert!(
            err.contains(&format!("{method}: not a scalar")),
            "{method} gave: {err}"
        );
    }
}

// ---------------------------------------------------------------------------
// The general property
// ---------------------------------------------------------------------------

/// Whatever style is forced, re-parsing must yield the value the
/// document started with. This is the invariant the decode/re-encode
/// design buys, and it covers the cases a textual conversion gets wrong
/// (quotes, escapes, type-lookalikes, control characters, edge
/// whitespace).
#[test]
fn round_trips_through_every_style() {
    let lua = lua();
    // `r##"..."##`: the Luau source contains `"#hash"`, which would end
    // a single-`#` raw string.
    let script = r##"
      local vals = {
        "it's", 'say "hi"', "a: b", "true", "42", " pad ", "has\ttab", "",
        "#hash", "-dash", "*star", "yes", "010", "1:30", "back\\slash", "both'\"q",
      }
      local styles = {
        "unquote", "single_quote", "double_quote", "auto_quote",
      }
      local bad = {}
      for _, v in ipairs(vals) do
        -- Start from an encoding known to be faithful, so only the
        -- restyling under test can lose anything.
        local src = yaml.to_string({a = v})
        if yaml.parse(src)["a"] ~= v then
          table.insert(bad, "base encoding lost: " .. string.format("%q", v))
        end
        for _, st in ipairs(styles) do
          local s = yaml.edit(src)
          local c = s:doc()["a"]
          -- A style that cannot represent the value returns false; that
          -- is a refusal, not a loss, so only successes are checked.
          if c[st](c) then
            local got = yaml.parse(s:to_string())["a"]
            if got ~= v then
              table.insert(bad, string.format("%s: %q -> %q", st, v, tostring(got)))
            end
          end
        end
      end
      return table.concat(bad, "; ")
    "##;
    let bad: String = lua.load(script).eval().expect("eval failed");
    assert_eq!(bad, "", "value-changing conversions");
}
