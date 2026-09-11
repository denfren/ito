//! `ui.prompt` — interactive terminal prompts, backed by `dialoguer`.
//!
//! The chapter `docs/src/modules/ui-prompt.md` is the surface description.
//! Implementation notes:
//!
//! - Two layers: a **shorthand** per kind, and a **builder** exposing the
//!   dialoguer knobs. Both fill the same `*Spec` and run through the same
//!   `*_run` function, so they cannot drift. Specs hold the script's raw
//!   values (1-based indices included); everything is validated at
//!   `:interact()`, so setter call order never matters.
//! - All prompts run on a blocking thread (`spawn_blocking`): a terminal read
//!   has no async form and the runtime is single-threaded.
//! - Prompts are serialised by an async mutex (a terminal has one cursor) and
//!   run with live `ui.progress` bars paused.
//! - **Ctrl-C** raises a Luau error: dialoguer/console `raise(SIGINT)` on it,
//!   so SIGINT is given a no-op handler for the duration of the prompt and the
//!   read then fails with `Interrupted`. A guard restores the previous handler
//!   and the cursor however the prompt ends.
//! - An abandoned prompt (its task cancelled) cannot be interrupted: the
//!   blocking read keeps going until a key arrives.
//! - No custom themes, live validators or history/completion: those take Rust
//!   callbacks run on the reading thread, which a `!Send` Lua cannot serve.
//!
//! Host-only (`ito`); see [`crate::scripting::ui`].

use crate::scripting::fmt::ansi;
use dialoguer::theme::{ColorfulTheme, SimpleTheme, Theme};
use dialoguer::{Confirm, Input, MultiSelect, Password, Select};
use mlua::{Lua, Result as LuaResult, Table, UserData, UserDataMethods, Value};
use std::cell::RefCell;
use std::io::IsTerminal;
use std::rc::Rc;

// ---------------------------------------------------------------------------
// Shared plumbing
// ---------------------------------------------------------------------------

/// One prompt at a time.
static PROMPT_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// A bool argument: `true`/`false` only -- no truthiness coercion.
fn flag(v: &Value, what: &str) -> LuaResult<bool> {
    match v {
        Value::Boolean(b) => Ok(*b),
        other => Err(mlua::Error::runtime(format!(
            "ui.prompt: {what} expects a boolean, got {}",
            other.type_name()
        ))),
    }
}

/// An optional bool argument: `nil` is absent, anything else must be a boolean.
fn opt_flag(v: Option<Value>, what: &str) -> LuaResult<Option<bool>> {
    match v {
        None | Some(Value::Nil) => Ok(None),
        Some(v) => flag(&v, what).map(Some),
    }
}

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

    /// `auto` tracks `fmt.colors()`, detected against stderr.
    fn build(self) -> Box<dyn Theme> {
        let colorful = match self {
            ThemeChoice::Auto => ansi::enabled_stderr(),
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

/// Prompts draw on stderr; without a terminal there is nobody to ask.
fn attended() -> bool {
    // Tests never prompt.
    !cfg!(test) && std::io::stderr().is_terminal()
}

extern "C" fn ignore_sigint(_: libc::c_int) {}

/// While alive: SIGINT is a no-op (so Ctrl-C surfaces as an `Interrupted` read
/// error instead of killing the process). Dropping restores the previous
/// handler and shows the cursor dialoguer may have left hidden.
struct TermGuard(libc::sighandler_t);

impl TermGuard {
    fn new() -> Self {
        Self(unsafe { libc::signal(libc::SIGINT, ignore_sigint as *const () as libc::sighandler_t) })
    }
}

impl Drop for TermGuard {
    fn drop(&mut self) {
        unsafe { libc::signal(libc::SIGINT, self.0) };
        let _ = dialoguer::console::Term::stderr().show_cursor();
    }
}

/// Run a dialoguer interaction. `f(opt)` returns `Some` on an answer and
/// `None` for Escape (only when `opt`). `fallback` is what a non-terminal
/// session answers (from `:or_default()`), else it is an error.
async fn run_prompt<T, F>(name: &'static str, fallback: Option<T>, f: F) -> LuaResult<Option<T>>
where
    T: Send + 'static,
    F: FnOnce() -> dialoguer::Result<Option<T>> + Send + 'static,
{
    if !attended() {
        return match fallback {
            Some(v) => Ok(Some(v)),
            None => Err(mlua::Error::runtime(format!(
                "ui.prompt.{name}: stderr is not a terminal, so there is nobody to ask \
                 (give the prompt a default and call :or_default(true) to accept it instead)"
            ))),
        };
    }
    let _lock = PROMPT_LOCK.lock().await;
    let _pause = super::progress::pause();
    tokio::task::spawn_blocking(move || {
        let _guard = TermGuard::new();
        f()
    })
    .await
    .map_err(|e| mlua::Error::runtime(format!("ui.prompt.{name}: task failed: {e}")))?
    .map_err(|e| mlua::Error::runtime(format!("ui.prompt.{name}: {e}")))
}

/// 1-based index argument to 0-based; `None` stays `None`.
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

/// Builder methods every kind shares: `report`, `theme`.
macro_rules! common_setters {
    ($m:ident) => {
        $m.add_method("report", |_, this, v: Value| {
            this.0.borrow_mut().report = flag(&v, "report")?;
            Ok(this.clone())
        });
        $m.add_method("theme", |_, this, name: String| {
            this.0.borrow_mut().theme = ThemeChoice::parse(&name)?;
            Ok(this.clone())
        });
    };
}

/// A boolean setter for field `$f`, exposed as `$name`.
macro_rules! flag_setter {
    ($m:ident, $name:literal, $f:ident) => {
        $m.add_method($name, |_, this, v: Value| {
            this.0.borrow_mut().$f = flag(&v, $name)?;
            Ok(this.clone())
        });
    };
}

// ---------------------------------------------------------------------------
// Confirm
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct ConfirmSpec {
    prompt: String,
    default: Option<bool>,
    show_default: bool,
    wait_for_newline: bool,
    report: bool,
    or_default: bool,
    theme: ThemeChoice,
}

impl Default for ConfirmSpec {
    fn default() -> Self {
        Self {
            prompt: String::new(),
            default: None,
            show_default: true,
            wait_for_newline: false,
            report: true,
            or_default: false,
            theme: ThemeChoice::Auto,
        }
    }
}

async fn confirm_run(s: ConfirmSpec, opt: bool) -> LuaResult<Option<bool>> {
    let fallback = s.default.filter(|_| s.or_default);
    run_prompt("confirm", fallback, move || {
        let theme = s.theme.build();
        let mut p = Confirm::with_theme(theme.as_ref())
            .with_prompt(s.prompt)
            .show_default(s.show_default)
            .wait_for_newline(s.wait_for_newline)
            .report(s.report);
        if let Some(d) = s.default {
            p = p.default(d);
        }
        if opt { p.interact_opt() } else { p.interact().map(Some) }
    })
    .await
}

#[derive(Clone)]
struct ConfirmBuilder(Rc<RefCell<ConfirmSpec>>);

impl UserData for ConfirmBuilder {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("prompt", |_, this, v: String| {
            this.0.borrow_mut().prompt = v;
            Ok(this.clone())
        });
        methods.add_method("default", |_, this, v: Value| {
            this.0.borrow_mut().default = Some(flag(&v, "default")?);
            Ok(this.clone())
        });
        flag_setter!(methods, "show_default", show_default);
        flag_setter!(methods, "wait_for_newline", wait_for_newline);
        flag_setter!(methods, "or_default", or_default);
        common_setters!(methods);
        methods.add_async_method("interact", |_, this, ()| async move {
            let spec = this.0.borrow().clone();
            Ok(confirm_run(spec, false).await?.unwrap_or_default())
        });
        methods.add_async_method("interact_opt", |_, this, ()| async move {
            let spec = this.0.borrow().clone();
            confirm_run(spec, true).await
        });
    }
}

// ---------------------------------------------------------------------------
// Input
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct InputSpec {
    prompt: String,
    default: Option<String>,
    initial_text: Option<String>,
    allow_empty: bool,
    show_default: bool,
    post_completion_text: Option<String>,
    report: bool,
    or_default: bool,
    theme: ThemeChoice,
}

impl Default for InputSpec {
    fn default() -> Self {
        Self {
            prompt: String::new(),
            default: None,
            initial_text: None,
            allow_empty: false,
            show_default: true,
            post_completion_text: None,
            report: true,
            or_default: false,
            theme: ThemeChoice::Auto,
        }
    }
}

async fn input_run(s: InputSpec) -> LuaResult<String> {
    let fallback = s.default.clone().filter(|_| s.or_default);
    let answer = run_prompt("input", fallback, move || {
        let theme = s.theme.build();
        let mut p = Input::<String>::with_theme(theme.as_ref())
            .with_prompt(s.prompt)
            .allow_empty(s.allow_empty)
            .show_default(s.show_default)
            .report(s.report);
        if let Some(d) = s.default {
            p = p.default(d);
        }
        if let Some(t) = s.initial_text {
            p = p.with_initial_text(t);
        }
        if let Some(t) = s.post_completion_text {
            p = p.with_post_completion_text(t);
        }
        p.interact_text().map(Some)
    })
    .await?;
    Ok(answer.unwrap_or_default())
}

#[derive(Clone)]
struct InputBuilder(Rc<RefCell<InputSpec>>);

impl UserData for InputBuilder {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("prompt", |_, this, v: String| {
            this.0.borrow_mut().prompt = v;
            Ok(this.clone())
        });
        methods.add_method("default", |_, this, v: String| {
            this.0.borrow_mut().default = Some(v);
            Ok(this.clone())
        });
        methods.add_method("initial_text", |_, this, v: String| {
            this.0.borrow_mut().initial_text = Some(v);
            Ok(this.clone())
        });
        methods.add_method("post_completion_text", |_, this, v: String| {
            this.0.borrow_mut().post_completion_text = Some(v);
            Ok(this.clone())
        });
        flag_setter!(methods, "allow_empty", allow_empty);
        flag_setter!(methods, "show_default", show_default);
        flag_setter!(methods, "or_default", or_default);
        common_setters!(methods);
        methods.add_async_method("interact", |_, this, ()| async move {
            let spec = this.0.borrow().clone();
            input_run(spec).await
        });
    }
}

// ---------------------------------------------------------------------------
// Password
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct PasswordSpec {
    prompt: String,
    allow_empty_password: bool,
    confirmation: Option<(String, String)>,
    report: bool,
    theme: ThemeChoice,
}

impl Default for PasswordSpec {
    fn default() -> Self {
        Self {
            prompt: String::new(),
            allow_empty_password: false,
            confirmation: None,
            report: true,
            theme: ThemeChoice::Auto,
        }
    }
}

async fn password_run(s: PasswordSpec) -> LuaResult<String> {
    let answer = run_prompt("password", None, move || {
        let theme = s.theme.build();
        let mut p = Password::with_theme(theme.as_ref())
            .with_prompt(s.prompt)
            .allow_empty_password(s.allow_empty_password)
            .report(s.report);
        if let Some((prompt, mismatch)) = s.confirmation {
            p = p.with_confirmation(prompt, mismatch);
        }
        p.interact().map(Some)
    })
    .await?;
    Ok(answer.unwrap_or_default())
}

#[derive(Clone)]
struct PasswordBuilder(Rc<RefCell<PasswordSpec>>);

impl UserData for PasswordBuilder {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("prompt", |_, this, v: String| {
            this.0.borrow_mut().prompt = v;
            Ok(this.clone())
        });
        flag_setter!(methods, "allow_empty_password", allow_empty_password);
        methods.add_method(
            "with_confirmation",
            |_, this, (prompt, mismatch_err): (String, String)| {
                this.0.borrow_mut().confirmation = Some((prompt, mismatch_err));
                Ok(this.clone())
            },
        );
        common_setters!(methods);
        methods.add_async_method("interact", |_, this, ()| async move {
            let spec = this.0.borrow().clone();
            password_run(spec).await
        });
    }
}

// ---------------------------------------------------------------------------
// Select
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct SelectSpec {
    prompt: Option<String>,
    items: Vec<String>,
    /// 1-based, validated at run time.
    default: Option<i64>,
    max_length: Option<usize>,
    clear: bool,
    report: bool,
    or_default: bool,
    theme: ThemeChoice,
}

impl Default for SelectSpec {
    fn default() -> Self {
        Self {
            prompt: None,
            items: Vec::new(),
            default: None,
            max_length: None,
            clear: true,
            report: true,
            or_default: false,
            theme: ThemeChoice::Auto,
        }
    }
}

/// The chosen 1-based index and its item, or `None` on Escape.
async fn select_run(s: SelectSpec, opt: bool) -> LuaResult<Option<(i64, String)>> {
    require_items("select", &s.items)?;
    let default = to_zero_based("default", s.default, s.items.len())?;
    let items = s.items.clone();
    let fallback = default.filter(|_| s.or_default);
    let index = run_prompt("select", fallback, move || {
        let theme = s.theme.build();
        let mut p = Select::with_theme(theme.as_ref())
            .items(&s.items)
            .default(default.unwrap_or(0))
            .clear(s.clear)
            .report(s.report);
        if let Some(t) = s.prompt {
            p = p.with_prompt(t);
        }
        if let Some(m) = s.max_length {
            p = p.max_length(m);
        }
        if opt { p.interact_opt() } else { p.interact().map(Some) }
    })
    .await?;
    Ok(index.map(|i| (i as i64 + 1, items[i].clone())))
}

#[derive(Clone)]
struct SelectBuilder(Rc<RefCell<SelectSpec>>);

impl UserData for SelectBuilder {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("prompt", |_, this, v: String| {
            this.0.borrow_mut().prompt = Some(v);
            Ok(this.clone())
        });
        methods.add_method("items", |_, this, v: Vec<String>| {
            this.0.borrow_mut().items = v;
            Ok(this.clone())
        });
        methods.add_method("item", |_, this, v: String| {
            this.0.borrow_mut().items.push(v);
            Ok(this.clone())
        });
        methods.add_method("default", |_, this, one_based: i64| {
            this.0.borrow_mut().default = Some(one_based);
            Ok(this.clone())
        });
        methods.add_method("max_length", |_, this, v: usize| {
            this.0.borrow_mut().max_length = Some(v);
            Ok(this.clone())
        });
        flag_setter!(methods, "clear", clear);
        flag_setter!(methods, "or_default", or_default);
        common_setters!(methods);
        methods.add_async_method("interact", |_, this, ()| async move {
            let spec = this.0.borrow().clone();
            let (i, item) = select_run(spec, false).await?.unzip();
            Ok((i, item))
        });
        methods.add_async_method("interact_opt", |_, this, ()| async move {
            let spec = this.0.borrow().clone();
            let (i, item) = select_run(spec, true).await?.unzip();
            Ok((i, item))
        });
    }
}

// ---------------------------------------------------------------------------
// MultiSelect
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct MultiSelectSpec {
    prompt: Option<String>,
    items: Vec<String>,
    /// Pre-checked, 1-based, validated at run time.
    checked: Vec<i64>,
    all: bool,
    max_length: Option<usize>,
    clear: bool,
    report: bool,
    theme: ThemeChoice,
}

impl Default for MultiSelectSpec {
    fn default() -> Self {
        Self {
            prompt: None,
            items: Vec::new(),
            checked: Vec::new(),
            all: false,
            max_length: None,
            clear: true,
            report: true,
            theme: ThemeChoice::Auto,
        }
    }
}

async fn multi_select_run(s: MultiSelectSpec, opt: bool) -> LuaResult<Option<Vec<i64>>> {
    require_items("multi_select", &s.items)?;
    let mut defaults = vec![s.all; s.items.len()];
    for i in &s.checked {
        let zero = to_zero_based("checked", Some(*i), s.items.len())?.expect("Some in, Some out");
        defaults[zero] = true;
    }
    let picked = run_prompt("multi_select", None, move || {
        let theme = s.theme.build();
        let mut p = MultiSelect::with_theme(theme.as_ref())
            .items(&s.items)
            .defaults(&defaults)
            .clear(s.clear)
            .report(s.report);
        if let Some(t) = s.prompt {
            p = p.with_prompt(t);
        }
        if let Some(m) = s.max_length {
            p = p.max_length(m);
        }
        if opt { p.interact_opt() } else { p.interact().map(Some) }
    })
    .await?;
    Ok(picked.map(|v| v.into_iter().map(|i| i as i64 + 1).collect()))
}

#[derive(Clone)]
struct MultiSelectBuilder(Rc<RefCell<MultiSelectSpec>>);

impl UserData for MultiSelectBuilder {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("prompt", |_, this, v: String| {
            this.0.borrow_mut().prompt = Some(v);
            Ok(this.clone())
        });
        methods.add_method("items", |_, this, v: Vec<String>| {
            this.0.borrow_mut().items = v;
            Ok(this.clone())
        });
        methods.add_method("item", |_, this, (v, checked): (String, Option<Value>)| {
            let mut spec = this.0.borrow_mut();
            spec.items.push(v);
            if opt_flag(checked, "item's checked")?.unwrap_or(false) {
                let n = spec.items.len() as i64;
                spec.checked.push(n);
            }
            drop(spec);
            Ok(this.clone())
        });
        methods.add_method("checked", |_, this, one_based: i64| {
            this.0.borrow_mut().checked.push(one_based);
            Ok(this.clone())
        });
        methods.add_method("all", |_, this, ()| {
            this.0.borrow_mut().all = true;
            Ok(this.clone())
        });
        methods.add_method("defaults", |_, this, list: Vec<i64>| {
            this.0.borrow_mut().checked = list;
            Ok(this.clone())
        });
        methods.add_method("max_length", |_, this, v: usize| {
            this.0.borrow_mut().max_length = Some(v);
            Ok(this.clone())
        });
        flag_setter!(methods, "clear", clear);
        common_setters!(methods);
        methods.add_async_method("interact", |_, this, ()| async move {
            let spec = this.0.borrow().clone();
            Ok(multi_select_run(spec, false).await?.unwrap_or_default())
        });
        methods.add_async_method("interact_opt", |_, this, ()| async move {
            let spec = this.0.borrow().clone();
            multi_select_run(spec, true).await
        });
    }
}

// ---------------------------------------------------------------------------
// Module
// ---------------------------------------------------------------------------

/// Build the `ui.prompt` table: shorthands plus builders.
pub fn module(lua: &Lua) -> LuaResult<Table> {
    let module = lua.create_table()?;

    // Shorthands fill the same specs as the builders, with the auto theme.
    module.set(
        "confirm",
        lua.create_async_function(async |_, (msg, default): (String, Option<Value>)| {
            let default = opt_flag(default, "default")?;
            let spec = ConfirmSpec {
                prompt: msg,
                default,
                ..Default::default()
            };
            Ok(confirm_run(spec, false).await?.unwrap_or_default())
        })?,
    )?;
    module.set(
        "input",
        lua.create_async_function(async |_, (msg, default): (String, Option<String>)| {
            input_run(InputSpec {
                prompt: msg,
                default,
                ..Default::default()
            })
            .await
        })?,
    )?;
    module.set(
        "password",
        lua.create_async_function(async |_, (msg, confirm): (String, Option<Value>)| {
            let confirm = opt_flag(confirm, "confirm")?.unwrap_or(false);
            password_run(PasswordSpec {
                prompt: msg,
                confirmation: confirm
                    .then(|| ("Confirm".to_string(), "passwords didn't match".to_string())),
                ..Default::default()
            })
            .await
        })?,
    )?;
    module.set(
        "select",
        lua.create_async_function(
            async |_, (msg, items, default): (String, Vec<String>, Option<i64>)| {
                let spec = SelectSpec {
                    prompt: Some(msg),
                    items,
                    default,
                    ..Default::default()
                };
                let (i, item) = select_run(spec, false).await?.unzip();
                Ok((i, item))
            },
        )?,
    )?;
    module.set(
        "multi_select",
        lua.create_async_function(
            async |_, (msg, items, defaults): (String, Vec<String>, Option<Vec<i64>>)| {
                let spec = MultiSelectSpec {
                    prompt: Some(msg),
                    items,
                    checked: defaults.unwrap_or_default(),
                    ..Default::default()
                };
                Ok(multi_select_run(spec, false).await?.unwrap_or_default())
            },
        )?,
    )?;

    module.set(
        "confirm_builder",
        lua.create_function(|_, ()| {
            Ok(ConfirmBuilder(Rc::new(RefCell::new(ConfirmSpec::default()))))
        })?,
    )?;
    module.set(
        "input_builder",
        lua.create_function(|_, ()| Ok(InputBuilder(Rc::new(RefCell::new(InputSpec::default())))))?,
    )?;
    module.set(
        "password_builder",
        lua.create_function(|_, ()| {
            Ok(PasswordBuilder(Rc::new(RefCell::new(
                PasswordSpec::default(),
            ))))
        })?,
    )?;
    module.set(
        "select_builder",
        lua.create_function(|_, ()| {
            Ok(SelectBuilder(Rc::new(RefCell::new(SelectSpec::default()))))
        })?,
    )?;
    module.set(
        "multi_select_builder",
        lua.create_function(|_, ()| {
            Ok(MultiSelectBuilder(Rc::new(RefCell::new(
                MultiSelectSpec::default(),
            ))))
        })?,
    )?;

    Ok(module)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Validation happens before the terminal is touched, and tests are never
    /// "attended", so everything here runs without a TTY.
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

    fn fails(script: &str, needle: &str) {
        let err = eval(script).unwrap_err();
        assert!(err.to_string().contains(needle), "{err}");
    }

    #[test]
    fn select_rejects_empty_items() {
        fails(r#"prompt.select("x", {})"#, "items must not be empty");
    }

    #[test]
    fn select_rejects_out_of_range_default() {
        fails(r#"prompt.select("x", {"a", "b"}, 3)"#, "out of range");
        fails(r#"prompt.select("x", {"a", "b"}, 0)"#, "out of range");
    }

    #[test]
    fn multi_select_validation() {
        fails(r#"prompt.multi_select("x", {})"#, "items must not be empty");
        fails(r#"prompt.multi_select("x", {"a", "b"}, {1, 5})"#, "out of range");
    }

    #[test]
    fn builder_rejects_unknown_theme() {
        fails(r#"prompt.confirm_builder():theme("neon")"#, "unknown theme");
    }

    #[test]
    fn index_validation_is_deferred_to_interact_so_order_is_free() {
        // default before items is fine; the bound is checked when it runs.
        fails(
            r#"prompt.select_builder():default(2):items({"a"}):interact()"#,
            "out of range",
        );
        fails(
            r#"prompt.multi_select_builder():checked(3):items({"a"}):interact()"#,
            "out of range",
        );
        fails(r#"prompt.select_builder():interact()"#, "items must not be empty");
    }

    #[test]
    fn bools_are_not_coerced() {
        fails(r#"prompt.confirm_builder():report("false")"#, "expects a boolean");
        fails(r#"prompt.confirm_builder():default(1)"#, "expects a boolean");
        fails(r#"prompt.confirm("x", "yes")"#, "expects a boolean");
        fails(r#"prompt.password("x", 1)"#, "expects a boolean");
        fails(r#"prompt.select_builder():clear(nil)"#, "expects a boolean");
    }

    #[test]
    fn no_terminal_is_a_clear_error_unless_or_default() {
        fails(r#"prompt.confirm("go?")"#, "not a terminal");
        fails(r#"prompt.input_builder():prompt("n"):or_default(true):interact()"#, "not a terminal");
        eval(
            r#"
            assert(prompt.confirm_builder():default(true):or_default(true):interact() == true)
            assert(prompt.input_builder():default("web"):or_default(true):interact() == "web")
            local i, item = prompt.select_builder():items({"a", "b"}):default(2):or_default(true):interact()
            assert(i == 2 and item == "b")
            "#,
        )
        .unwrap();
    }

    #[test]
    fn builders_chain() {
        eval(
            r#"
            prompt.confirm_builder():prompt("go?"):default(true):wait_for_newline(true):report(false)
            prompt.input_builder():prompt("name"):default("web"):allow_empty(true)
            prompt.select_builder():items({"a", "b"}):prompt("pick"):default(2)
            prompt.multi_select_builder():items({"a", "b", "c"}):all():checked(2):defaults({1, 3})
            prompt.multi_select_builder():item("a", true):item("b")
            "#,
        )
        .unwrap();
    }
}
