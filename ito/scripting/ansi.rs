//! ANSI SGR styling, exposed as `string` methods.
//!
//! Registered onto Luau's built-in `string` table, so every style reads as a
//! method on the text it applies to:
//!
//! ```luau
//! print(("this is red"):red() .. " this is normal")
//! print(("warning"):bold():yellow())
//! ```
//!
//! Each method wraps its receiver in a select/reset pair (`\27[31m…\27[39m`)
//! and returns a new string, so they compose by nesting — the inner reset is
//! the *specific* one (`39` for a foreground, `49` for a background, `22`/`23`/
//! `24`/`27`/`29` for an attribute) rather than the blanket `0`, which is what
//! lets `("x"):red():bold()` keep both instead of the outer style clearing the
//! inner one.
//!
//! The surface:
//!
//! - Foregrounds: `black` `red` `green` `yellow` `blue` `magenta` `cyan`
//!   `white`, plus a `bright_` counterpart for each.
//! - Backgrounds: the same sixteen names prefixed `bg_` (`bg_red`,
//!   `bg_bright_white`).
//! - Attributes: `bold` `dim` `italic` `underline` `reverse` `strike`.
//!
//! Every one of the above takes an optional boolean: `s:red(false)` opts this
//! one call out of styling even when [`enabled`] says on. It is **not** a
//! symmetric override — `s:red(true)` does not force codes on when the
//! switch itself is off, so the result is always `enabled() && on ~= false`.
//! This is for a caller who knows *this particular string* must stay plain
//! (e.g. it is going to a file, not the terminal `ito.colors()` describes),
//! not a way to defeat the switch.
//! - `if_color([alt])` / `if_nocolor([alt])` — the receiver when styling is
//!   on / off respectively, and `alt` (or `""`) otherwise. Both **eager**, and
//!   neither inspects the receiver for escape sequences.
//! - `stripped()` — remove every escape sequence.
//! - `width()` — display width in characters, escape sequences excluded.
//!
//! # When the codes are real
//!
//! Whether a style emits anything is one process-wide switch, read at call
//! time, not at registration: `ito.colors(true|false|nil)` sets it and
//! `ito.colors()` reads it back. When styling is off every method is the
//! identity function, which is the point — a script writes `s:red()`
//! unconditionally and the switch decides, rather than each call site
//! re-deciding with an `if`.
//!
//! `nil` is *auto*, the default, and what auto resolves to is the one thing
//! that differs between the binaries:
//!
//! - Under `ito`/`irun` it detects, following the CLICOLORS convention
//!   (<https://bixense.com/clicolors/>) — see [`detect`] for the precedence.
//! - Under `iq`/`ied` it resolves to **off**, because detection reads ambient
//!   state those binaries deliberately cannot see: `iq`'s output *is* its
//!   return value and an `ied --diff` must not depend on what the run was
//!   piped into, or a preview could disagree with the `--write` after it.
//!   A script that wants color there asks for it: `ito.colors(true)`.
//!
//! So the methods themselves live in all four binaries (they are pure string
//! transforms), while the *detection* is host-only, and `ito.is_tty()` — its
//! input — is registered next to `os.getenv` and `time.now` for the same
//! reason those are.

use std::io::IsTerminal;
use std::sync::atomic::{AtomicU8, Ordering};

use mlua::{Lua, Table};

// ---------------------------------------------------------------------------
// The switch
// ---------------------------------------------------------------------------

/// `ito.colors()`'s three states, in one process-wide cell.
///
/// Process-wide (rather than per-`Lua`) because it answers a question about
/// the process's own stdout, and because a `require`d module setting it should
/// be seen by the entry script that imported it — one run has one answer.
const AUTO: u8 = 0;
const ON: u8 = 1;
const OFF: u8 = 2;

static MODE: AtomicU8 = AtomicU8::new(AUTO);

/// Whether *auto* is allowed to detect. Set only by [`register_host_detect`],
/// so `iq`/`ied` resolve auto to off without a detection path existing at all.
static DETECT: AtomicU8 = AtomicU8::new(0);

/// Reset the switch to its default. Called by `Engine::build` so two runs in
/// one process (the test suite, chiefly) do not inherit each other's state.
pub fn reset() {
    MODE.store(AUTO, Ordering::Relaxed);
    DETECT.store(0, Ordering::Relaxed);
}

/// Whether styling is on right now.
///
/// Public so other modules whose own output should track the same switch —
/// `ui.prompt`'s theme choice, chiefly — read one answer rather than
/// re-deriving it from `ito.colors()` through Luau.
pub fn enabled() -> bool {
    match MODE.load(Ordering::Relaxed) {
        ON => true,
        OFF => false,
        _ => DETECT.load(Ordering::Relaxed) == 1 && detect(),
    }
}

/// Auto-detection, CLICOLORS convention (<https://bixense.com/clicolors/>),
/// highest precedence first:
///
/// 1. `CLICOLOR_FORCE` set and not `0` — on, even when piped. This is the
///    escape hatch for a run whose output goes to something that renders the
///    codes anyway, so it deliberately outranks the TTY check.
/// 2. `NO_COLOR` set (to anything, including empty) — off.
/// 3. `CLICOLOR` set to `0` — off.
/// 4. Otherwise: on iff stdout is a terminal.
///
/// `NO_COLOR` over `CLICOLOR` is arbitrary in practice (both mean off) but
/// stated so the order is not accidental.
fn detect() -> bool {
    if let Some(v) = std::env::var_os("CLICOLOR_FORCE") {
        if v != "0" {
            return true;
        }
    }
    if std::env::var_os("NO_COLOR").is_some() {
        return false;
    }
    if std::env::var_os("CLICOLOR").is_some_and(|v| v == "0") {
        return false;
    }
    std::io::stdout().is_terminal()
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

/// Every foreground/background name and its SGR foreground code. Backgrounds
/// are the same code plus 10, and the bright pair is the base plus 60, so one
/// table generates all thirty-two methods rather than four lists that could
/// drift out of step.
const COLORS: &[(&str, u8)] = &[
    ("black", 30),
    ("red", 31),
    ("green", 32),
    ("yellow", 33),
    ("blue", 34),
    ("magenta", 35),
    ("cyan", 36),
    ("white", 37),
];

/// Attribute name, its set code, and its *specific* reset code. Bold and dim
/// share reset `22`: the terminal has one "normal intensity" and no way to
/// clear only one of them, which is a property of SGR, not a shortcut here.
const ATTRS: &[(&str, u8, u8)] = &[
    ("bold", 1, 22),
    ("dim", 2, 22),
    ("italic", 3, 23),
    ("underline", 4, 24),
    ("reverse", 7, 27),
    ("strike", 9, 29),
];

/// Register the styling methods on Luau's built-in `string` table.
///
/// Pure string transforms, so all four binaries get them; whether they emit
/// anything is [`enabled`]'s call.
pub fn register(lua: &Lua) -> mlua::Result<()> {
    let string_table: Table = lua.globals().get("string")?;

    for &(name, code) in COLORS {
        // Foreground: base code, and base+60 for the bright variant. Reset 39
        // ("default foreground"), not 0, so an enclosing style survives.
        string_table.set(name, style_fn(lua, code, 39)?)?;
        string_table.set(format!("bright_{name}"), style_fn(lua, code + 60, 39)?)?;
        // Background: the same codes shifted by 10, reset 49.
        string_table.set(format!("bg_{name}"), style_fn(lua, code + 10, 49)?)?;
        string_table.set(format!("bg_bright_{name}"), style_fn(lua, code + 70, 49)?)?;
    }

    for &(name, set, unset) in ATTRS {
        string_table.set(name, style_fn(lua, set, unset)?)?;
    }

    // s:if_color([alt]) / s:if_nocolor([alt])
    //
    // Pick between two strings on the same switch the styles read. Both are
    // **eager**: Luau has already evaluated the receiver and the argument by
    // the time either is called, so these select, they do not guard. A branch
    // worth not computing wants a plain `if ito.colors() then`.
    //
    // Neither looks at whether the receiver actually contains escape
    // sequences. The receiver is just text: `("[ok]"):if_nocolor("✔")` is a
    // perfectly good call with no codes anywhere, and a script that styles
    // conditionally would otherwise get a different answer depending on
    // whether the switch was on when the *inner* style ran.
    string_table.set(
        "if_color",
        lua.create_function(|_, (s, alt): (String, Option<String>)| {
            Ok(if enabled() {
                s
            } else {
                alt.unwrap_or_default()
            })
        })?,
    )?;

    string_table.set(
        "if_nocolor",
        lua.create_function(|_, (s, alt): (String, Option<String>)| {
            Ok(if enabled() {
                alt.unwrap_or_default()
            } else {
                s
            })
        })?,
    )?;

    string_table.set(
        "stripped",
        lua.create_function(|_, s: String| Ok(strip(&s)))?,
    )?;

    string_table.set(
        "width",
        lua.create_function(|_, s: String| Ok(strip(&s).chars().count() as i64))?,
    )?;

    Ok(())
}

/// Add `ito.is_tty()` and let *auto* detect.
///
/// Host-only (`ito`/`irun`): both halves read ambient state — the terminal-ness
/// of stdout and the CLICOLOR variables — which is the same line `os.getenv`
/// and `time.now` sit on. Without this call `ito.colors(nil)` means off, so the
/// styling methods stay pure everywhere else.
///
/// Must run before `sandbox(true)` freezes the `ito` table.
pub fn register_host_detect(lua: &Lua) -> mlua::Result<()> {
    DETECT.store(1, Ordering::Relaxed);

    let module: Table = lua.globals().get("ito")?;

    // ito.is_tty([stream]) -> bool
    // Defaults to stdout, the stream that decides whether styling a report is
    // worth doing. `"stderr"` is available because that is where logging goes,
    // and a run can easily have one redirected and not the other.
    module.set(
        "is_tty",
        lua.create_function(|_, stream: Option<String>| {
            Ok(match stream.as_deref().unwrap_or("stdout") {
                "stdout" => std::io::stdout().is_terminal(),
                "stderr" => std::io::stderr().is_terminal(),
                "stdin" => std::io::stdin().is_terminal(),
                other => {
                    return Err(mlua::Error::runtime(format!(
                        "ito.is_tty: unknown stream {other:?}; \
                         expected \"stdout\", \"stderr\" or \"stdin\""
                    )));
                }
            })
        })?,
    )?;

    Ok(())
}

/// Add `ito.colors` to the already-registered `ito` table.
///
/// Registered for every binary, unlike [`register_host_detect`]: setting the
/// switch is not a capability, and it is how a script under `iq`/`ied` — where
/// auto means off — turns styling on.
pub fn register_switch(lua: &Lua) -> mlua::Result<()> {
    let module: Table = lua.globals().get("ito")?;

    // ito.colors([on]) -> bool
    // A setter and a getter in one call, and the reason it is a function
    // rather than a field: reading it has to *resolve* auto, and the answer to
    // "will my styles show up" is more useful than "which of three states did
    // someone set". Returns the resolved boolean either way.
    //
    // Distinguishing "called with nil" from "not called" needs the variadic
    // form -- `Option<bool>` cannot tell `ito.colors()` from
    // `ito.colors(nil)`, and those mean different things here (read vs. reset
    // to auto).
    module.set(
        "colors",
        lua.create_function(|_, args: mlua::Variadic<mlua::Value>| {
            match args.first() {
                None => {}
                Some(mlua::Value::Nil) => MODE.store(AUTO, Ordering::Relaxed),
                Some(mlua::Value::Boolean(true)) => MODE.store(ON, Ordering::Relaxed),
                Some(mlua::Value::Boolean(false)) => MODE.store(OFF, Ordering::Relaxed),
                Some(other) => {
                    return Err(mlua::Error::runtime(format!(
                        "ito.colors: expected a boolean or nil, got {}",
                        other.type_name()
                    )));
                }
            }
            Ok(enabled())
        })?,
    )?;

    Ok(())
}

/// One styling method: wrap the receiver in `set`/`unset`, or hand it back
/// untouched when styling is off.
///
/// Takes an optional boolean second argument that can opt this one call out
/// of styling — `s:red(false)` is plain text regardless of the switch. It is
/// not a symmetric override: `s:red(true)` does not force codes on when the
/// switch itself is off, so the codes appear only when *both* agree
/// (`enabled() && on != false`).
fn style_fn(lua: &Lua, set: u8, unset: u8) -> mlua::Result<mlua::Function> {
    lua.create_function(move |_, (s, on): (String, Option<bool>)| {
        Ok(if enabled() && on != Some(false) {
            format!("\u{1b}[{set}m{s}\u{1b}[{unset}m")
        } else {
            s
        })
    })
}

// ---------------------------------------------------------------------------
// strip
// ---------------------------------------------------------------------------

/// Remove ANSI escape sequences.
///
/// Handles the two forms that reach a terminal from this module and from the
/// programs a script is likely to capture: CSI (`\27[` … final byte in
/// `@`–`~`) and the two-character sequences (`\27` plus one byte). An
/// unterminated CSI runs to the end of the string and is dropped whole, which
/// is what a terminal would do with it.
fn strip(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match chars.next() {
            // CSI: parameters and intermediates, then a final byte.
            Some('[') => {
                for c in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&c) {
                        break;
                    }
                }
            }
            // OSC: runs to BEL or ST (`\27\`).
            Some(']') => {
                while let Some(c) = chars.next() {
                    if c == '\u{7}' {
                        break;
                    }
                    if c == '\u{1b}' {
                        // Consume the `\` of a String Terminator.
                        let _ = chars.next();
                        break;
                    }
                }
            }
            // Any other two-byte escape: both bytes go.
            Some(_) | None => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The switch is process-wide, so the tests that move it must not run
    /// concurrently with each other.
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn lua_with_styles() -> Lua {
        let lua = Lua::new();
        register(&lua).unwrap();
        lua.globals()
            .set("ito", lua.create_table().unwrap())
            .unwrap();
        register_switch(&lua).unwrap();
        lua
    }

    #[test]
    fn on_wraps_off_is_identity() {
        let _g = LOCK.lock().unwrap();
        reset();
        let lua = lua_with_styles();

        lua.load(r#"assert(ito.colors(false) == false)"#)
            .exec()
            .unwrap();
        let plain: String = lua.load(r#"return ("x"):red()"#).eval().unwrap();
        assert_eq!(plain, "x", "off is the identity function");

        lua.load(r#"assert(ito.colors(true) == true)"#)
            .exec()
            .unwrap();
        let styled: String = lua.load(r#"return ("x"):red()"#).eval().unwrap();
        assert_eq!(styled, "\u{1b}[31mx\u{1b}[39m");
    }

    #[test]
    fn auto_is_off_without_host_detection() {
        let _g = LOCK.lock().unwrap();
        reset();
        let lua = lua_with_styles();
        // No `register_host_detect`: this is the `iq`/`ied` shape, where auto
        // must resolve to off rather than reading the process's stdout.
        let on: bool = lua.load(r#"return ito.colors()"#).eval().unwrap();
        assert!(!on);
        let plain: String = lua.load(r#"return ("x"):green()"#).eval().unwrap();
        assert_eq!(plain, "x");
    }

    #[test]
    fn nil_resets_to_auto() {
        let _g = LOCK.lock().unwrap();
        reset();
        let lua = lua_with_styles();
        lua.load(r#"ito.colors(true)"#).exec().unwrap();
        // Explicitly-passed nil goes back to auto, which without detection is
        // off -- distinct from a bare `ito.colors()`, which only reads.
        let on: bool = lua.load(r#"return ito.colors(nil)"#).eval().unwrap();
        assert!(!on, "nil is auto, and auto here is off");
    }

    #[test]
    fn bare_call_reads_without_setting() {
        let _g = LOCK.lock().unwrap();
        reset();
        let lua = lua_with_styles();
        lua.load(r#"ito.colors(true)"#).exec().unwrap();
        let first: bool = lua.load(r#"return ito.colors()"#).eval().unwrap();
        let second: bool = lua.load(r#"return ito.colors()"#).eval().unwrap();
        assert!(first && second, "reading must not clear the switch");
    }

    #[test]
    fn colors_rejects_a_non_boolean() {
        let _g = LOCK.lock().unwrap();
        reset();
        let lua = lua_with_styles();
        let err = lua.load(r#"ito.colors("yes")"#).exec().unwrap_err();
        assert!(err.to_string().contains("expected a boolean"), "{err}");
    }

    #[test]
    fn nesting_keeps_both_styles() {
        let _g = LOCK.lock().unwrap();
        reset();
        let lua = lua_with_styles();
        lua.load(r#"ito.colors(true)"#).exec().unwrap();
        // The specific resets (39/22) are what make this work: a blanket `0`
        // on the inner style would kill the outer bold from there on.
        let s: String = lua.load(r#"return ("x"):red():bold()"#).eval().unwrap();
        assert_eq!(s, "\u{1b}[1m\u{1b}[31mx\u{1b}[39m\u{1b}[22m");
    }

    #[test]
    fn bright_and_background_codes() {
        let _g = LOCK.lock().unwrap();
        reset();
        let lua = lua_with_styles();
        lua.load(r#"ito.colors(true)"#).exec().unwrap();

        let cases = [
            (r#"("x"):bright_red()"#, "\u{1b}[91mx\u{1b}[39m"),
            (r#"("x"):bg_red()"#, "\u{1b}[41mx\u{1b}[49m"),
            (r#"("x"):bg_bright_white()"#, "\u{1b}[107mx\u{1b}[49m"),
            (r#"("x"):underline()"#, "\u{1b}[4mx\u{1b}[24m"),
        ];
        for (src, want) in cases {
            let got: String = lua.load(format!("return {src}")).eval().unwrap();
            assert_eq!(got, want, "{src}");
        }
    }

    #[test]
    fn if_color_and_if_nocolor_pick_opposite_branches() {
        let _g = LOCK.lock().unwrap();
        reset();
        let lua = lua_with_styles();

        lua.load(r#"ito.colors(true)"#).exec().unwrap();
        let s: String = lua
            .load(r#"return ("Y"):if_color("N") .. ("Y"):if_nocolor("N")"#)
            .eval()
            .unwrap();
        assert_eq!(s, "YN", "on: if_color keeps, if_nocolor swaps");

        lua.load(r#"ito.colors(false)"#).exec().unwrap();
        let s: String = lua
            .load(r#"return ("Y"):if_color("N") .. ("Y"):if_nocolor("N")"#)
            .eval()
            .unwrap();
        assert_eq!(s, "NY", "off: the mirror image");
    }

    #[test]
    fn a_missing_alternative_is_empty() {
        let _g = LOCK.lock().unwrap();
        reset();
        let lua = lua_with_styles();

        lua.load(r#"ito.colors(false)"#).exec().unwrap();
        let s: String = lua.load(r#"return ("x"):if_color()"#).eval().unwrap();
        assert_eq!(s, "", "the losing branch with no alternative vanishes");
        let s: String = lua.load(r#"return ("x"):if_nocolor()"#).eval().unwrap();
        assert_eq!(s, "x");
    }

    #[test]
    fn the_receiver_is_not_inspected_for_escapes() {
        let _g = LOCK.lock().unwrap();
        reset();
        let lua = lua_with_styles();
        lua.load(r#"ito.colors(true)"#).exec().unwrap();

        // Plain text with no codes in it still counts as the styled branch:
        // the switch decides, not the content.
        let s: String = lua
            .load(r#"return ("[ok]"):if_color("plain")"#)
            .eval()
            .unwrap();
        assert_eq!(s, "[ok]");

        // And a receiver that *does* carry codes is not special-cased either.
        let s: String = lua
            .load(r#"return (("v"):green()):if_nocolor("plain")"#)
            .eval()
            .unwrap();
        assert_eq!(s, "plain");
    }

    #[test]
    fn the_documented_composition_reads_as_intended() {
        let _g = LOCK.lock().unwrap();
        reset();
        let lua = lua_with_styles();

        // The whole point: one expression, correct on both kinds of stream.
        let src = r#"return ("✔"):green():if_color("[ok]")"#;
        lua.load(r#"ito.colors(true)"#).exec().unwrap();
        let styled: String = lua.load(src).eval().unwrap();
        assert_eq!(styled, "\u{1b}[32m✔\u{1b}[39m");
        lua.load(r#"ito.colors(false)"#).exec().unwrap();
        let plain: String = lua.load(src).eval().unwrap();
        assert_eq!(
            plain, "[ok]",
            "the style was a no-op, the fallback replaced it"
        );
    }

    #[test]
    fn strip_removes_what_the_methods_add() {
        assert_eq!(strip("\u{1b}[31mred\u{1b}[39m"), "red");
        assert_eq!(strip("\u{1b}[1m\u{1b}[31mx\u{1b}[39m\u{1b}[22m"), "x");
        assert_eq!(strip("plain"), "plain");
        // 256-colour and truecolour sequences: multi-parameter CSI.
        assert_eq!(strip("\u{1b}[38;5;208mo\u{1b}[0m"), "o");
        assert_eq!(strip("\u{1b}[38;2;255;128;0mo\u{1b}[0m"), "o");
        // An unterminated CSI is dropped whole, as a terminal would.
        assert_eq!(strip("a\u{1b}[31"), "a");
        // OSC 8 hyperlink: dropped up to its terminator, text kept.
        assert_eq!(strip("\u{1b}]8;;http://x\u{7}link\u{1b}]8;;\u{7}"), "link");
    }

    #[test]
    fn width_ignores_escapes() {
        let _g = LOCK.lock().unwrap();
        reset();
        let lua = lua_with_styles();
        lua.load(r#"ito.colors(true)"#).exec().unwrap();
        let w: i64 = lua
            .load(r#"return ("hello"):red():width()"#)
            .eval()
            .unwrap();
        assert_eq!(w, 5);
        // Characters, not bytes: a multi-byte codepoint counts once.
        let w: i64 = lua.load(r#"return ("héllo"):width()"#).eval().unwrap();
        assert_eq!(w, 5);
    }

    #[test]
    fn explicit_false_opts_out_but_true_does_not_force_on() {
        let _g = LOCK.lock().unwrap();
        reset();
        let lua = lua_with_styles();

        // Switch off: passing `true` does NOT force codes on -- this is not a
        // symmetric override, just an opt-out.
        lua.load(r#"ito.colors(false)"#).exec().unwrap();
        let still_plain: String = lua.load(r#"return ("x"):red(true)"#).eval().unwrap();
        assert_eq!(still_plain, "x", "switch off wins even with on=true");

        // Switch on: passing `false` opts this call out.
        lua.load(r#"ito.colors(true)"#).exec().unwrap();
        let forced_off: String = lua.load(r#"return ("x"):red(false)"#).eval().unwrap();
        assert_eq!(forced_off, "x", "false opts this call out");

        // Switch on, no argument: reads the switch as before.
        let unset: String = lua.load(r#"return ("x"):red()"#).eval().unwrap();
        assert_eq!(unset, "\u{1b}[31mx\u{1b}[39m", "omitted still reads the switch");
    }

    #[test]
    fn detect_precedence() {
        let _g = LOCK.lock().unwrap();
        // `detect` reads the process environment, so exercise it directly
        // rather than through Luau, and put every variable back.
        let saved: Vec<_> = ["CLICOLOR_FORCE", "NO_COLOR", "CLICOLOR"]
            .iter()
            .map(|k| (*k, std::env::var_os(k)))
            .collect();
        let clear = || {
            for (k, _) in &saved {
                unsafe { std::env::remove_var(k) };
            }
        };

        clear();
        unsafe { std::env::set_var("NO_COLOR", "1") };
        assert!(!detect(), "NO_COLOR disables");

        // CLICOLOR_FORCE outranks NO_COLOR and the TTY check both.
        unsafe { std::env::set_var("CLICOLOR_FORCE", "1") };
        assert!(detect(), "CLICOLOR_FORCE forces on");

        clear();
        unsafe { std::env::set_var("CLICOLOR", "0") };
        assert!(!detect(), "CLICOLOR=0 disables");

        clear();
        unsafe { std::env::set_var("CLICOLOR_FORCE", "0") };
        // A `0` force is not a force; falls through to the TTY check, which is
        // false under `cargo test`.
        assert!(!detect(), "CLICOLOR_FORCE=0 does not force");

        clear();
        for (k, v) in saved {
            if let Some(v) = v {
                unsafe { std::env::set_var(k, v) };
            }
        }
    }
}
