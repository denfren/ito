//! `ui.progress` — progress bars and spinners, backed by `indicatif`.
//!
//! The chapter `docs/src/modules/ui-progress.md` is the surface description.
//! Implementation notes:
//!
//! - Every bar/spinner joins one shared `indicatif::MultiProgress` (created on
//!   first use) so concurrent indicators stack instead of overwriting each
//!   other. A finished bar keeps its last frame; indicatif reaps it once every
//!   bar above it has finished too.
//! - Output that shares the terminal (`print`, `io.*`, log records, prompts)
//!   goes through [`suspended`]/[`pause`] so it never garbles a live bar.
//!   `ui.progress.suspend(fn)` does *not* use indicatif's own `suspend`, which
//!   holds the multi's lock while `fn` runs (touching a bar inside deadlocks);
//!   it hides the draw target for the duration instead, and is async.
//! - [`teardown`] abandons still-running bars when the run ends or fails.
//! - A style is a set of optional overrides applied over the bar's or
//!   spinner's default template, so `:progress_chars()` alone keeps `{msg}`.
//!
//! Both draw to **stderr**, indicatif's own default. Host-only (`ito`): see
//! [`crate::scripting::ui`].

use super::super::fmt::ansi;
use indicatif::{MultiProgress, ProgressBar, ProgressDrawTarget, ProgressStyle};
use mlua::{Lua, Result as LuaResult, Table, UserData, UserDataMethods, UserDataRef, Value};
use std::cell::{Cell, RefCell};
use std::io::Write;
use std::time::Duration;

thread_local! {
    /// The `MultiProgress` every `bar()`/`spinner()` joins, created lazily.
    static MULTI: RefCell<Option<MultiProgress>> = const { RefCell::new(None) };
    /// Every bar handed out, so [`teardown`] can stop the unfinished ones.
    static LIVE: RefCell<Vec<ProgressBar>> = const { RefCell::new(Vec::new()) };
    /// Nesting depth of [`pause`].
    static DEPTH: Cell<usize> = const { Cell::new(0) };
}

fn multi() -> Option<MultiProgress> {
    MULTI.with(|cell| cell.borrow().clone())
}

/// Forget all state: a fresh engine must not inherit the last one's bars.
pub fn reset() {
    MULTI.with(|cell| *cell.borrow_mut() = None);
    LIVE.with(|cell| cell.borrow_mut().clear());
    DEPTH.with(|d| d.set(0));
}

/// Stop every unfinished bar (abandon, keeping its last frame) and drop the
/// shared state. Runs when a script ends, successfully or not.
pub fn teardown() {
    let live = LIVE.with(|cell| std::mem::take(&mut *cell.borrow_mut()));
    for pb in live {
        if !pb.is_finished() {
            pb.abandon();
        }
    }
    reset();
}

/// Join the shared `MultiProgress`, creating it if needed. Returns the bar
/// `MultiProgress::add` hands back: styling or ticking a bar *before* it is
/// added can draw it standalone first and leave a line nobody erases.
fn join_multi(pb: ProgressBar) -> ProgressBar {
    let pb = MULTI.with(|cell| {
        cell.borrow_mut()
            .get_or_insert_with(MultiProgress::new)
            .add(pb)
    });
    LIVE.with(|cell| {
        let mut live = cell.borrow_mut();
        live.retain(|p| !p.is_finished());
        live.push(pb.clone());
    });
    pb
}

/// Erase a bar's line. Only `finish_and_clear` calls this: `finish` and
/// `abandon` keep the last frame on screen, which `remove` would undo.
fn leave_multi(pb: &ProgressBar) {
    if let Some(multi) = multi() {
        multi.remove(pb);
    }
}

/// Run `f` with the bars hidden and redrawn afterwards: for plain writes to
/// the terminal (`print`, log lines). `f` must not touch a bar.
pub fn suspended<R>(f: impl FnOnce() -> R) -> R {
    match multi() {
        Some(m) => m.suspend(f),
        None => f(),
    }
}

/// `stderr` whose writes pause the bars. The log subscriber's writer.
pub struct SuspendedStderr;

impl Write for SuspendedStderr {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        suspended(|| std::io::stderr().write(buf))
    }
    fn flush(&mut self) -> std::io::Result<()> {
        std::io::stderr().flush()
    }
}

/// Holds the bars hidden while alive (nestable); restores and redraws on drop.
/// Unlike `MultiProgress::suspend` it holds no lock, so the code it guards may
/// touch bars and may await.
pub(crate) struct Pause(Option<MultiProgress>);

pub(crate) fn pause() -> Pause {
    let m = multi();
    if let Some(m) = &m
        && DEPTH.with(|d| {
            d.set(d.get() + 1);
            d.get() == 1
        })
    {
        let _ = m.clear();
        m.set_draw_target(ProgressDrawTarget::hidden());
    }
    Pause(m)
}

impl Drop for Pause {
    fn drop(&mut self) {
        if let Some(m) = &self.0
            && DEPTH.with(|d| {
                d.set(d.get().saturating_sub(1));
                d.get() == 0
            })
        {
            m.set_draw_target(ProgressDrawTarget::stderr());
            m.suspend(|| ()); // forces a redraw
        }
    }
}

fn err(what: &str, e: impl std::fmt::Display) -> mlua::Error {
    mlua::Error::runtime(format!("ui.progress.{what}: {e}"))
}

/// Overrides collected by the style builder; applied over the default
/// template of whatever the style lands on.
#[derive(Default, Clone)]
struct StyleSpec {
    template: Option<String>,
    tick: Option<Vec<String>>,
    progress_chars: Option<String>,
}

impl StyleSpec {
    fn build(&self, spinner: bool) -> ProgressStyle {
        let colour = ansi::enabled_stderr();
        let base = match (spinner, colour) {
            (false, true) => BAR_TEMPLATE,
            (false, false) => BAR_TEMPLATE_PLAIN,
            (true, true) => SPINNER_TEMPLATE,
            (true, false) => SPINNER_TEMPLATE_PLAIN,
        };
        let mut style = ProgressStyle::with_template(self.template.as_deref().unwrap_or(base))
            .expect("template validated when set");
        if let Some(t) = &self.tick {
            let refs: Vec<&str> = t.iter().map(String::as_str).collect();
            style = style.tick_strings(&refs);
        }
        style = style.progress_chars(self.progress_chars.as_deref().unwrap_or("=> "));
        style
    }
}

/// A style under construction; `Rc`-shared so methods return the same
/// handle for chaining.
#[derive(Clone)]
struct StyleBuilder(std::rc::Rc<RefCell<StyleSpec>>);

impl UserData for StyleBuilder {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("template", |_, this, template: String| {
            ProgressStyle::with_template(&template).map_err(|e| err("style", e))?;
            this.0.borrow_mut().template = Some(template);
            Ok(this.clone())
        });
        methods.add_method("tick_chars", |_, this, chars: String| {
            let v: Vec<String> = chars.chars().map(String::from).collect();
            if v.len() < 2 {
                return Err(err("style", "tick_chars needs at least 2 characters"));
            }
            this.0.borrow_mut().tick = Some(v);
            Ok(this.clone())
        });
        methods.add_method("tick_strings", |_, this, strings: Vec<String>| {
            if strings.len() < 2 {
                return Err(err("style", "tick_strings needs at least 2 strings"));
            }
            this.0.borrow_mut().tick = Some(strings);
            Ok(this.clone())
        });
        methods.add_method("progress_chars", |_, this, chars: String| {
            // indicatif panics on < 2 cells or unequal widths; check up front.
            let ok = chars.chars().count() >= 2
                && std::panic::catch_unwind(|| ProgressStyle::default_bar().progress_chars(&chars))
                    .is_ok();
            if !ok {
                return Err(err(
                    "style",
                    "progress_chars needs at least 2 characters of equal width",
                ));
            }
            this.0.borrow_mut().progress_chars = Some(chars);
            Ok(this.clone())
        });
    }
}

/// A handle around one `ProgressBar`.
struct Handle {
    pb: ProgressBar,
    spinner: bool,
}

/// Read a tick interval (delta grammar); positive and finite.
fn interval(v: &Value) -> LuaResult<Duration> {
    let secs = super::super::time::parse_delta_value(v, "enable_steady_tick")?;
    Duration::try_from_secs_f64(secs)
        .ok()
        .filter(|d| !d.is_zero())
        .ok_or_else(|| err("enable_steady_tick", format!("{secs} is not a valid interval")))
}

/// Without a terminal indicatif draws nothing; say the line on stderr instead.
fn say(pb: &ProgressBar, msg: &str) {
    if pb.is_hidden() {
        eprintln!("{msg}");
    }
}

impl UserData for Handle {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("style", |_, this, style: UserDataRef<StyleBuilder>| {
            this.pb.set_style(style.0.borrow().build(this.spinner));
            Ok(())
        });
        methods.add_method("inc", |_, this, delta: Option<u64>| {
            this.pb.inc(delta.unwrap_or(1));
            Ok(())
        });
        methods.add_method("dec", |_, this, delta: Option<u64>| {
            this.pb.dec(delta.unwrap_or(1));
            Ok(())
        });
        methods.add_method("set", |_, this, pos: u64| {
            this.pb.set_position(pos);
            Ok(())
        });
        methods.add_method("set_length", |_, this, len: u64| {
            this.pb.set_length(len);
            Ok(())
        });
        methods.add_method("inc_length", |_, this, delta: u64| {
            this.pb.inc_length(delta);
            Ok(())
        });
        methods.add_method("dec_length", |_, this, delta: u64| {
            this.pb.dec_length(delta);
            Ok(())
        });
        methods.add_method("set_message", |_, this, msg: String| {
            this.pb.set_message(msg);
            Ok(())
        });
        methods.add_method("set_prefix", |_, this, prefix: String| {
            this.pb.set_prefix(prefix);
            Ok(())
        });
        methods.add_method("tick", |_, this, ()| {
            this.pb.tick();
            Ok(())
        });
        methods.add_method("enable_steady_tick", |_, this, every: Value| {
            this.pb.enable_steady_tick(interval(&every)?);
            Ok(())
        });
        methods.add_method("disable_steady_tick", |_, this, ()| {
            this.pb.disable_steady_tick();
            Ok(())
        });
        methods.add_method("position", |_, this, ()| Ok(this.pb.position()));
        methods.add_method("length", |_, this, ()| Ok(this.pb.length()));
        methods.add_method("elapsed", |_, this, ()| {
            Ok(this.pb.elapsed().as_secs_f64())
        });
        methods.add_method("eta", |_, this, ()| Ok(this.pb.eta().as_secs_f64()));
        methods.add_method("per_sec", |_, this, ()| Ok(this.pb.per_sec()));
        methods.add_method("is_finished", |_, this, ()| Ok(this.pb.is_finished()));
        methods.add_method("println", |_, this, msg: String| {
            if this.pb.is_hidden() {
                eprintln!("{msg}");
            } else {
                this.pb.println(msg);
            }
            Ok(())
        });
        methods.add_method("reset", |_, this, ()| {
            this.pb.reset();
            Ok(())
        });
        methods.add_method("finish", |_, this, msg: Option<String>| {
            match msg {
                Some(msg) => {
                    say(&this.pb, &msg);
                    this.pb.finish_with_message(msg)
                }
                None => this.pb.finish(),
            }
            // No leave_multi(): remove() would erase the frame just drawn.
            Ok(())
        });
        methods.add_method("finish_and_clear", |_, this, ()| {
            this.pb.finish_and_clear();
            leave_multi(&this.pb);
            Ok(())
        });
        methods.add_method("abandon", |_, this, msg: Option<String>| {
            match msg {
                Some(msg) => {
                    say(&this.pb, &msg);
                    this.pb.abandon_with_message(msg)
                }
                None => this.pb.abandon(),
            }
            Ok(())
        });
    }
}

/// Bar template: message, filled/unfilled bar, count and ETA.
const BAR_TEMPLATE: &str = "{msg} [{bar:40.cyan/blue}] {pos}/{len} ({eta})";
const BAR_TEMPLATE_PLAIN: &str = "{msg} [{bar:40}] {pos}/{len} ({eta})";

/// Spinner template: the animated glyph, then the message.
const SPINNER_TEMPLATE: &str = "{spinner:.cyan} {msg}";
const SPINNER_TEMPLATE_PLAIN: &str = "{spinner} {msg}";

/// How often a bar's or spinner's own thread redraws.
const TICK: Duration = Duration::from_millis(100);

/// Build the `ui.progress` table.
pub fn module(lua: &Lua) -> LuaResult<Table> {
    let module = lua.create_table()?;

    // ui.progress.bar(len[, msg]) -> handle
    module.set(
        "bar",
        lua.create_function(|_, (len, msg): (u64, Option<String>)| {
            let pb = join_multi(ProgressBar::new(len));
            pb.set_style(StyleSpec::default().build(false));
            pb.set_message(msg.unwrap_or_default());
            pb.enable_steady_tick(TICK);
            Ok(Handle { pb, spinner: false })
        })?,
    )?;

    // ui.progress.spinner([msg]) -> handle
    module.set(
        "spinner",
        lua.create_function(|_, msg: Option<String>| {
            let pb = join_multi(ProgressBar::new_spinner());
            pb.set_style(StyleSpec::default().build(true));
            pb.set_message(msg.unwrap_or_default());
            pb.enable_steady_tick(TICK);
            Ok(Handle { pb, spinner: true })
        })?,
    )?;

    // ui.progress.style() -> empty style builder (overrides only)
    module.set(
        "style",
        lua.create_function(|_, ()| {
            Ok(StyleBuilder(std::rc::Rc::new(RefCell::new(
                StyleSpec::default(),
            ))))
        })?,
    )?;

    // ui.progress.clear() -- erase every bar's line from the display.
    module.set(
        "clear",
        lua.create_function(|_, ()| {
            if let Some(m) = multi() {
                let _ = m.clear();
            }
            Ok(())
        })?,
    )?;

    // ui.progress.suspend(fn) -- hide the bars, await fn, redraw. The bars stay
    // usable inside `fn` (the draw target is hidden, nothing is locked).
    module.set(
        "suspend",
        lua.create_async_function(async |_, f: mlua::Function| {
            let _pause = pause();
            f.call_async::<()>(()).await
        })?,
    )?;

    Ok(module)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lua_with_progress() -> Lua {
        let lua = Lua::new();
        let table = module(&lua).unwrap();
        lua.globals().set("progress", table).unwrap();
        lua
    }

    #[test]
    fn bar_tracks_position_and_length() {
        let lua = lua_with_progress();
        lua.load(
            r#"
            local b = progress.bar(10, "work")
            b:inc(3)
            b:set(7)
            assert(b:position() == 7, b:position())
            assert(b:length() == 10, b:length())
            b:set_length(20)
            assert(b:length() == 20)
            b:inc_length(5)
            assert(b:length() == 25)
            b:dec_length(5)
            assert(b:length() == 20)
            b:dec(2)
            assert(b:position() == 5, b:position())
            b:finish("done")
            assert(b:is_finished())
            "#,
        )
        .exec()
        .unwrap();
    }

    #[test]
    fn spinner_ticks_and_clears() {
        let lua = lua_with_progress();
        lua.load(
            r#"
            local s = progress.spinner("waiting")
            s:set_message("still waiting")
            s:set_prefix(">>")
            s:tick()
            s:disable_steady_tick()
            s:println("a line above the spinner")
            s:finish_and_clear()
            "#,
        )
        .exec()
        .unwrap();
    }

    #[test]
    fn abandon_and_reset() {
        let lua = lua_with_progress();
        lua.load(
            r#"
            local b = progress.bar(10)
            b:set(4)
            b:abandon("gave up")
            assert(b:is_finished())
            "#,
        )
        .exec()
        .unwrap();
    }

    #[test]
    fn style_builder_applies_to_a_bar() {
        let lua = lua_with_progress();
        lua.load(
            r##"
            local style = progress.style()
                :template("{msg} [{wide_bar}] {percent}%")
                :progress_chars("#>-")
            local b = progress.bar(10, "custom")
            b:style(style)
            b:set(3)
            "##,
        )
        .exec()
        .unwrap();
    }

    #[test]
    fn concurrent_bars_join_and_clear() {
        let lua = lua_with_progress();
        lua.load(
            r#"
            local a = progress.bar(10, "a")
            local b = progress.spinner("b")
            a:inc(1)
            b:tick()
            a:finish_and_clear()
            b:finish_and_clear()
            progress.clear()
            local c = progress.bar(5, "c")
            c:finish_and_clear()
            "#,
        )
        .exec()
        .unwrap();
    }

    async fn run(lua: &Lua, code: &str) -> mlua::Result<()> {
        lua.load(code).exec_async().await
    }

    fn rt() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    #[test]
    fn suspend_may_touch_bars_and_nest() {
        let lua = lua_with_progress();
        rt().block_on(run(
            &lua,
            r#"
            local b = progress.bar(10, "x")
            local ran = 0
            progress.suspend(function()
                b:inc(2)
                b:set_message("inside")
                progress.suspend(function() ran = ran + 1 end)
                ran = ran + 1
            end)
            assert(ran == 2 and b:position() == 2)
            b:finish_and_clear()
            progress.suspend(function() ran = 3 end)
            assert(ran == 3)
            "#,
        ))
        .unwrap();
        teardown();
    }

    #[test]
    fn bad_styles_and_intervals_raise_instead_of_panicking() {
        let lua = lua_with_progress();
        for bad in [
            "progress.style():tick_strings({'x'})",
            "progress.style():tick_chars('x')",
            "progress.style():progress_chars('x')",
            "progress.bar(1):enable_steady_tick(-1)",
            "progress.bar(1):enable_steady_tick(0)",
            "progress.bar(1):enable_steady_tick(1e300)",
        ] {
            assert!(lua.load(bad).exec().is_err(), "{bad}");
        }
        lua.load("progress.bar(1):enable_steady_tick('250ms')")
            .exec()
            .unwrap();
        teardown();
    }

    #[test]
    fn inc_defaults_to_one_and_teardown_stops_bars() {
        let lua = lua_with_progress();
        lua.load(
            r##"
            b = progress.bar(5)
            b:inc() b:inc()
            assert(b:position() == 2)
            b:dec()
            assert(b:position() == 1)
            progress.style():progress_chars("#>-")
            "##,
        )
        .exec()
        .unwrap();
        let b: UserDataRef<Handle> = lua.globals().get("b").unwrap();
        assert!(!b.pb.is_finished());
        teardown();
        assert!(b.pb.is_finished());
    }

    #[test]
    fn style_builder_tick_strings() {
        let lua = lua_with_progress();
        lua.load(
            r#"
            local style = progress.style():tick_strings({ "a", "b", "c", "" })
            local s = progress.spinner("x")
            s:style(style)
            "#,
        )
        .exec()
        .unwrap();
    }
}
