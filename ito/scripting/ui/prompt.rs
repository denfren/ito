//! `ui.prompt` — interactive terminal prompts, backed by `dialoguer`.
//!
//! Two layers, like `hcl.builder()`/`hcl.block()` beside `hcl.parse`: a
//! **shorthand** call for the common case, and a **builder** for everything
//! `dialoguer` exposes on that prompt kind.
//!
//! ```lua
//! -- shorthand
//! if ui.prompt.confirm("Deploy to prod?") then ... end
//! local name = ui.prompt.input("Service name", "web")
//!
//! -- builder: every dialoguer knob, `with_*` chained, `:interact()` finishes
//! local ok = ui.prompt.confirm_builder()
//!     :prompt("Deploy to prod?")
//!     :default(false)
//!     :wait_for_newline(true)
//!     :interact()
//! ```
//!
//! | Shorthand | Builder | Returns |
//! | --- | --- | --- |
//! | `ui.prompt.confirm(msg[, default])` | `ui.prompt.confirm_builder()` | `bool` |
//! | `ui.prompt.input(msg[, default])` | `ui.prompt.input_builder()` | `string` |
//! | `ui.prompt.password(msg[, confirm])` | `ui.prompt.password_builder()` | `string` |
//! | `ui.prompt.select(msg, items[, default])` | `ui.prompt.select_builder()` | `1`-based index |
//! | `ui.prompt.multi_select(msg, items[, defaults])` | `ui.prompt.multi_select_builder()` | array of `1`-based indices |
//!
//! Each builder is a fresh userdata per call (`ui.prompt.input_builder()`),
//! carrying the same fields dialoguer's own type does; `with_*`-style
//! methods here are unprefixed (`:prompt(..)`, `:default(..)`) since the
//! kind is already in the constructor's name. A terminal `:interact()` (or
//! `:interact_opt()` on `select`/`multi_select`, which answers `nil` on
//! Escape instead of raising) consumes the builder and runs the prompt.
//!
//! **All prompts block the calling task on the terminal**, so both the
//! shorthand and the builder's `:interact()` run on a blocking thread via
//! `spawn_blocking` and are awaited — the same shape `hash.*_file` uses for
//! a blocking read, and for the same reason: this is the single-threaded
//! runtime every I/O module shares, and pinning it here would starve every
//! other `task`. There is no form that runs concurrently with itself, since
//! a terminal has one cursor.
//!
//! **Ctrl-C during a prompt** raises a Luau error (dialoguer reports it as
//! an I/O interruption) rather than silently returning a default — wrap the
//! call in `pcall` for "abort gracefully". **Escape** is different: plain
//! `interact()` also raises for it, but `select`/`multi_select` offer
//! `:interact_opt()`, which returns `nil` instead — the one gesture
//! dialoguer itself treats as a first-class "no answer" on those two kinds.
//!
//! `select`/`multi_select` index **1-based**, matching every other ito
//! surface (`path.capture`, `cli` positionals, Luau itself).
//!
//! # What's not exposed, and why
//!
//! - **Custom themes.** `with_theme` takes a `&dyn Theme` tied to the
//!   builder's lifetime; a fully custom theme would mean implementing that
//!   trait from Luau, which mlua has no path for. `:theme("colorful")` /
//!   `:theme("simple")` cover dialoguer's own two theme types, built fresh
//!   per `:interact()` rather than borrowed; the default, `:theme("auto")`
//!   (also what the shorthands use), tracks `ito.colors()` — the same switch
//!   `string:red()` reads — so a prompt's colour follows `NO_COLOR`/a piped
//!   stdout/an explicit `ito.colors(false)` like everything else does.
//! - **Live validators** (`validate_with`) and **history/completion**
//!   (`Input`'s `history_with`/`completion_with`). All three take a Rust
//!   closure or trait object invoked *during* the interaction, on whichever
//!   thread is reading the terminal — here that's a `spawn_blocking` thread,
//!   and calling back into a `!Send` `mlua::Lua` from it is not something
//!   mlua supports. The idiomatic replacement is to validate the *result*
//!   after `:interact()` returns and loop:
//!   ```lua
//!   local n
//!   repeat
//!       n = tonumber(ui.prompt.input("Port"))
//!   until n and n > 0 and n < 65536
//!   ```
//!
//! Host-only (`ito`/`irun`): terminal interaction is ambient state a query's
//! return value or an edit preview must not depend on. See
//! [`crate::scripting::ui`] for the umbrella module's own rationale.

use crate::scripting::ansi;
use dialoguer::theme::{ColorfulTheme, SimpleTheme, Theme};
use dialoguer::{Confirm, Input, MultiSelect, Password, Select};
use mlua::{Lua, Result as LuaResult, Table, UserData, UserDataMethods};
use std::cell::RefCell;

// ---------------------------------------------------------------------------
// Theme
// ---------------------------------------------------------------------------

/// The theme spellings every builder accepts. A fresh `Box<dyn Theme>` is
/// built at `:interact()` time rather than held across the builder's life, so
/// nothing here borrows.
///
/// `Auto` (the default) tracks the same switch `string:red()` and friends
/// read (`ito.colors()`): a prompt is part of the same run's output, so
/// `NO_COLOR`/a piped stdout/`ito.colors(false)` should turn its colour off
/// exactly as it does everywhere else, and `:theme(...)` is the explicit
/// override for a script that wants to disagree.
#[derive(Clone, Copy, Default)]
enum ThemeChoice {
    #[default]
    Auto,
    Simple,
    Colorful,
}

impl ThemeChoice {
    fn parse(name: &str) -> LuaResult<Self> {
        match name {
            "auto" => Ok(ThemeChoice::Auto),
            "simple" => Ok(ThemeChoice::Simple),
            "colorful" => Ok(ThemeChoice::Colorful),
            other => Err(mlua::Error::runtime(format!(
                "ui.prompt: unknown theme {other:?}; expected \"auto\", \"simple\" or \"colorful\""
            ))),
        }
    }

    fn build(self) -> Box<dyn Theme> {
        let colorful = match self {
            ThemeChoice::Auto => ansi::enabled(),
            ThemeChoice::Simple => false,
            ThemeChoice::Colorful => true,
        };
        if colorful {
            Box::new(ColorfulTheme::default())
        } else {
            Box::new(SimpleTheme)
        }
    }
}

/// Run a blocking `dialoguer` interaction off the single-threaded runtime,
/// mapping its `io::Result` (a raised Ctrl-C, or a broken terminal) onto a
/// Luau error rather than a default value -- a script asking to fall back on
/// interruption wraps the call in `pcall` itself.
async fn run_blocking<T, F>(name: &'static str, f: F) -> LuaResult<T>
where
    T: Send + 'static,
    F: FnOnce() -> dialoguer::Result<T> + Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| mlua::Error::runtime(format!("ui.prompt.{name}: task failed: {e}")))?
        .map_err(|e| mlua::Error::runtime(format!("ui.prompt.{name}: {e}")))
}

/// Same as [`run_blocking`], for the `_opt` interactions that return
/// `Option<T>` instead of raising on Escape.
async fn run_blocking_opt<T, F>(name: &'static str, f: F) -> LuaResult<Option<T>>
where
    T: Send + 'static,
    F: FnOnce() -> dialoguer::Result<Option<T>> + Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| mlua::Error::runtime(format!("ui.prompt.{name}: task failed: {e}")))?
        .map_err(|e| mlua::Error::runtime(format!("ui.prompt.{name}: {e}")))
}

/// Validate and convert a 1-based index argument to 0-based, or `None` when
/// the argument itself was `None`. Shared by `select`'s `default` and
/// `multi_select`'s `defaults` so the bounds message reads the same either way.
fn to_zero_based(arg_name: &str, one_based: Option<i64>, len: usize) -> LuaResult<Option<usize>> {
    let Some(i) = one_based else { return Ok(None) };
    if i < 1 || i as usize > len {
        return Err(mlua::Error::runtime(format!(
            "ui.prompt: {arg_name} {i} out of range for {len} item(s) (1-based)"
        )));
    }
    Ok(Some(i as usize - 1))
}

fn require_items(name: &'static str, items: &[String]) -> LuaResult<()> {
    if items.is_empty() {
        Err(mlua::Error::runtime(format!(
            "ui.prompt.{name}: items must not be empty"
        )))
    } else {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Confirm
// ---------------------------------------------------------------------------

#[derive(Default)]
struct ConfirmSpec {
    prompt: String,
    default: Option<bool>,
    show_default: bool,
    wait_for_newline: bool,
    report: bool,
    theme: ThemeChoice,
}

struct ConfirmBuilder(RefCell<ConfirmSpec>);

impl ConfirmBuilder {
    fn new() -> Self {
        Self(RefCell::new(ConfirmSpec {
            show_default: true,
            report: true,
            ..Default::default()
        }))
    }
}

impl UserData for ConfirmBuilder {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("prompt", |_, this, v: String| {
            this.0.borrow_mut().prompt = v;
            Ok(())
        });
        methods.add_method("default", |_, this, v: bool| {
            this.0.borrow_mut().default = Some(v);
            Ok(())
        });
        methods.add_method("show_default", |_, this, v: bool| {
            this.0.borrow_mut().show_default = v;
            Ok(())
        });
        methods.add_method("wait_for_newline", |_, this, v: bool| {
            this.0.borrow_mut().wait_for_newline = v;
            Ok(())
        });
        methods.add_method("report", |_, this, v: bool| {
            this.0.borrow_mut().report = v;
            Ok(())
        });
        methods.add_method("theme", |_, this, name: String| {
            this.0.borrow_mut().theme = ThemeChoice::parse(&name)?;
            Ok(())
        });
        methods.add_async_method("interact", |_, this, ()| async move {
            let (prompt, default, show_default, wait_for_newline, report, theme) = {
                let spec = this.0.borrow();
                (
                    spec.prompt.clone(),
                    spec.default,
                    spec.show_default,
                    spec.wait_for_newline,
                    spec.report,
                    spec.theme,
                )
            };
            run_blocking("confirm", move || {
                let theme = theme.build();
                let mut p = Confirm::with_theme(theme.as_ref())
                    .with_prompt(prompt)
                    .show_default(show_default)
                    .wait_for_newline(wait_for_newline)
                    .report(report);
                if let Some(d) = default {
                    p = p.default(d);
                }
                p.interact()
            })
            .await
        });
        methods.add_async_method("interact_opt", |_, this, ()| async move {
            let (prompt, default, show_default, wait_for_newline, report, theme) = {
                let spec = this.0.borrow();
                (
                    spec.prompt.clone(),
                    spec.default,
                    spec.show_default,
                    spec.wait_for_newline,
                    spec.report,
                    spec.theme,
                )
            };
            run_blocking_opt("confirm", move || {
                let theme = theme.build();
                let mut p = Confirm::with_theme(theme.as_ref())
                    .with_prompt(prompt)
                    .show_default(show_default)
                    .wait_for_newline(wait_for_newline)
                    .report(report);
                if let Some(d) = default {
                    p = p.default(d);
                }
                p.interact_opt()
            })
            .await
        });
    }
}

// ---------------------------------------------------------------------------
// Input
// ---------------------------------------------------------------------------

#[derive(Default)]
struct InputSpec {
    prompt: String,
    default: Option<String>,
    initial_text: Option<String>,
    allow_empty: bool,
    show_default: bool,
    post_completion_text: Option<String>,
    report: bool,
    theme: ThemeChoice,
}

struct InputBuilder(RefCell<InputSpec>);

impl InputBuilder {
    fn new() -> Self {
        Self(RefCell::new(InputSpec {
            show_default: true,
            report: true,
            ..Default::default()
        }))
    }
}

impl UserData for InputBuilder {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("prompt", |_, this, v: String| {
            this.0.borrow_mut().prompt = v;
            Ok(())
        });
        methods.add_method("default", |_, this, v: String| {
            this.0.borrow_mut().default = Some(v);
            Ok(())
        });
        methods.add_method("initial_text", |_, this, v: String| {
            this.0.borrow_mut().initial_text = Some(v);
            Ok(())
        });
        methods.add_method("allow_empty", |_, this, v: bool| {
            this.0.borrow_mut().allow_empty = v;
            Ok(())
        });
        methods.add_method("show_default", |_, this, v: bool| {
            this.0.borrow_mut().show_default = v;
            Ok(())
        });
        methods.add_method("post_completion_text", |_, this, v: String| {
            this.0.borrow_mut().post_completion_text = Some(v);
            Ok(())
        });
        methods.add_method("report", |_, this, v: bool| {
            this.0.borrow_mut().report = v;
            Ok(())
        });
        methods.add_method("theme", |_, this, name: String| {
            this.0.borrow_mut().theme = ThemeChoice::parse(&name)?;
            Ok(())
        });
        methods.add_async_method("interact", |_, this, ()| async move {
            let (prompt, default, initial_text, allow_empty, show_default, post, report, theme) = {
                let spec = this.0.borrow();
                (
                    spec.prompt.clone(),
                    spec.default.clone(),
                    spec.initial_text.clone(),
                    spec.allow_empty,
                    spec.show_default,
                    spec.post_completion_text.clone(),
                    spec.report,
                    spec.theme,
                )
            };
            run_blocking("input", move || {
                let theme = theme.build();
                let mut p = Input::<String>::with_theme(theme.as_ref())
                    .with_prompt(prompt)
                    .allow_empty(allow_empty)
                    .show_default(show_default)
                    .report(report);
                if let Some(d) = default {
                    p = p.default(d);
                }
                if let Some(t) = initial_text {
                    p = p.with_initial_text(t);
                }
                if let Some(t) = post {
                    p = p.with_post_completion_text(t);
                }
                p.interact_text()
            })
            .await
        });
    }
}

// ---------------------------------------------------------------------------
// Password
// ---------------------------------------------------------------------------

#[derive(Default)]
struct PasswordSpec {
    prompt: String,
    allow_empty_password: bool,
    confirmation: Option<(String, String)>,
    report: bool,
    theme: ThemeChoice,
}

struct PasswordBuilder(RefCell<PasswordSpec>);

impl PasswordBuilder {
    fn new() -> Self {
        Self(RefCell::new(PasswordSpec {
            report: true,
            ..Default::default()
        }))
    }
}

impl UserData for PasswordBuilder {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("prompt", |_, this, v: String| {
            this.0.borrow_mut().prompt = v;
            Ok(())
        });
        methods.add_method("allow_empty_password", |_, this, v: bool| {
            this.0.borrow_mut().allow_empty_password = v;
            Ok(())
        });
        methods.add_method(
            "with_confirmation",
            |_, this, (prompt, mismatch_err): (String, String)| {
                this.0.borrow_mut().confirmation = Some((prompt, mismatch_err));
                Ok(())
            },
        );
        methods.add_method("report", |_, this, v: bool| {
            this.0.borrow_mut().report = v;
            Ok(())
        });
        methods.add_method("theme", |_, this, name: String| {
            this.0.borrow_mut().theme = ThemeChoice::parse(&name)?;
            Ok(())
        });
        methods.add_async_method("interact", |_, this, ()| async move {
            let (prompt, allow_empty_password, confirmation, report, theme) = {
                let spec = this.0.borrow();
                (
                    spec.prompt.clone(),
                    spec.allow_empty_password,
                    spec.confirmation.clone(),
                    spec.report,
                    spec.theme,
                )
            };
            run_blocking("password", move || {
                let theme = theme.build();
                let mut p = Password::with_theme(theme.as_ref())
                    .with_prompt(prompt)
                    .allow_empty_password(allow_empty_password)
                    .report(report);
                if let Some((prompt, mismatch)) = confirmation {
                    p = p.with_confirmation(prompt, mismatch);
                }
                p.interact()
            })
            .await
        });
    }
}

// ---------------------------------------------------------------------------
// Select
// ---------------------------------------------------------------------------

#[derive(Default)]
struct SelectSpec {
    prompt: Option<String>,
    items: Vec<String>,
    default: usize,
    max_length: Option<usize>,
    clear: bool,
    report: bool,
    theme: ThemeChoice,
}

struct SelectBuilder(RefCell<SelectSpec>);

impl SelectBuilder {
    fn new() -> Self {
        Self(RefCell::new(SelectSpec {
            clear: true,
            report: true,
            ..Default::default()
        }))
    }
}

impl UserData for SelectBuilder {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("prompt", |_, this, v: String| {
            this.0.borrow_mut().prompt = Some(v);
            Ok(())
        });
        methods.add_method("items", |_, this, v: Vec<String>| {
            this.0.borrow_mut().items = v;
            Ok(())
        });
        methods.add_method("item", |_, this, v: String| {
            this.0.borrow_mut().items.push(v);
            Ok(())
        });
        methods.add_method("default", |_, this, one_based: i64| {
            let len = this.0.borrow().items.len();
            let zero = to_zero_based("default", Some(one_based), len)?.expect("Some in, Some out");
            this.0.borrow_mut().default = zero;
            Ok(())
        });
        methods.add_method("max_length", |_, this, v: usize| {
            this.0.borrow_mut().max_length = Some(v);
            Ok(())
        });
        methods.add_method("clear", |_, this, v: bool| {
            this.0.borrow_mut().clear = v;
            Ok(())
        });
        methods.add_method("report", |_, this, v: bool| {
            this.0.borrow_mut().report = v;
            Ok(())
        });
        methods.add_method("theme", |_, this, name: String| {
            this.0.borrow_mut().theme = ThemeChoice::parse(&name)?;
            Ok(())
        });
        methods.add_async_method("interact", |_, this, ()| async move {
            let (prompt, items, default, max_length, clear, report, theme) = {
                let spec = this.0.borrow();
                require_items("select", &spec.items)?;
                (
                    spec.prompt.clone(),
                    spec.items.clone(),
                    spec.default,
                    spec.max_length,
                    spec.clear,
                    spec.report,
                    spec.theme,
                )
            };
            let index = run_blocking("select", move || {
                let theme = theme.build();
                let mut p = Select::with_theme(theme.as_ref())
                    .items(&items)
                    .default(default)
                    .clear(clear)
                    .report(report);
                if let Some(p2) = prompt {
                    p = p.with_prompt(p2);
                }
                if let Some(m) = max_length {
                    p = p.max_length(m);
                }
                p.interact()
            })
            .await?;
            Ok(index as i64 + 1)
        });
        methods.add_async_method("interact_opt", |_, this, ()| async move {
            let (prompt, items, default, max_length, clear, report, theme) = {
                let spec = this.0.borrow();
                require_items("select", &spec.items)?;
                (
                    spec.prompt.clone(),
                    spec.items.clone(),
                    spec.default,
                    spec.max_length,
                    spec.clear,
                    spec.report,
                    spec.theme,
                )
            };
            let index = run_blocking_opt("select", move || {
                let theme = theme.build();
                let mut p = Select::with_theme(theme.as_ref())
                    .items(&items)
                    .default(default)
                    .clear(clear)
                    .report(report);
                if let Some(p2) = prompt {
                    p = p.with_prompt(p2);
                }
                if let Some(m) = max_length {
                    p = p.max_length(m);
                }
                p.interact_opt()
            })
            .await?;
            Ok(index.map(|i| i as i64 + 1))
        });
    }
}

// ---------------------------------------------------------------------------
// MultiSelect
// ---------------------------------------------------------------------------

#[derive(Default)]
struct MultiSelectSpec {
    prompt: Option<String>,
    items: Vec<String>,
    defaults: Vec<bool>,
    max_length: Option<usize>,
    clear: bool,
    report: bool,
    theme: ThemeChoice,
}

struct MultiSelectBuilder(RefCell<MultiSelectSpec>);

impl MultiSelectBuilder {
    fn new() -> Self {
        Self(RefCell::new(MultiSelectSpec {
            clear: true,
            report: true,
            ..Default::default()
        }))
    }
}

impl UserData for MultiSelectBuilder {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("prompt", |_, this, v: String| {
            this.0.borrow_mut().prompt = Some(v);
            Ok(())
        });
        methods.add_method("items", |_, this, v: Vec<String>| {
            let mut spec = this.0.borrow_mut();
            spec.defaults = vec![false; v.len()];
            spec.items = v;
            Ok(())
        });
        methods.add_method("item", |_, this, (v, checked): (String, Option<bool>)| {
            let mut spec = this.0.borrow_mut();
            spec.items.push(v);
            spec.defaults.push(checked.unwrap_or(false));
            Ok(())
        });
        // :checked(i) marks the i'th (1-based) item pre-selected. Plural
        // `defaults({...})` sets the whole array at once for a caller who
        // already has the list of indices to check.
        methods.add_method("checked", |_, this, one_based: i64| {
            let len = this.0.borrow().items.len();
            let zero = to_zero_based("checked", Some(one_based), len)?.expect("Some in, Some out");
            this.0.borrow_mut().defaults[zero] = true;
            Ok(())
        });
        methods.add_method("defaults", |_, this, one_based_list: Vec<i64>| {
            let len = this.0.borrow().items.len();
            let mut flags = vec![false; len];
            for one_based in one_based_list {
                let zero =
                    to_zero_based("defaults", Some(one_based), len)?.expect("Some in, Some out");
                flags[zero] = true;
            }
            this.0.borrow_mut().defaults = flags;
            Ok(())
        });
        methods.add_method("max_length", |_, this, v: usize| {
            this.0.borrow_mut().max_length = Some(v);
            Ok(())
        });
        methods.add_method("clear", |_, this, v: bool| {
            this.0.borrow_mut().clear = v;
            Ok(())
        });
        methods.add_method("report", |_, this, v: bool| {
            this.0.borrow_mut().report = v;
            Ok(())
        });
        methods.add_method("theme", |_, this, name: String| {
            this.0.borrow_mut().theme = ThemeChoice::parse(&name)?;
            Ok(())
        });
        methods.add_async_method("interact", |_, this, ()| async move {
            let (prompt, items, defaults, max_length, clear, report, theme) = {
                let spec = this.0.borrow();
                require_items("multi_select", &spec.items)?;
                (
                    spec.prompt.clone(),
                    spec.items.clone(),
                    spec.defaults.clone(),
                    spec.max_length,
                    spec.clear,
                    spec.report,
                    spec.theme,
                )
            };
            let indices = run_blocking("multi_select", move || {
                let theme = theme.build();
                let mut p = MultiSelect::with_theme(theme.as_ref())
                    .items(&items)
                    .defaults(&defaults)
                    .clear(clear)
                    .report(report);
                if let Some(p2) = prompt {
                    p = p.with_prompt(p2);
                }
                if let Some(m) = max_length {
                    p = p.max_length(m);
                }
                p.interact()
            })
            .await?;
            Ok(indices
                .into_iter()
                .map(|i| i as i64 + 1)
                .collect::<Vec<_>>())
        });
        methods.add_async_method("interact_opt", |_, this, ()| async move {
            let (prompt, items, defaults, max_length, clear, report, theme) = {
                let spec = this.0.borrow();
                require_items("multi_select", &spec.items)?;
                (
                    spec.prompt.clone(),
                    spec.items.clone(),
                    spec.defaults.clone(),
                    spec.max_length,
                    spec.clear,
                    spec.report,
                    spec.theme,
                )
            };
            let indices = run_blocking_opt("multi_select", move || {
                let theme = theme.build();
                let mut p = MultiSelect::with_theme(theme.as_ref())
                    .items(&items)
                    .defaults(&defaults)
                    .clear(clear)
                    .report(report);
                if let Some(p2) = prompt {
                    p = p.with_prompt(p2);
                }
                if let Some(m) = max_length {
                    p = p.max_length(m);
                }
                p.interact_opt()
            })
            .await?;
            Ok(indices.map(|v| v.into_iter().map(|i| i as i64 + 1).collect::<Vec<_>>()))
        });
    }
}

// ---------------------------------------------------------------------------
// Module
// ---------------------------------------------------------------------------

/// Build the `ui.prompt` table: shorthands plus builders.
pub fn module(lua: &Lua) -> LuaResult<Table> {
    let module = lua.create_table()?;

    // ---- shorthands --------------------------------------------------

    // Shorthands build against `ThemeChoice::Auto` -- the same `ito.colors()`
    // switch every other module's output tracks -- rather than dialoguer's own
    // default `SimpleTheme`, so a prompt's colour follows the run the way
    // `string:red()` does. A script wanting the other themes uses the builder.
    module.set(
        "confirm",
        lua.create_async_function(|_, (msg, default): (String, Option<bool>)| async move {
            run_blocking("confirm", move || {
                let theme = ThemeChoice::Auto.build();
                let mut p = Confirm::with_theme(theme.as_ref()).with_prompt(msg);
                if let Some(d) = default {
                    p = p.default(d);
                }
                p.interact()
            })
            .await
        })?,
    )?;

    module.set(
        "input",
        lua.create_async_function(|_, (msg, default): (String, Option<String>)| async move {
            run_blocking("input", move || {
                let theme = ThemeChoice::Auto.build();
                let mut p = Input::<String>::with_theme(theme.as_ref()).with_prompt(msg);
                if let Some(d) = default {
                    p = p.default(d);
                }
                p.interact_text()
            })
            .await
        })?,
    )?;

    module.set(
        "password",
        lua.create_async_function(|_, (msg, confirm): (String, Option<bool>)| async move {
            run_blocking("password", move || {
                let theme = ThemeChoice::Auto.build();
                let mut p = Password::with_theme(theme.as_ref()).with_prompt(msg);
                if confirm.unwrap_or(false) {
                    p = p.with_confirmation("Confirm", "passwords didn't match");
                }
                p.interact()
            })
            .await
        })?,
    )?;

    module.set(
        "select",
        lua.create_async_function(
            |_, (msg, items, default): (String, Vec<String>, Option<i64>)| async move {
                require_items("select", &items)?;
                let default = to_zero_based("default", default, items.len())?;
                let index = run_blocking("select", move || {
                    let theme = ThemeChoice::Auto.build();
                    let mut p = Select::with_theme(theme.as_ref())
                        .with_prompt(msg)
                        .items(&items);
                    if let Some(d) = default {
                        p = p.default(d);
                    }
                    p.interact()
                })
                .await?;
                Ok(index as i64 + 1)
            },
        )?,
    )?;

    module.set(
        "multi_select",
        lua.create_async_function(
            |_, (msg, items, defaults): (String, Vec<String>, Option<Vec<i64>>)| async move {
                require_items("multi_select", &items)?;
                let mut checked = vec![false; items.len()];
                if let Some(defaults) = defaults {
                    for one_based in defaults {
                        let zero = to_zero_based("defaults", Some(one_based), items.len())?
                            .expect("Some(_) in, Some(_) out");
                        checked[zero] = true;
                    }
                }
                let indices = run_blocking("multi_select", move || {
                    let theme = ThemeChoice::Auto.build();
                    MultiSelect::with_theme(theme.as_ref())
                        .with_prompt(msg)
                        .items(&items)
                        .defaults(&checked)
                        .interact()
                })
                .await?;
                Ok(indices
                    .into_iter()
                    .map(|i| i as i64 + 1)
                    .collect::<Vec<_>>())
            },
        )?,
    )?;

    // ---- builders ------------------------------------------------------

    module.set(
        "confirm_builder",
        lua.create_function(|_, ()| Ok(ConfirmBuilder::new()))?,
    )?;
    module.set(
        "input_builder",
        lua.create_function(|_, ()| Ok(InputBuilder::new()))?,
    )?;
    module.set(
        "password_builder",
        lua.create_function(|_, ()| Ok(PasswordBuilder::new()))?,
    )?;
    module.set(
        "select_builder",
        lua.create_function(|_, ()| Ok(SelectBuilder::new()))?,
    )?;
    module.set(
        "multi_select_builder",
        lua.create_function(|_, ()| Ok(MultiSelectBuilder::new()))?,
    )?;

    Ok(module)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `select`/`multi_select` validate bounds before ever touching the
    /// terminal, so those paths are testable with no TTY at all; the async
    /// functions still need a runtime to drive `exec_async`.
    fn eval(script: &str) -> LuaResult<()> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build test runtime");
        let local = tokio::task::LocalSet::new();
        local.block_on(&runtime, async {
            let lua = Lua::new();
            let table = module(&lua)?;
            lua.globals().set("prompt", table)?;
            lua.load(script).exec_async().await
        })
    }

    #[test]
    fn select_rejects_empty_items() {
        let err = eval(r#"prompt.select("x", {})"#).unwrap_err();
        assert!(err.to_string().contains("items must not be empty"), "{err}");
    }

    #[test]
    fn select_rejects_out_of_range_default() {
        let err = eval(r#"prompt.select("x", {"a", "b"}, 3)"#).unwrap_err();
        assert!(err.to_string().contains("out of range"), "{err}");
        let err = eval(r#"prompt.select("x", {"a", "b"}, 0)"#).unwrap_err();
        assert!(err.to_string().contains("out of range"), "{err}");
    }

    #[test]
    fn multi_select_rejects_empty_items() {
        let err = eval(r#"prompt.multi_select("x", {})"#).unwrap_err();
        assert!(err.to_string().contains("items must not be empty"), "{err}");
    }

    #[test]
    fn multi_select_rejects_out_of_range_defaults() {
        let err = eval(r#"prompt.multi_select("x", {"a", "b"}, {1, 5})"#).unwrap_err();
        assert!(err.to_string().contains("out of range"), "{err}");
    }

    #[test]
    fn builder_rejects_unknown_theme() {
        let err = eval(r#"prompt.confirm_builder():theme("neon")"#).unwrap_err();
        assert!(err.to_string().contains("unknown theme"), "{err}");
    }

    #[test]
    fn builder_select_validates_default_bounds_before_interact() {
        let err = eval(
            r#"
            local b = prompt.select_builder()
            b:items({"a", "b"})
            b:default(9)
            "#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("out of range"), "{err}");
    }

    #[test]
    fn builder_select_interact_rejects_empty_items() {
        let err = eval(r#"prompt.select_builder():interact()"#).unwrap_err();
        assert!(err.to_string().contains("items must not be empty"), "{err}");
    }

    #[test]
    fn builder_multi_select_checked_and_defaults() {
        // No :interact() here (needs a TTY); this just exercises the spec
        // mutation paths for bounds-checking.
        eval(
            r#"
            local b = prompt.multi_select_builder()
            b:items({"a", "b", "c"})
            b:checked(2)
            b:defaults({1, 3})
            "#,
        )
        .unwrap();
    }
}
