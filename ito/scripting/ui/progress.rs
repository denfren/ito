//! `ui.progress` — progress bars and spinners, backed by `indicatif`.
//!
//! Two layers, like `hcl.builder()`/`hcl.block()` beside `hcl.parse`: a
//! **shorthand** constructor for the common case, and a **style builder**
//! for everything `indicatif::ProgressStyle` exposes.
//!
//! ```lua
//! -- shorthand: sensible default template
//! local bar = ui.progress.bar(#hosts, "checking hosts")
//! for _, h in ipairs(hosts) do
//!     probe(h)
//!     bar:inc(1)
//! end
//! bar:finish("done")
//!
//! -- style builder: any indicatif template, tick set or fill characters
//! local style = ui.progress.style()
//!     :template("{spinner} {msg} [{wide_bar}] {percent}%")
//!     :progress_chars("#>-")
//!     :tick_strings({ "◐", "◓", "◑", "◒", "" })
//! local bar = ui.progress.bar(100):style(style)
//! ```
//!
//! | Call | Effect |
//! | --- | --- |
//! | `ui.progress.bar(len[, msg])` | a bar handle with a default template |
//! | `ui.progress.spinner([msg])` | a spinner handle, self-animating |
//! | `ui.progress.style()` | a style builder, `indicatif`'s template language |
//! | `ui.progress.clear()` | drop the shared multi-bar state, for a fresh start |
//! | `ui.progress.suspend(fn)` | hide every bar, run `fn`, redraw -- for output that must not interleave |
//!
//! Every bar/spinner transparently joins one shared `indicatif::MultiProgress`
//! -- created on first use, so several concurrent indicators (e.g. one per
//! `task.pool` worker) stack as separate terminal lines instead of
//! overwriting each other. A single bar draws exactly as it did before this
//! existed; the difference only shows once a second one is live at the same
//! time. A finisher (`finish`/`finish_and_clear`/`abandon`) leaves the shared
//! state; `ui.progress.clear()` drops it outright for a script that wants a
//! clean slate between unrelated phases rather than stacking onto whatever's
//! left from the last one.
//!
//! `ui.progress.bar(len[, msg])`/`spinner([msg])` return a **handle**
//! (userdata), not a one-shot call, because a progress indicator is
//! inherently a thing updated over time — across loop iterations, or from
//! several `task`s sharing one bar. `indicatif`'s `ProgressBar` is already
//! `Clone` and internally synchronized (an `Arc` over a mutexed state plus
//! atomics), so the handle can be cloned into closures spawned onto other
//! tasks with no locking of its own to add.
//!
//! | Method | Effect |
//! | --- | --- |
//! | `h:style(style)` | apply a [`style`](#uiprogressstyle) builder |
//! | `h:inc(n)` | advance the position by `n` |
//! | `h:dec(n)` | move the position back by `n` |
//! | `h:set(n)` | set the position to `n` |
//! | `h:set_length(n)` | change the total (bar only) |
//! | `h:inc_length(n)` / `h:dec_length(n)` | grow/shrink the total in place |
//! | `h:set_message(s)` | replace the trailing status text |
//! | `h:set_prefix(s)` | replace the leading `{prefix}` text |
//! | `h:tick()` | advance a spinner's animation frame by hand |
//! | `h:enable_steady_tick(d)` | animate on `indicatif`'s own thread every `d` |
//! | `h:disable_steady_tick()` | stop that thread |
//! | `h:position()` / `h:length()` | current position / total (`length` may be `nil`) |
//! | `h:elapsed()` / `h:eta()` / `h:per_sec()` | timing, seconds / seconds / rate |
//! | `h:is_finished()` | whether a terminal state was reached |
//! | `h:println(s)` | print a line above the bar without disturbing it |
//! | `h:reset()` | clear position, elapsed time and ETA |
//! | `h:finish([msg])` | stop, keep the bar on screen, optionally set `msg` first |
//! | `h:finish_and_clear()` | stop and erase the line |
//! | `h:abandon([msg])` | stop, keep on screen, marked incomplete rather than done |
//!
//! **A spinner's animation runs on `indicatif`'s own background thread**
//! (`enable_steady_tick`), not on ito's single-threaded Luau runtime — a
//! `task.wait` loop advancing it by hand would stop animating the moment the
//! script itself awaited anything else. That thread draws only; every method
//! above still just mutates shared, synchronized state, so nothing here blocks
//! the calling task.
//!
//! Both draw to **stderr** by default -- indicatif's own default target --
//! which is what keeps a bar from interleaving with a script's own `print`
//! (stdout) or a piped `iq`-style return value; this module doesn't change it.
//!
//! # `ui.progress.style()`
//!
//! A builder over `indicatif::ProgressStyle`, `with_*`-chained like every
//! other ito builder (`hcl.builder()`, `proc.command()`); a terminal
//! `h:style(...)` on a bar/spinner applies it (there is nothing else to
//! finish it into — a style has no meaning on its own).
//!
//! | Method | Effect |
//! | --- | --- |
//! | `s:template(str)` | the layout, `indicatif`'s own placeholder syntax (`{bar}`, `{msg}`, `{pos}`, `{len}`, `{percent}`, `{eta}`, `{elapsed}`, `{spinner}`, `{prefix}`, `{wide_bar}`, …) |
//! | `s:tick_chars(str)` | one string, one character per spinner frame plus a final frame |
//! | `s:tick_strings(list)` | the same, as separate strings (multi-byte glyphs) |
//! | `s:progress_chars(str)` | three characters: filled, current, unfilled |
//!
//! A **malformed** template (a stray `{`/`}`, an unterminated brace) raises,
//! naming the syntax problem; an **unrecognized placeholder** does not
//! error — `indicatif` renders it empty rather than checking the key set
//! against the ones it knows.
//!
//! # What's not exposed, and why
//!
//! **`ProgressTracker` / `with_key`** — a custom computed template field —
//! takes a Rust trait object invoked on every redraw, on whichever thread is
//! drawing (this module's own steady-tick thread, or the caller for a
//! manually-ticked bar). Calling back into a `!Send` `mlua::Lua` from there
//! has the same shape as `ui.prompt`'s live-validator gap, and isn't
//! something mlua supports; a script computing its own text updates it with
//! `h:set_message(...)`/`h:set_prefix(...)` instead of a template key.
//!
//! Host-only (`ito`/`irun`): see [`crate::scripting::ui`] for why the whole
//! `ui` umbrella lives only there.

use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use mlua::{Lua, Result as LuaResult, Table, UserData, UserDataMethods, UserDataRef};
use std::cell::RefCell;
use std::time::Duration;

thread_local! {
    /// The `MultiProgress` every `bar()`/`spinner()` joins, created lazily on
    /// first use. Single bar or spinner draws identically whether or not it
    /// has a `MultiProgress` around it, so joining one unconditionally costs
    /// nothing in the common single-indicator case and makes concurrent bars
    /// (e.g. one per `task.pool` worker) stack instead of clobbering each
    /// other's terminal line -- no `ui.progress.multi()` for a script to
    /// remember to reach for. `ui.progress.clear()` drops it for a script
    /// that wants a clean slate between unrelated phases.
    static MULTI: RefCell<Option<MultiProgress>> = const { RefCell::new(None) };
}

/// Join the shared `MultiProgress`, creating it if this is the first bar or
/// spinner since the last `clear()`. Returns the bar `MultiProgress::add`
/// hands back rather than the one passed in: indicatif's own docs warn that
/// styling/ticking/setting a message on a bar *before* it's added can draw it
/// standalone first, leaving a line the `MultiProgress` never learns to erase
/// -- the "zombie line" duplicates a script would otherwise see while several
/// bars are starting up together. So callers must add a bare `ProgressBar`
/// to the multi *first* and do everything else to the bar this returns.
fn join_multi(pb: ProgressBar) -> ProgressBar {
    MULTI.with(|cell| {
        let mut multi = cell.borrow_mut();
        let multi = multi.get_or_insert_with(MultiProgress::new);
        multi.add(pb)
    })
}

/// Drop a bar/spinner from the shared `MultiProgress`'s bookkeeping, erasing
/// its line on the next redraw. Only `finish_and_clear` calls this: `finish`
/// and `abandon` mean "keep the last frame on screen", which `remove` would
/// immediately undo. `MultiProgress`'s own zombie handling already keeps a
/// finished-but-not-removed member's last frame visible without redrawing
/// it, so those two need no bookkeeping call at all.
fn leave_multi(pb: &ProgressBar) {
    MULTI.with(|cell| {
        if let Some(multi) = cell.borrow().as_ref() {
            multi.remove(pb);
        }
    });
}

/// Map an `indicatif` template/parse error onto a Luau error naming what was
/// rejected, the same shape a bad strftime pattern gets from `time`.
fn style_err(e: impl std::fmt::Display) -> mlua::Error {
    mlua::Error::runtime(format!("ui.progress.style: {e}"))
}

/// A style under construction. Thin enough that each method just rebuilds
/// the wrapped `ProgressStyle` and reassigns it -- `ProgressStyle`'s own
/// builder is consuming (`self -> Self`), so there is nothing to mutate in
/// place without one. `Rc`-shared, like `HclBody`/`HclBlock`, so a method
/// can return a *clone* of the handle (same underlying cell) to support
/// `:template(...):progress_chars(...)` chaining the way every other ito
/// builder does.
#[derive(Clone)]
struct StyleBuilder(std::rc::Rc<std::cell::RefCell<ProgressStyle>>);

impl UserData for StyleBuilder {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method(
            "template",
            |_, this, template: String| -> LuaResult<StyleBuilder> {
                let next = this
                    .0
                    .borrow()
                    .clone()
                    .template(&template)
                    .map_err(style_err)?;
                *this.0.borrow_mut() = next;
                Ok(this.clone())
            },
        );
        methods.add_method(
            "tick_chars",
            |_, this, chars: String| -> LuaResult<StyleBuilder> {
                let next = this.0.borrow().clone().tick_chars(&chars);
                *this.0.borrow_mut() = next;
                Ok(this.clone())
            },
        );
        methods.add_method(
            "tick_strings",
            |_, this, strings: Vec<String>| -> LuaResult<StyleBuilder> {
                let refs: Vec<&str> = strings.iter().map(String::as_str).collect();
                let next = this.0.borrow().clone().tick_strings(&refs);
                *this.0.borrow_mut() = next;
                Ok(this.clone())
            },
        );
        methods.add_method(
            "progress_chars",
            |_, this, chars: String| -> LuaResult<StyleBuilder> {
                let next = this.0.borrow().clone().progress_chars(&chars);
                *this.0.borrow_mut() = next;
                Ok(this.clone())
            },
        );
    }
}

/// A handle around one `ProgressBar`. Cheap to clone (an `Arc` underneath),
/// but the Luau side only ever sees this wrapper as userdata.
struct Handle(ProgressBar);

impl UserData for Handle {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("style", |_, this, style: UserDataRef<StyleBuilder>| {
            this.0.set_style(style.0.borrow().clone());
            Ok(())
        });
        methods.add_method("inc", |_, this, delta: u64| {
            this.0.inc(delta);
            Ok(())
        });
        methods.add_method("dec", |_, this, delta: u64| {
            this.0.dec(delta);
            Ok(())
        });
        methods.add_method("set", |_, this, pos: u64| {
            this.0.set_position(pos);
            Ok(())
        });
        methods.add_method("set_length", |_, this, len: u64| {
            this.0.set_length(len);
            Ok(())
        });
        methods.add_method("inc_length", |_, this, delta: u64| {
            this.0.inc_length(delta);
            Ok(())
        });
        methods.add_method("dec_length", |_, this, delta: u64| {
            this.0.dec_length(delta);
            Ok(())
        });
        methods.add_method("set_message", |_, this, msg: String| {
            this.0.set_message(msg);
            Ok(())
        });
        methods.add_method("set_prefix", |_, this, prefix: String| {
            this.0.set_prefix(prefix);
            Ok(())
        });
        methods.add_method("tick", |_, this, ()| {
            this.0.tick();
            Ok(())
        });
        methods.add_method("enable_steady_tick", |_, this, secs: f64| {
            this.0.enable_steady_tick(Duration::from_secs_f64(secs));
            Ok(())
        });
        methods.add_method("disable_steady_tick", |_, this, ()| {
            this.0.disable_steady_tick();
            Ok(())
        });
        methods.add_method("position", |_, this, ()| Ok(this.0.position()));
        methods.add_method("length", |_, this, ()| Ok(this.0.length()));
        methods.add_method("elapsed", |_, this, ()| Ok(this.0.elapsed().as_secs_f64()));
        methods.add_method("eta", |_, this, ()| Ok(this.0.eta().as_secs_f64()));
        methods.add_method("per_sec", |_, this, ()| Ok(this.0.per_sec()));
        methods.add_method("is_finished", |_, this, ()| Ok(this.0.is_finished()));
        methods.add_method("println", |_, this, msg: String| {
            this.0.println(msg);
            Ok(())
        });
        methods.add_method("reset", |_, this, ()| {
            this.0.reset();
            Ok(())
        });
        methods.add_method("finish", |_, this, msg: Option<String>| {
            match msg {
                Some(msg) => this.0.finish_with_message(msg),
                None => this.0.finish(),
            }
            // Deliberately no leave_multi(): remove() erases the bar's line
            // on the next redraw, which would undo the very message finish()
            // just drew. Left registered, indicatif's own zombie handling
            // keeps a finished member's last frame on screen without
            // redrawing it further -- exactly "stop, keep on screen".
            Ok(())
        });
        methods.add_method("finish_and_clear", |_, this, ()| {
            this.0.finish_and_clear();
            leave_multi(&this.0);
            Ok(())
        });
        methods.add_method("abandon", |_, this, msg: Option<String>| {
            match msg {
                Some(msg) => this.0.abandon_with_message(msg),
                None => this.0.abandon(),
            }
            // Same reasoning as finish(): kept on screen, so not removed.
            Ok(())
        });
    }
}

/// Bar template: position, a filled/unfilled bar, count and ETA. `msg`
/// occupies its own segment so `set_message` has somewhere to land without a
/// second template.
const BAR_TEMPLATE: &str = "{msg} [{bar:40.cyan/blue}] {pos}/{len} ({eta})";

/// Spinner template: the animated glyph, then the message.
const SPINNER_TEMPLATE: &str = "{spinner:.cyan} {msg}";

/// How often the spinner's own thread redraws. Fast enough to look alive,
/// far under anything a human perceives as flicker.
const SPINNER_TICK: Duration = Duration::from_millis(100);

/// Build the `ui.progress` table.
pub fn module(lua: &Lua) -> LuaResult<Table> {
    let module = lua.create_table()?;

    // ui.progress.bar(len[, msg]) -> handle
    module.set(
        "bar",
        lua.create_function(|_, (len, msg): (u64, Option<String>)| {
            let pb = join_multi(ProgressBar::new(len));
            pb.set_style(
                ProgressStyle::with_template(BAR_TEMPLATE)
                    .expect("BAR_TEMPLATE is a valid indicatif template")
                    .progress_chars("=> "),
            );
            pb.set_message(msg.unwrap_or_default());
            Ok(Handle(pb))
        })?,
    )?;

    // ui.progress.spinner([msg]) -> handle
    //
    // Ticks itself on indicatif's own background thread (`enable_steady_tick`)
    // rather than through `task`, so it keeps animating across an `await` the
    // script makes elsewhere -- see the module docs.
    module.set(
        "spinner",
        lua.create_function(|_, msg: Option<String>| {
            let pb = join_multi(ProgressBar::new_spinner());
            pb.set_style(
                ProgressStyle::with_template(SPINNER_TEMPLATE)
                    .expect("SPINNER_TEMPLATE is a valid indicatif template"),
            );
            pb.set_message(msg.unwrap_or_default());
            pb.enable_steady_tick(SPINNER_TICK);
            Ok(Handle(pb))
        })?,
    )?;

    // ui.progress.style() -> style builder, starting from indicatif's own
    // generic default (a plain "{wide_bar} {pos}/{len}" shape) -- a script
    // that only wants to change the fill characters need not restate a
    // template first.
    module.set(
        "style",
        lua.create_function(|_, ()| {
            Ok(StyleBuilder(std::rc::Rc::new(std::cell::RefCell::new(
                ProgressStyle::default_bar(),
            ))))
        })?,
    )?;

    // ui.progress.clear() -- drop the shared MultiProgress, so the next
    // bar()/spinner() starts a fresh one. For a script that wants a clean
    // slate between unrelated phases rather than stacking onto whatever's
    // left from the last one.
    module.set(
        "clear",
        lua.create_function(|_, ()| {
            MULTI.with(|cell| *cell.borrow_mut() = None);
            Ok(())
        })?,
    )?;

    // ui.progress.suspend(fn) -- indicatif's own atomic hide/run/redraw:
    // clears every live bar's lines, runs the Luau function, then redraws
    // them, safe against a spinner's own steady-tick thread redrawing
    // mid-way (a bare pause()/resume() pair couldn't promise that). If no
    // bar/spinner has been created yet there's nothing to hide, so the
    // function just runs directly. The multi is cloned out and the
    // thread-local borrow dropped *before* calling into Luau, since the
    // function may itself create a new bar (which needs its own borrow).
    module.set(
        "suspend",
        lua.create_function(|_, f: mlua::Function| {
            let multi = MULTI.with(|cell| cell.borrow().clone());
            match multi {
                Some(multi) => multi.suspend(|| f.call::<()>(())),
                None => f.call::<()>(()),
            }
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

    #[test]
    fn suspend_runs_the_function_with_and_without_a_bar() {
        let lua = lua_with_progress();
        lua.load(
            r#"
            local ran = false
            progress.suspend(function() ran = true end)
            assert(ran)

            progress.clear()
            local b = progress.bar(10, "x")
            ran = false
            progress.suspend(function() ran = true end)
            assert(ran)
            b:finish_and_clear()
            "#,
        )
        .exec()
        .unwrap();
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
