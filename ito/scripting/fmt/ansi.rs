//! ANSI SGR styling and the colour switch, exposed as `fmt.*` functions and
//! `string` methods (`fmt.red(s)` is `s:red()`).
//!
//! ```luau
//! print(("this is red"):red() .. " this is normal")
//! print(("warning"):bold():yellow())
//! ```
//!
//! Each style wraps its receiver in a select/reset pair (`\27[31m…\27[39m`)
//! and returns a new string, so they compose by nesting — the inner reset is
//! the *specific* one (`39` for a foreground, `49` for a background, `22`/`23`/
//! `24`/`27`/`29` for an attribute) rather than the blanket `0`. Nesting keeps
//! both styles across *different* categories (`red():bold()`); a same-category
//! inner style (`red()` around `blue()`) ends the outer one at its reset.
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
//! switch itself is off, so the result is always `enabled() && on`. An
//! *explicit* `nil` counts as `false` (so `s:green(is_live)` with `is_live`
//! unset is plain), a non-boolean raises; only an omitted argument means "on".
//! - `if_color([alt])` / `if_nocolor([alt])` — the receiver when styling is
//!   on / off respectively, and `alt` (or `""`) otherwise. Both **eager**, and
//!   neither inspects the receiver for escape sequences.
//! - `fmt.ok()` / `fail()` / `warn()` / `info()` — a status symbol: coloured
//!   `✔ ✘ ! ℹ` when styling is on, plain `[ok] [fail] [warn] [info]` when off.
//!
//! # When the codes are real
//!
//! Whether a style emits anything is one process-wide switch, read at call
//! time, not at registration: `fmt.colors(true|false|nil|"stdout"|"stderr")`
//! sets it and `fmt.colors()` reads it back. When styling is off every method
//! is the identity function, which is the point — a script writes `s:red()`
//! unconditionally and the switch decides.
//!
//! `nil` (or `"stdout"`) is *auto*, the default; `"stderr"` is auto against
//! stderr's terminal-ness instead. What auto resolves to is the one thing
//! that differs between the binaries:
//!
//! - Under `ito` it detects, following the CLICOLORS convention
//!   (<https://bixense.com/clicolors/>) — see [`detect`] for the precedence.
//! - Under `ied` it resolves to **off**, because detection reads ambient
//!   state that binary deliberately cannot see: an `ied --diff` must not
//!   depend on what the run was piped into, or a preview could disagree with the `--write` after it.
//!   A script that wants color there asks for it: `fmt.colors(true)`.
//!
//! So the styles live in all three binaries (they are pure string
//! transforms), while the *detection* is host-only, and `fmt.is_tty()` — its
//! input — is registered next to `os.getenv` and `time.now` for the same
//! reason those are.

use std::io::IsTerminal;
use std::sync::atomic::{AtomicU8, Ordering};

use mlua::{Lua, Table, Value};

use super::def;

// ---------------------------------------------------------------------------
// The switch
// ---------------------------------------------------------------------------

/// `fmt.colors()`'s states, in one process-wide cell.
///
/// Process-wide (rather than per-`Lua`) because it answers a question about
/// the process's own streams, and because a `require`d module setting it should
/// be seen by the entry script that imported it — one run has one answer.
const AUTO: u8 = 0;
const ON: u8 = 1;
const OFF: u8 = 2;
/// Auto, detecting against stderr instead of stdout.
const AUTO_ERR: u8 = 3;

static MODE: AtomicU8 = AtomicU8::new(AUTO);

/// Whether *auto* is allowed to detect. Set only by [`register_host`], so
/// `ied` resolve auto to off without a detection path existing at all.
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
/// re-deriving it from `fmt.colors()` through Luau.
pub fn enabled() -> bool {
    match MODE.load(Ordering::Relaxed) {
        ON => true,
        OFF => false,
        m => DETECT.load(Ordering::Relaxed) == 1 && detect(m == AUTO_ERR),
    }
}

/// [`enabled`] for output that is drawn on stderr (`ui.prompt`, `ui.progress`):
/// an explicit `fmt.colors(true/false)` still wins, but *auto* detects against
/// stderr, whatever stream the script chose for its own styling.
pub fn enabled_stderr() -> bool {
    match MODE.load(Ordering::Relaxed) {
        ON => true,
        OFF => false,
        _ => DETECT.load(Ordering::Relaxed) == 1 && detect(true),
    }
}

/// Auto-detection, CLICOLORS convention (<https://bixense.com/clicolors/>),
/// highest precedence first:
///
/// 1. `CLICOLOR_FORCE` set and not `0` — on, even when piped. This is the
///    escape hatch for a run whose output goes to something that renders the
///    codes anyway, so it deliberately outranks the TTY check.
/// 2. `NO_COLOR` set to a **non-empty** value — off (per <https://no-color.org>,
///    an empty value does not count).
/// 3. `CLICOLOR` set to `0` — off.
/// 4. Otherwise: on iff the chosen stream (stdout, or stderr) is a terminal.
fn detect(stderr: bool) -> bool {
    if let Some(v) = std::env::var_os("CLICOLOR_FORCE") {
        if v != "0" {
            return true;
        }
    }
    if std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty()) {
        return false;
    }
    if std::env::var_os("CLICOLOR").is_some_and(|v| v == "0") {
        return false;
    }
    if stderr {
        std::io::stderr().is_terminal()
    } else {
        std::io::stdout().is_terminal()
    }
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

/// Status symbols: name, styled glyph, SGR foreground, plain fallback.
const SYMBOLS: &[(&str, &str, u8, &str)] = &[
    ("ok", "✔", 32, "[ok]"),
    ("fail", "✘", 31, "[fail]"),
    ("warn", "!", 33, "[warn]"),
    ("info", "ℹ", 36, "[info]"),
];

/// Register the styles, `if_color`/`if_nocolor`, the status symbols and
/// `fmt.colors` on the `fmt` table and (except `colors`/symbols) the `string`
/// table.
///
/// Pure string transforms, so all three binaries get them; whether they emit
/// anything is [`enabled`]'s call.
pub fn register(lua: &Lua, fmt: &Table, string: &Table) -> mlua::Result<()> {
    for &(name, code) in COLORS {
        // Foreground: base code, and base+60 for the bright variant. Reset 39
        // ("default foreground"), not 0, so an enclosing style survives.
        def(fmt, string, name, style_fn(lua, code, 39)?)?;
        def(fmt, string, &format!("bright_{name}"), style_fn(lua, code + 60, 39)?)?;
        // Background: the same codes shifted by 10, reset 49.
        def(fmt, string, &format!("bg_{name}"), style_fn(lua, code + 10, 49)?)?;
        def(fmt, string, &format!("bg_bright_{name}"), style_fn(lua, code + 70, 49)?)?;
    }

    for &(name, set, unset) in ATTRS {
        def(fmt, string, name, style_fn(lua, set, unset)?)?;
    }

    // s:if_color([alt]) / s:if_nocolor([alt])
    //
    // Pick between two strings on the same switch the styles read. Both are
    // **eager**: Luau has already evaluated the receiver and the argument by
    // the time either is called, so these select, they do not guard. A branch
    // worth not computing wants a plain `if fmt.colors() then`.
    //
    // Neither looks at whether the receiver actually contains escape
    // sequences: the switch decides.
    def(
        fmt,
        string,
        "if_color",
        lua.create_function(|_, (s, alt): (String, Option<String>)| {
            Ok(if enabled() {
                s
            } else {
                alt.unwrap_or_default()
            })
        })?,
    )?;
    def(
        fmt,
        string,
        "if_nocolor",
        lua.create_function(|_, (s, alt): (String, Option<String>)| {
            Ok(if enabled() {
                alt.unwrap_or_default()
            } else {
                s
            })
        })?,
    )?;

    // fmt.ok() / fail() / warn() / info(): coloured glyph, or the ASCII
    // fallback when the switch is off.
    for &(name, glyph, code, plain) in SYMBOLS {
        fmt.set(
            name,
            lua.create_function(move |_, ()| {
                Ok(if enabled() {
                    format!("\u{1b}[{code}m{glyph}\u{1b}[39m")
                } else {
                    plain.to_string()
                })
            })?,
        )?;
    }

    // fmt.colors([on]) -> bool
    // A setter and a getter in one call, and the reason it is a function
    // rather than a field: reading it has to *resolve* auto, and the answer to
    // "will my styles show up" is more useful than "which state did someone
    // set". Returns the resolved boolean either way.
    //
    // Distinguishing "called with nil" from "not called" needs the variadic
    // form: `fmt.colors()` reads, `fmt.colors(nil)` resets to auto.
    fmt.set(
        "colors",
        lua.create_function(|_, args: mlua::Variadic<Value>| {
            if args.len() > 1 {
                return Err(mlua::Error::runtime(format!(
                    "fmt.colors: expected at most 1 argument, got {}",
                    args.len()
                )));
            }
            match args.first() {
                None => {}
                Some(Value::Nil) => MODE.store(AUTO, Ordering::Relaxed),
                Some(Value::Boolean(true)) => MODE.store(ON, Ordering::Relaxed),
                Some(Value::Boolean(false)) => MODE.store(OFF, Ordering::Relaxed),
                Some(Value::String(s)) => match &*s.to_str()? {
                    "stdout" => MODE.store(AUTO, Ordering::Relaxed),
                    "stderr" => MODE.store(AUTO_ERR, Ordering::Relaxed),
                    other => {
                        return Err(mlua::Error::runtime(format!(
                            "fmt.colors: unknown stream {other:?}; \
                             expected \"stdout\" or \"stderr\""
                        )));
                    }
                },
                Some(other) => {
                    return Err(mlua::Error::runtime(format!(
                        "fmt.colors: expected a boolean, nil or a stream name, got {}",
                        other.type_name()
                    )));
                }
            }
            Ok(enabled())
        })?,
    )?;

    Ok(())
}

/// Add `fmt.is_tty()` and let *auto* detect.
///
/// Host-only (`ito`): both halves read ambient state — the terminal-ness
/// of the streams and the CLICOLOR variables — which is the same line
/// `os.getenv` and `time.now` sit on. Without this call `fmt.colors(nil)`
/// means off, so the styles stay pure everywhere else.
///
/// Must run before `sandbox(true)` freezes the `fmt` table.
pub fn register_host(lua: &Lua, fmt: &Table) -> mlua::Result<()> {
    DETECT.store(1, Ordering::Relaxed);

    // fmt.is_tty([stream]) -> bool
    // Defaults to stdout, the stream that decides whether styling a report is
    // worth doing. `"stderr"` is available because that is where logging goes,
    // and a run can easily have one redirected and not the other. (To *style*
    // by stderr, prefer `fmt.colors("stderr")`, which keeps `NO_COLOR`.)
    fmt.set(
        "is_tty",
        lua.create_function(|_, stream: Option<String>| {
            Ok(match stream.as_deref().unwrap_or("stdout") {
                "stdout" => std::io::stdout().is_terminal(),
                "stderr" => std::io::stderr().is_terminal(),
                "stdin" => std::io::stdin().is_terminal(),
                other => {
                    return Err(mlua::Error::runtime(format!(
                        "fmt.is_tty: unknown stream {other:?}; \
                         expected \"stdout\", \"stderr\" or \"stdin\""
                    )));
                }
            })
        })?,
    )?;

    Ok(())
}

/// One styling function: wrap the receiver in `set`/`unset`, or hand it back
/// untouched when styling is off.
///
/// Takes an optional boolean second argument that can opt this one call out
/// of styling — `s:red(false)` is plain text regardless of the switch. It is
/// not a symmetric override: `s:red(true)` does not force codes on when the
/// switch itself is off. An explicit `nil` is `false`; only omission is `true`.
fn style_fn(lua: &Lua, set: u8, unset: u8) -> mlua::Result<mlua::Function> {
    lua.create_function(move |_, (s, args): (String, mlua::Variadic<Value>)| {
        let on = match args.as_slice() {
            [] => true,
            [Value::Nil] => false,
            [Value::Boolean(b)] => *b,
            [other] => {
                return Err(mlua::Error::runtime(format!(
                    "expected a boolean, got {}",
                    other.type_name()
                )));
            }
            _ => return Err(mlua::Error::runtime("expected at most 1 extra argument")),
        };
        Ok(if enabled() && on {
            format!("\u{1b}[{set}m{s}\u{1b}[{unset}m")
        } else {
            s
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The switch is process-wide, so the tests that move it must not run
    /// concurrently with each other.
    use super::super::TEST_LOCK as LOCK;

    fn lua_with_styles() -> Lua {
        let lua = Lua::new();
        super::super::register(&lua).unwrap();
        lua
    }

    fn eval<T: mlua::FromLua>(lua: &Lua, src: &str) -> T {
        lua.load(src).eval().unwrap()
    }

    #[test]
    fn on_wraps_off_is_identity() {
        let _g = LOCK.lock().unwrap();
        reset();
        let lua = lua_with_styles();
        assert!(!eval::<bool>(&lua, "return fmt.colors(false)"));
        assert_eq!(eval::<String>(&lua, r#"return ("x"):red()"#), "x");
        assert!(eval::<bool>(&lua, "return fmt.colors(true)"));
        assert_eq!(
            eval::<String>(&lua, r#"return ("x"):red()"#),
            "\u{1b}[31mx\u{1b}[39m"
        );
        // The function form is the same thing.
        assert_eq!(
            eval::<String>(&lua, r#"return fmt.red("x")"#),
            "\u{1b}[31mx\u{1b}[39m"
        );
    }

    #[test]
    fn auto_is_off_without_host_detection() {
        let _g = LOCK.lock().unwrap();
        reset();
        let lua = lua_with_styles();
        // No `register_host`: the `ied` shape, where auto is off.
        assert!(!eval::<bool>(&lua, "return fmt.colors()"));
        assert!(!eval::<bool>(&lua, r#"return fmt.colors("stderr")"#));
        assert_eq!(eval::<String>(&lua, r#"return ("x"):green()"#), "x");
    }

    #[test]
    fn nil_resets_to_auto_and_bare_call_reads() {
        let _g = LOCK.lock().unwrap();
        reset();
        let lua = lua_with_styles();
        lua.load("fmt.colors(true)").exec().unwrap();
        assert!(eval::<bool>(&lua, "return fmt.colors()"));
        assert!(eval::<bool>(&lua, "return fmt.colors()"));
        assert!(!eval::<bool>(&lua, "return fmt.colors(nil)"));
    }

    #[test]
    fn colors_rejects_bad_arguments() {
        let _g = LOCK.lock().unwrap();
        reset();
        let lua = lua_with_styles();
        for src in [
            r#"fmt.colors("yes")"#,
            "fmt.colors(1)",
            "fmt.colors(true, false)",
        ] {
            assert!(lua.load(src).exec().is_err(), "{src}");
        }
    }

    #[test]
    fn nesting_keeps_both_styles() {
        let _g = LOCK.lock().unwrap();
        reset();
        let lua = lua_with_styles();
        lua.load("fmt.colors(true)").exec().unwrap();
        assert_eq!(
            eval::<String>(&lua, r#"return ("x"):red():bold()"#),
            "\u{1b}[1m\u{1b}[31mx\u{1b}[39m\u{1b}[22m"
        );
    }

    #[test]
    fn bright_and_background_codes() {
        let _g = LOCK.lock().unwrap();
        reset();
        let lua = lua_with_styles();
        lua.load("fmt.colors(true)").exec().unwrap();
        let cases = [
            (r#"("x"):bright_red()"#, "\u{1b}[91mx\u{1b}[39m"),
            (r#"("x"):bg_red()"#, "\u{1b}[41mx\u{1b}[49m"),
            (r#"("x"):bg_bright_white()"#, "\u{1b}[107mx\u{1b}[49m"),
            (r#"("x"):underline()"#, "\u{1b}[4mx\u{1b}[24m"),
        ];
        for (src, want) in cases {
            assert_eq!(eval::<String>(&lua, &format!("return {src}")), want, "{src}");
        }
    }

    #[test]
    fn if_color_and_if_nocolor_pick_opposite_branches() {
        let _g = LOCK.lock().unwrap();
        reset();
        let lua = lua_with_styles();
        let src = r#"return ("Y"):if_color("N") .. ("Y"):if_nocolor("N")"#;
        lua.load("fmt.colors(true)").exec().unwrap();
        assert_eq!(eval::<String>(&lua, src), "YN");
        lua.load("fmt.colors(false)").exec().unwrap();
        assert_eq!(eval::<String>(&lua, src), "NY");
        // A missing alternative is empty.
        assert_eq!(eval::<String>(&lua, r#"return ("x"):if_color()"#), "");
        assert_eq!(eval::<String>(&lua, r#"return ("x"):if_nocolor()"#), "x");
    }

    #[test]
    fn symbols_follow_the_switch() {
        let _g = LOCK.lock().unwrap();
        reset();
        let lua = lua_with_styles();
        lua.load("fmt.colors(true)").exec().unwrap();
        assert_eq!(eval::<String>(&lua, "return fmt.ok()"), "\u{1b}[32m✔\u{1b}[39m");
        lua.load("fmt.colors(false)").exec().unwrap();
        assert_eq!(eval::<String>(&lua, "return fmt.ok()"), "[ok]");
        assert_eq!(eval::<String>(&lua, "return fmt.fail()"), "[fail]");
    }

    #[test]
    fn style_argument_semantics() {
        let _g = LOCK.lock().unwrap();
        reset();
        let lua = lua_with_styles();
        // Switch off: `true` does not force codes on.
        lua.load("fmt.colors(false)").exec().unwrap();
        assert_eq!(eval::<String>(&lua, r#"return ("x"):red(true)"#), "x");
        lua.load("fmt.colors(true)").exec().unwrap();
        assert_eq!(eval::<String>(&lua, r#"return ("x"):red(false)"#), "x");
        // An explicit nil opts out (STR-2); omission styles.
        assert_eq!(eval::<String>(&lua, r#"local v; return ("x"):red(v)"#), "x");
        assert_eq!(
            eval::<String>(&lua, r#"return ("x"):red()"#),
            "\u{1b}[31mx\u{1b}[39m"
        );
        // Non-booleans raise.
        assert!(lua.load(r#"return ("x"):red(1)"#).exec().is_err());
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
        assert!(!detect(false), "NO_COLOR disables");

        // CLICOLOR_FORCE outranks NO_COLOR and the TTY check both.
        unsafe { std::env::set_var("CLICOLOR_FORCE", "1") };
        assert!(detect(false) && detect(true), "CLICOLOR_FORCE forces on");

        clear();
        unsafe { std::env::set_var("CLICOLOR", "0") };
        assert!(!detect(false), "CLICOLOR=0 disables");

        clear();
        unsafe { std::env::set_var("CLICOLOR_FORCE", "0") };
        assert!(!detect(false), "CLICOLOR_FORCE=0 does not force");

        // An empty NO_COLOR does not disable: CLICOLOR_FORCE still wins, and
        // with nothing else set it falls through to the tty check.
        clear();
        unsafe { std::env::set_var("NO_COLOR", "") };
        assert!(!detect(false), "no tty under cargo test");

        clear();
        for (k, v) in saved {
            if let Some(v) = v {
                unsafe { std::env::set_var(k, v) };
            }
        }
    }
}
