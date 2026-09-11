//! Luau bindings for process invocation, modeled on `tokio::process::Command`.
//!
//! Registered as the global `proc`. Redirect targets are installed onto the same
//! table (see `scripting::redirect`) and the `ssh` module takes the same values:
//! passing the *same* handle to both `:stdout()` and `:stderr()` merges the two
//! streams onto one file descriptor -- the equivalent of the shell's `2>&1`.
//!
//! Both streams default to NULL: nothing reaches the terminal unless asked for.
//!
//! `command` takes either a program name, with arguments added via `:arg()` /
//! `:args()`, or a list holding the program and its arguments together:
//!
//! ```lua
//! proc.command("ls"):arg("-la")
//! proc.command({ "ls", "-la" })
//! ```
//!
//! As in `std::process`, running splits two ways: `:run()` starts the child and
//! waits for it, while `:spawn()` returns a child to be awaited later. Both are
//! async all the way down, so several children under `task.spawn` genuinely
//! overlap rather than taking turns.
//!
//! ```lua
//! local result = proc.command({ "ls" }):run()
//!
//! local child = proc.command({ "sleep", "5" }):spawn()
//! local result = child:wait()
//! ```
//!
//! `:stdin(source)` takes `proc.NULL` (the default), `proc.INHERIT`, a
//! `proc.file(path)` (read, not created), or a Luau string or buffer that is
//! written to the child and then closed.
//!
//! `:exec()` is a third finalizer, and a different kind: it `execve`s this
//! process into the child, exactly as the shell's `exec` builtin does. There
//! is no fork, so it never returns to Luau on success — the calling
//! process's image, pid and open fds all become the new program's, with
//! stdin/stdout/stderr inherited unchanged. Because there is no child left
//! afterward, `:stdin()`/`:stdout()`/`:stderr()` must be left at their default
//! or set to `proc.INHERIT`; anything else has no process to apply to and
//! `:exec()` rejects it. A failed exec leaves this process as it was (working
//! directory included), and the program is looked up on the *child's* `PATH`.
//!
//! ```lua
//! proc.command({ "the-real-program", "--flag" }):exec()
//! -- unreached on success
//! ```
//!
//! `:interactive()` is the fourth: it spawns a real child (unlike `:exec()`,
//! there is a fork) but shares this process's stdin/stdout/stderr with it
//! outright and waits for it, for a child that needs the actual terminal --
//! a pager, an editor, anything prompting for input. `:run()`/`:spawn()`
//! default stdin and both output streams to NULL, so neither gives a child a
//! terminal to interact with; `:interactive()` defaults all three to INHERIT
//! without giving up the process the way `:exec()` does. Unlike `:exec()`,
//! any stream may still be redirected explicitly: `:stdout(proc.string())`
//! captures what an `fzf`-style program prints while its UI stays on the
//! terminal. ito ignores SIGINT/SIGQUIT while the child runs (the child gets
//! the default disposition back), so ^C reaches the child, not us.
//!
//! ```lua
//! local result = proc.command("vim"):arg("notes.txt"):interactive()
//! ```
//!
//! `:wait()` and `:kill()` both consume the child and return its result, so a
//! killed child is always reaped -- there is no way to leave a zombie behind by
//! forgetting to wait. `:kill()` sends SIGKILL, as `Child::kill()` does.
//!
//! `:signal(sig)` is the *other* half of that: it sends any signal and, unlike
//! `:kill()`, does **not** consume the child, because a signal is a request
//! rather than a verdict. SIGTERM asks a process to shut down and it may take
//! its time, ignore it, or exit nonzero on purpose; SIGHUP and SIGUSR1 need not
//! end it at all. Consuming the child there would force the caller to treat
//! "asked to stop" as "stopped":
//!
//! ```lua
//! local child = proc.command({ "sleep", "30" }):spawn()
//! child:signal("TERM")            -- or proc.SIGTERM, or 15
//! local result = child:wait()     -- still yours to wait for
//! print(result.signal_name)       --> "SIGTERM"
//! ```
//!
//! A signal-killed child reports `signal`/`signal_name` in its result table
//! alongside the `status = nil` that says there is no exit code; a child that
//! exited normally has both fields absent, so a bare `if result.signal then`
//! distinguishes the two.
//!
//! `:try_wait()` is the non-blocking `:wait()`: `nil` while the child is still
//! running, the result table (consuming the child) once it has exited. It makes
//! the grace-period pattern (`TERM`, poll, then `KILL`) stop waiting as soon as
//! the child is gone.
//!
//! An abandoned `:run()`/`:interactive()` (a `task.timeout` expiring mid-command,
//! say) kills its child when the run is torn down: those children are spawned
//! with `kill_on_drop`. A `:spawn()`ed child is *not*: dropping its handle leaves
//! it running (a killed-but-unwaited one is still reaped by tokio's SIGCHLD
//! handler, so there is no zombie). Bound a spawned command with a watchdog that
//! `:signal()`s or `:kill()`s the handle, never a pid (a pid can be reused).
//!
//! An **abandoned `:wait()`** hands the child back rather than swallowing it:
//! the borrow is scoped and a `WaitGuard` restores the child and its readers
//! when the future is dropped, so a later `:kill()` still works.
//!
//! The one gap is `task.timeout`, which cancels by `abort()`ing a `spawn_local`
//! driver — tokio drops that future only when it next polls it, which never
//! happens for an aborted task, so the guard runs at teardown instead. A child
//! abandoned *that* way stays taken, and every method on it (`:kill()`,
//! `:signal()`, `:id()` alike) reports "already been waited for" until the run
//! ends. To bound a command's runtime, don't time out its `:wait()` — spawn it,
//! poll or sleep, and `:signal()`/`:kill()` the handle you still hold.
//!
//! # `proc.trap` — signals sent *to this process*
//!
//! Everything above is about signalling a *child*; `proc.trap` is the
//! opposite direction, for reacting to a signal this process itself receives:
//!
//! ```lua
//! proc.trap(proc.SIGTERM, function(signal)
//!     log.info("caught signal", signal, "-- shutting down")
//!     proc.exit(0)
//! end)
//! ```
//!
//! A signal nothing has trapped keeps its default disposition entirely —
//! there is no built-in reaction to any signal, SIGINT included, so a run
//! that never calls `proc.trap` behaves exactly as if this did not exist. The
//! first `proc.trap(sig, f)` call for a given signal is what takes it off
//! that default; a later call for the same signal replaces `f` rather than
//! adding a second handler, and `proc.untrap(sig)` removes it, returning the
//! signal to being unhandled by this table (its process-level disposition,
//! having already been changed once, stays "caught, does nothing" rather than
//! reverting to the kernel default — a script that needs the literal default
//! back should have its handler act like it, e.g. `proc.exit`).
//!
//! `SIGKILL` and `SIGSTOP` cannot be trapped — the kernel never offers them to
//! a handler — and neither can `SIGILL`/`SIGFPE`/`SIGSEGV`/`SIGBUS` or a
//! number that is no signal: `proc.trap` raises for each, at the call, rather
//! than registering something that would silently never fire.
//!
//! The handler runs detached from the script's own control flow: there is no
//! `task` handle to `await`, so an error it raises is logged and the trap
//! keeps working for the next delivery rather than failing the run. A handler
//! that means to end the process calls `proc.exit` itself.
//!
//! `proc.exit(code)` is an immediate `std::process::exit` — no Luau unwind, no
//! task drain, no cleanup of any kind — chosen deliberately so a trap handler
//! deciding "stop now" is not itself blocked by whatever else might be stuck.

use crate::scripting::redirect::{
    self, CaptureKind, InheritTarget, Reader, Redirect, Slot, Stdin, bind_alias, capture_to_value,
    conflicting_files, join_reader, merged_capture, read_to_end, redirect_from_value,
    stdin_from_value,
};
use mlua::{AnyUserData, Lua, Result as LuaResult, Table, UserData, UserDataMethods, Value};
use std::ffi::{OsStr, OsString};
use std::os::unix::process::ExitStatusExt;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use tokio::process::Command;

// ---------------------------------------------------------------------------
// Signals
// ---------------------------------------------------------------------------

/// The signals worth naming, as `(name-without-SIG, number)`.
///
/// Deliberately not the whole of `signal(7)`: these are the ones a script
/// managing a child actually sends or sees. Anything omitted is still reachable
/// as a number, so the list bounds the *names*, not the capability.
///
/// The numbers come from `libc` rather than being written out, because the job
/// and user-defined signals genuinely differ between Linux and the BSDs
/// (`SIGUSR1` is 10 on Linux and 30 on macOS, `SIGSTOP` 19 against 17) — a
/// hardcoded table would send the wrong signal on one of the two platforms ito
/// runs on, silently.
///
/// The `SIG` prefix is added back by `install_signal_constants` and accepted but
/// not required by `signal_number`, so `"TERM"`, `"SIGTERM"` and `15` are three
/// spellings of one thing.
fn named_signals() -> impl Iterator<Item = (&'static str, i32)> {
    [
        ("HUP", libc::SIGHUP),
        ("INT", libc::SIGINT),
        ("QUIT", libc::SIGQUIT),
        ("KILL", libc::SIGKILL),
        ("USR1", libc::SIGUSR1),
        ("USR2", libc::SIGUSR2),
        ("PIPE", libc::SIGPIPE),
        ("ALRM", libc::SIGALRM),
        ("TERM", libc::SIGTERM),
        ("CONT", libc::SIGCONT),
        ("STOP", libc::SIGSTOP),
        ("TSTP", libc::SIGTSTP),
        ("CHLD", libc::SIGCHLD),
        ("TTIN", libc::SIGTTIN),
        ("TTOU", libc::SIGTTOU),
        ("WINCH", libc::SIGWINCH),
    ]
    .into_iter()
}

/// Resolve a `:signal()` argument to a number.
///
/// Accepts a name with or without the `SIG` prefix, in any case, or a raw
/// number for a signal this does not name. An unknown *name* is rejected rather
/// than guessed at: a typo that silently sent nothing would look like a process
/// ignoring the signal.
fn signal_number(value: &Value) -> LuaResult<i32> {
    match value {
        Value::Integer(n) => signal_range(i64::from(*n)),
        // A Luau number is a float; accept one only if it names an integer, so
        // `:signal(15.5)` is a mistake rather than a truncation.
        Value::Number(n) if n.fract() == 0.0 => signal_range(*n as i64),
        Value::String(s) => {
            let raw = s.to_str()?.to_string();
            let bare = raw.to_ascii_uppercase();
            let bare = bare.strip_prefix("SIG").unwrap_or(&bare);
            named_signals()
                .find(|(name, _)| *name == bare)
                .map(|(_, n)| n)
                .ok_or_else(|| {
                    let mut names: Vec<String> =
                        named_signals().map(|(n, _)| format!("SIG{n}")).collect();
                    names.sort();
                    mlua::Error::runtime(format!(
                        "unknown signal '{raw}'; expected one of {}, or a number",
                        names.join(", ")
                    ))
                })
        }
        other => Err(mlua::Error::runtime(format!(
            "expected a signal name or number, got {}",
            other.type_name()
        ))),
    }
}

/// A raw signal number must fit an `i32`, not wrap into some other signal.
fn signal_range(n: i64) -> LuaResult<i32> {
    i32::try_from(n).map_err(|_| mlua::Error::runtime(format!("signal number {n} is out of range")))
}

/// The `SIG`-prefixed name of a signal number, for the result table.
///
/// `None` for a number with no name here, in which case the result carries the
/// number alone rather than an invented `"SIG37"`.
fn signal_name(number: i32) -> Option<String> {
    named_signals()
        .find(|(_, n)| *n == number)
        .map(|(name, _)| format!("SIG{name}"))
}

/// Record how a child died, on the result table both `proc` and `ssh` build.
///
/// Present only for a signal death, and *both* fields together: `status = nil`
/// used to be the sole indication, which said an exit code was missing without
/// saying why. `if result.signal then` is now the test, and `signal_name` is
/// absent alongside `signal` for a number this does not name rather than being
/// an invented `"SIG37"`.
pub(crate) fn set_signal_fields(result: &Table, signal: Option<i32>) -> LuaResult<()> {
    let Some(number) = signal else {
        return Ok(());
    };
    result.raw_set("signal", number)?;
    if let Some(name) = signal_name(number) {
        result.raw_set("signal_name", name)?;
    }
    Ok(())
}

/// Add `proc.SIGTERM`, `proc.SIGKILL`, ... to the module table.
///
/// Constants as well as strings because a name typed as a string is checked at
/// the call, while `proc.SIGTERM` is checked when the table is indexed — under
/// `sandbox(true)` a misspelled `proc.SIGTREM` is `nil`, and `:signal(nil)`
/// raises immediately rather than being read as some other signal.
fn install_signal_constants(exports: &Table) -> LuaResult<()> {
    for (name, number) in named_signals() {
        exports.raw_set(format!("SIG{name}"), number)?;
    }
    Ok(())
}

/// Send `signal` to `pid`.
///
/// tokio's `Child` offers only `start_kill` (SIGKILL), so anything else is
/// `kill(2)` directly. The pid comes from the live `Child`, which is only
/// `Some` before the child has been reaped, so this cannot signal a recycled pid
/// belonging to some unrelated process.
fn send_signal(pid: u32, signal: i32) -> std::io::Result<()> {
    // SAFETY: `kill` is async-signal-safe and takes no pointers; the pid is one
    // this process spawned and has not yet reaped, so it names our own child.
    let rc = unsafe { libc::kill(pid as libc::pid_t, signal) };
    if rc == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

// ---------------------------------------------------------------------------
// proc.trap
// ---------------------------------------------------------------------------

/// Signals a script may legitimately want to trap.
///
/// `SIGKILL`/`SIGSTOP` are excluded outright -- the kernel never delivers
/// either to a handler (`sigaction` on them is a mistake to make, not a
/// choice to honour), so `proc.trap` rejects them before ever calling
/// `tokio::signal::unix::signal`, which would itself fail to register one.
fn trappable(signal: i32) -> bool {
    signal != libc::SIGKILL && signal != libc::SIGSTOP
}

/// One signal's trap: the Luau callback currently registered.
struct Trap {
    handler: mlua::Function,
}

/// Every signal a script has trapped, keyed by number.
///
/// Shared between `proc.trap`'s closure and each signal's worker task via one
/// `Rc`, mirroring `task::Registry`. `handlers` holds a signal's `Trap` only
/// while something is currently registered for it -- `proc.untrap` removes
/// the entry outright, which is what makes an untrapped signal's delivery a
/// no-op in the worker loop (see [`spawn_trap_worker`]).
///
/// `workers` is tracked *separately*, and never shrinks: it records every
/// signal number a worker has ever been spawned for, so a
/// `trap`-`untrap`-`trap` sequence on one number does not spawn a second
/// worker racing the first over the same underlying `tokio::signal::unix`
/// registration. `handlers` alone cannot answer "does a worker already
/// exist" once `untrap` has emptied it for that number -- that is the whole
/// reason this is two collections rather than one.
#[derive(Default)]
struct Traps {
    handlers: std::collections::HashMap<i32, Trap>,
    workers: std::collections::HashSet<i32>,
}

/// Drive one signal's deliveries into whatever Luau function is currently
/// registered for it.
///
/// Spawned once per trapped signal number, for its whole remaining life --
/// unlike a `task.spawn`ed script body, this has no handle a script can await
/// or cancel, and outlives every individual `proc.trap` call that touches its
/// number.
///
/// # Deliveries can coalesce
///
/// POSIX signal delivery does not queue: if the same signal arrives twice
/// before this worker is next polled -- which, on ito's single-threaded
/// runtime, means before the script yields at some other await point -- the
/// two deliveries collapse into a single handler call. A script that must not
/// miss a coalesced pair needs a mechanism other than signals (a file, a
/// socket, a second signal number) to make the second delivery
/// distinguishable in the first place.
///
/// The stream is registered by the caller, synchronously inside `proc.trap`,
/// so a signal the OS refuses (SIGILL, SIGSEGV, a bogus number) raises there
/// instead of being logged from a detached task.
///
/// # Handler failures are logged, not propagated
///
/// A trap runs detached from the script's own control flow -- there is no
/// `await`/`poll` a caller could route an error through, the way there is for
/// `task.spawn`. An error is therefore logged and the worker moves on to the
/// next delivery, rather than failing the run the way an un-awaited
/// `task.spawn` does; a signal handler that means to stop the run calls
/// `proc.exit` itself.
fn spawn_trap_worker(
    traps: &std::rc::Rc<std::cell::RefCell<Traps>>,
    signal: i32,
    mut stream: tokio::signal::unix::Signal,
) {
    let traps = std::rc::Rc::clone(traps);
    tokio::task::spawn_local(async move {
        while stream.recv().await.is_some() {
            // Look the handler up fresh on every delivery rather than
            // capturing one at spawn time, so a later `proc.trap` call
            // replacing it takes effect immediately -- and clone it out
            // before the `await` below, since holding a `RefCell` borrow
            // across an await is a hazard the moment anything else on this
            // thread might touch `traps`.
            let handler = {
                let traps = traps.borrow();
                let Some(trap) = traps.handlers.get(&signal) else {
                    // Untrapped since the worker started (see `proc.untrap`):
                    // keep listening in case a later call re-traps it, rather
                    // than tearing the worker down and losing the signal
                    // number's place in the map.
                    continue;
                };
                trap.handler.clone()
            };
            if let Err(e) = handler.call_async::<()>(signal).await {
                tracing::error!("proc.trap: handler for signal {signal} failed: {e}");
            }
        }
    });
}

/// One `:env()`/`:env_remove()` step, applied in call order.
#[derive(Clone)]
enum EnvOp {
    Set(OsString, OsString),
    Remove(OsString),
}

#[derive(Clone)]
struct CommandState {
    // Program, arguments, environment and directory are `OsString`s: Luau
    // strings are bytes and so are Unix arguments, so nothing need be UTF-8.
    program: OsString,
    args: Vec<OsString>,
    env_ops: Vec<EnvOp>,
    env_clear: bool,
    current_dir: Option<PathBuf>,
    // `None` is "never configured", which is what `:exec()`/`:interactive()`
    // need to tell apart from an explicit target: the default for `:run()` is
    // NULL, for `:interactive()` it is INHERIT.
    stdin: Option<Stdin>,
    stdout: Option<Redirect>,
    stderr: Option<Redirect>,
}

/// Builder for a child process, finalized with `:run()` or `:spawn()`.
struct CommandBuilder {
    state: Arc<Mutex<CommandState>>,
}

impl CommandBuilder {
    fn new(program: OsString) -> Self {
        Self {
            state: Arc::new(Mutex::new(CommandState {
                program,
                args: Vec::new(),
                env_ops: Vec::new(),
                env_clear: false,
                current_dir: None,
                stdin: None,
                stdout: None,
                stderr: None,
            })),
        }
    }
}

/// Turn a `Redirect` into a `Stdio`. File targets open lazily on first use and
/// hand out `try_clone()`s afterwards, so a handle shared between stdout and
/// stderr yields two `Stdio`s over one file descriptor -- one write offset,
/// correctly interleaved output.
fn to_stdio(redirect: &Redirect) -> LuaResult<Stdio> {
    match redirect {
        Redirect::Null => Ok(Stdio::null()),
        Redirect::Inherit(target) => target.to_stdio(),
        Redirect::Capture { .. } => Ok(Stdio::piped()),
        Redirect::File { path, file, append } => Ok(Stdio::from(redirect::open_shared_file(
            path, file, *append,
        )?)),
        // Resolved before this point.
        Redirect::AliasStdout | Redirect::AliasStderr => {
            Err(mlua::Error::runtime("internal: unresolved stream alias"))
        }
    }
}

/// Split a `command` argument into a program and its arguments, accepting either
/// a bare program name or a list of `{program, args...}`.
pub(crate) fn parse_command_spec(spec: Value) -> LuaResult<(OsString, Vec<OsString>)> {
    match spec {
        Value::String(program) => Ok((redirect::os_string(&program), Vec::new())),
        Value::Table(list) => {
            let mut parts = Vec::new();
            for item in list.sequence_values::<mlua::LuaString>() {
                parts.push(redirect::os_string(&item?));
            }
            let mut parts = parts.into_iter();
            let program = parts.next().ok_or_else(|| {
                mlua::Error::runtime(
                    "command{} needs the program at index 1, but the list is empty",
                )
            })?;
            Ok((program, parts.collect()))
        }
        other => Err(mlua::Error::runtime(format!(
            "expected a program name or a list of {{program, args...}}, got {}",
            other.type_name()
        ))),
    }
}

impl UserData for CommandBuilder {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_function("arg", |_, (ud, arg): (AnyUserData, mlua::LuaString)| {
            let arg = redirect::os_string(&arg);
            ud.borrow::<Self>()?.state.lock().unwrap().args.push(arg);
            Ok(ud)
        });

        methods.add_function("args", |_, (ud, args): (AnyUserData, Vec<mlua::LuaString>)| {
            let args = args.iter().map(redirect::os_string);
            ud.borrow::<Self>()?.state.lock().unwrap().args.extend(args);
            Ok(ud)
        });

        methods.add_function(
            "env",
            |_, (ud, key, value): (AnyUserData, mlua::LuaString, mlua::LuaString)| {
                let op = EnvOp::Set(redirect::os_string(&key), redirect::os_string(&value));
                ud.borrow::<Self>()?.state.lock().unwrap().env_ops.push(op);
                Ok(ud)
            },
        );

        // `:envs{ A = "1", B = "2" }` is `:env` per pair (in no particular order).
        methods.add_function("envs", |_, (ud, vars): (AnyUserData, Table)| {
            let mut ops = Vec::new();
            for pair in vars.pairs::<mlua::LuaString, mlua::LuaString>() {
                let (k, v) = pair?;
                ops.push(EnvOp::Set(redirect::os_string(&k), redirect::os_string(&v)));
            }
            ud.borrow::<Self>()?
                .state
                .lock()
                .unwrap()
                .env_ops
                .extend(ops);
            Ok(ud)
        });

        // Unset one variable in the child, whether inherited or set earlier.
        methods.add_function(
            "env_remove",
            |_, (ud, key): (AnyUserData, mlua::LuaString)| {
                let op = EnvOp::Remove(redirect::os_string(&key));
                ud.borrow::<Self>()?.state.lock().unwrap().env_ops.push(op);
                Ok(ud)
            },
        );

        // As in `std::process::Command`, this also forgets earlier `:env()` calls.
        methods.add_function("env_clear", |_, ud: AnyUserData| {
            {
                let this = ud.borrow::<Self>()?;
                let mut state = this.state.lock().unwrap();
                state.env_clear = true;
                state.env_ops.clear();
            }
            Ok(ud)
        });

        methods.add_function(
            "current_dir",
            |_, (ud, dir): (AnyUserData, mlua::LuaString)| {
                let dir = PathBuf::from(redirect::os_string(&dir));
                ud.borrow::<Self>()?.state.lock().unwrap().current_dir = Some(dir);
                Ok(ud)
            },
        );

        // NULL (the default), INHERIT, `proc.file(path)` (read), or a string or
        // buffer fed to the child and then closed.
        methods.add_function("stdin", |_, (ud, source): (AnyUserData, Value)| {
            let source = stdin_from_value(&source)?;
            ud.borrow::<Self>()?.state.lock().unwrap().stdin = Some(source);
            Ok(ud)
        });

        methods.add_function("stdout", |_, (ud, target): (AnyUserData, Value)| {
            let redirect = redirect_from_value(&target)?;
            let builder = ud.borrow::<Self>()?;
            let mut state = builder.state.lock().unwrap();
            // Bind the alias now, like the shell's `1>&2` duplicates whatever
            // fd 2 currently is; reassigning stderr later must not follow.
            state.stdout = Some(bind_alias(redirect, state.stderr.as_ref(), Slot::Stdout)?);
            drop(state);
            drop(builder);
            Ok(ud)
        });

        methods.add_function("stderr", |_, (ud, target): (AnyUserData, Value)| {
            let redirect = redirect_from_value(&target)?;
            let builder = ud.borrow::<Self>()?;
            let mut state = builder.state.lock().unwrap();
            // Same eager binding as `:stdout()` -- `2>&1` snapshots stdout.
            state.stderr = Some(bind_alias(redirect, state.stdout.as_ref(), Slot::Stderr)?);
            drop(state);
            drop(builder);
            Ok(ud)
        });

        // --- finalizers ---------------------------------------------------

        // Start the child without waiting for it.
        methods.add_function("spawn", |_, ud: AnyUserData| {
            let state = ud.borrow::<Self>()?.state.lock().unwrap().clone();
            Child::spawn(state, false, false)
        });

        // Start the child and wait for it, returning its result.
        methods.add_async_function("run", |lua, ud: AnyUserData| async move {
            let state = ud.borrow::<Self>()?.state.lock().unwrap().clone();
            run_to_end(&lua, state, false).await
        });

        // Replace this process with the child via `execve`, exactly as the
        // shell's `exec` builtin does. On success this never returns to
        // Luau -- the calling process's image, pid and open fds all become
        // the new program's -- so there is no result table and no child
        // left to wait for or capture from.
        methods.add_function("exec", |_, ud: AnyUserData| {
            let state = ud.borrow::<Self>()?.state.lock().unwrap().clone();
            do_exec(state)
        });

        // Start the child on the real terminal -- stdin/stdout/stderr default
        // to INHERIT, unlike `:run()`'s NULL -- and wait for it, for a child
        // that needs one (a pager, an editor, a password prompt). Any stream
        // can still be redirected explicitly (`fzf`'s selection on stdout).
        methods.add_async_function("interactive", |lua, ud: AnyUserData| async move {
            let state = ud.borrow::<Self>()?.state.lock().unwrap().clone();
            run_to_end(&lua, state, true).await
        });
    }
}

/// Spawn, wait, and build the result: `:run()` and `:interactive()`.
///
/// This `Child` never reaches Luau, so there is no borrow to scope -- it is
/// owned here. It is spawned with `kill_on_drop`, so a run abandoned mid-wait
/// (a `task.timeout` expiring) takes its child with it instead of orphaning it.
async fn run_to_end(lua: &Lua, state: CommandState, interactive: bool) -> LuaResult<Table> {
    // The child has the terminal to itself for the duration, so ^C must not
    // take ito down while the child (an editor, say) is deciding what it means.
    let _guard = interactive.then(IgnoreInterrupts::new);
    let mut child = Child::spawn(state, interactive, true)?;
    let (mut inner, mut readers) = child.take_for_wait()?;
    Child::wait_on(
        lua,
        &child.program,
        &child.captured,
        &mut inner,
        &mut readers,
    )
    .await
}

/// Ignores SIGINT/SIGQUIT in this process while alive, restoring whatever
/// disposition was there before (a `proc.trap` handler included) on drop.
/// Children spawned with `Child::spawn(.., interactive = true)` reset both to
/// default in `pre_exec`, since an ignored signal stays ignored across `exec`.
pub(crate) struct IgnoreInterrupts {
    saved: [(i32, libc::sigaction); 2],
}

impl IgnoreInterrupts {
    pub(crate) fn new() -> Self {
        let swap = |sig: i32| {
            // SAFETY: `sigaction` with valid, zero-initialized structs.
            unsafe {
                let mut ignore: libc::sigaction = std::mem::zeroed();
                ignore.sa_sigaction = libc::SIG_IGN;
                libc::sigemptyset(&mut ignore.sa_mask);
                let mut old: libc::sigaction = std::mem::zeroed();
                libc::sigaction(sig, &ignore, &mut old);
                (sig, old)
            }
        };
        Self {
            saved: [swap(libc::SIGINT), swap(libc::SIGQUIT)],
        }
    }
}

impl Drop for IgnoreInterrupts {
    fn drop(&mut self) {
        for (sig, old) in &self.saved {
            // SAFETY: restoring a disposition `sigaction` itself handed back.
            unsafe { libc::sigaction(*sig, old, std::ptr::null_mut()) };
        }
    }
}

/// The child's environment: ours (unless cleared) with the ops applied in order.
fn build_env(state: &CommandState) -> Vec<(OsString, OsString)> {
    let mut env: Vec<(OsString, OsString)> = if state.env_clear {
        Vec::new()
    } else {
        std::env::vars_os().collect()
    };
    for op in &state.env_ops {
        match op {
            EnvOp::Set(k, v) => match env.iter_mut().find(|(ek, _)| ek == k) {
                Some(entry) => entry.1 = v.clone(),
                None => env.push((k.clone(), v.clone())),
            },
            EnvOp::Remove(k) => env.retain(|(ek, _)| ek != k),
        }
    }
    env
}

/// `execve` the current process into `state`'s program, inheriting our own
/// stdin/stdout/stderr -- there is no fork, so a `:stdout()`/`:stderr()`
/// redirect configured on the builder has no process left to apply to. Only
/// INHERIT (spelled out or left as the default) is accepted.
fn do_exec(state: CommandState) -> LuaResult<()> {
    let inherit = |r: &Option<Redirect>| matches!(r, None | Some(Redirect::Inherit(InheritTarget::Same)));
    if !inherit(&state.stdout)
        || !inherit(&state.stderr)
        || !matches!(state.stdin, None | Some(Stdin::Inherit))
    {
        return Err(mlua::Error::runtime(
            ":exec() replaces this process and inherits its stdin/stdout/stderr as-is; \
             it does not support :stdin()/:stdout()/:stderr() other than proc.INHERIT",
        ));
    }

    use std::ffi::CString;
    use std::io::Write;
    use std::os::unix::ffi::OsStrExt;

    let to_cstring = |s: &OsStr| {
        CString::new(s.as_bytes()).map_err(|_| {
            mlua::Error::runtime(format!("argument contains a NUL byte: {:?}", s))
        })
    };

    // The environment is computed, not applied to this process: a failed exec
    // must leave us as it found us.
    let env = build_env(&state);

    // `:run()`/`:spawn()` resolve the program through `$PATH` via
    // `std::process::Command`; a bare `execve` would not, which would make
    // `:exec()` the one finalizer that requires an absolute path for the
    // same `spec`. There is no portable `execvpe` (glibc/musl have one, but
    // macOS does not), so the search is done by hand over the *child's* PATH
    // and `execve` used with each candidate.
    let path = env.iter().find(|(k, _)| k == "PATH").map(|(_, v)| v.as_os_str());
    let candidates = resolve_program(&state.program, path);

    let mut argv: Vec<CString> = Vec::with_capacity(state.args.len() + 1);
    argv.push(to_cstring(&state.program)?);
    for arg in &state.args {
        argv.push(to_cstring(arg)?);
    }

    let envp: Vec<CString> = env
        .iter()
        .map(|(k, v)| {
            let mut bytes = k.as_bytes().to_vec();
            bytes.push(b'=');
            bytes.extend_from_slice(v.as_bytes());
            CString::new(bytes)
                .map_err(|_| mlua::Error::runtime("environment variable contains a NUL byte"))
        })
        .collect::<LuaResult<_>>()?;

    let mut argv_ptrs: Vec<*const libc::c_char> = argv
        .iter()
        .map(|s| s.as_ptr())
        .chain(std::iter::once(std::ptr::null()))
        .collect();
    let mut envp_ptrs: Vec<*const libc::c_char> = envp
        .iter()
        .map(|s| s.as_ptr())
        .chain(std::iter::once(std::ptr::null()))
        .collect();

    // Anything we printed must land before the new program's output.
    let _ = std::io::stdout().flush();

    // The one piece of process state we must change; put back on failure.
    let previous_dir = match &state.current_dir {
        Some(dir) => {
            let previous = std::env::current_dir().ok();
            std::env::set_current_dir(dir).map_err(|e| {
                mlua::Error::runtime(format!("failed to chdir to '{}': {e}", dir.display()))
            })?;
            previous
        }
        None => None,
    };

    // Like `execvp`: a missing or non-directory candidate moves on to the next
    // `$PATH` entry, a permission failure is remembered (and reported in
    // preference to "not found", which would hide it), anything else is final.
    let mut denied = false;
    let mut failure = None;
    for candidate in candidates {
        // SAFETY: `candidate`/`argv`/`envp` are NUL-terminated C strings kept
        // alive until after the call, and both pointer vectors are
        // NULL-terminated as `execve` requires. `execve` only returns on
        // failure, in which case nothing has been replaced and it is safe to
        // try the next `$PATH` candidate or report the error.
        unsafe {
            libc::execve(
                candidate.as_ptr(),
                argv_ptrs.as_mut_ptr(),
                envp_ptrs.as_mut_ptr(),
            );
        }
        let err = std::io::Error::last_os_error();
        match err.raw_os_error() {
            Some(libc::ENOENT | libc::ENOTDIR) => {}
            Some(libc::EACCES) => denied = true,
            _ => {
                failure = Some(err);
                break;
            }
        }
    }
    if let Some(previous) = previous_dir {
        let _ = std::env::set_current_dir(previous);
    }
    let failure = failure.unwrap_or_else(|| {
        std::io::Error::from_raw_os_error(if denied { libc::EACCES } else { libc::ENOENT })
    });
    Err(mlua::Error::runtime(format!(
        "failed to exec '{}': {failure}",
        state.program.to_string_lossy()
    )))
}

/// The candidate paths to try `execve`ing, in order.
///
/// A program containing a `/` is used as-is, matching `execvp`'s own rule
/// (and `std::process::Command`'s) -- `$PATH` is only consulted for a bare
/// name. Otherwise each `$PATH` entry is tried in turn, an empty entry
/// meaning the current directory, same as the shell and `execvp` treat it.
fn resolve_program(program: &OsStr, path: Option<&OsStr>) -> Vec<std::ffi::CString> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    if program.as_bytes().contains(&b'/') {
        return CString::new(program.as_bytes()).into_iter().collect();
    }

    let path = path.unwrap_or_else(|| OsStr::new("/usr/bin:/bin"));
    std::env::split_paths(path)
        .map(|dir| {
            if dir.as_os_str().is_empty() {
                PathBuf::from(program)
            } else {
                dir.join(program)
            }
        })
        .filter_map(|p| CString::new(p.into_os_string().into_encoded_bytes()).ok())
        .collect()
}

/// Which stream a captured byte stream belongs to once collected.
///
/// `Clone` so `:wait()` can copy it out under a scoped borrow rather than
/// holding one across the await; it is three `Option<CaptureKind>`s, so the
/// copy is free.
#[derive(Clone)]
pub(crate) struct Captured {
    stdout: Option<CaptureKind>,
    stderr: Option<CaptureKind>,
    /// Set when one handle serves both streams, so both report one value.
    merged: Option<CaptureKind>,
}

impl Captured {
    /// What the two (concrete) targets capture. Shared with `ssh`.
    pub(crate) fn of(stdout: &Redirect, stderr: &Redirect) -> Self {
        let kind = |r: &Redirect| match r {
            Redirect::Capture { kind, .. } => Some(*kind),
            _ => None,
        };
        Self {
            stdout: kind(stdout),
            stderr: kind(stderr),
            merged: merged_capture(stdout, stderr),
        }
    }
}

/// The result table (`success`/`status`/`signal`/captures) for a reaped
/// child, built here so `ssh`'s cannot drift from `proc`'s.
pub(crate) fn result_table(
    lua: &Lua,
    captured: &Captured,
    status: std::process::ExitStatus,
    stdout_bytes: &[u8],
    stderr_bytes: &[u8],
) -> LuaResult<Table> {
    Child::result(lua, captured, status, stdout_bytes, stderr_bytes)
}

/// A spawned child process, awaited with `:wait()`.
///
/// Capture readers start draining as soon as the child is spawned: a child that
/// fills a pipe's buffer would otherwise block before `:wait()` is ever called.
struct Child {
    program: String,
    child: Option<tokio::process::Child>,
    captured: Captured,
    /// Reader tasks, awaited by `wait()`. `(stdout, stderr)`.
    readers: (Option<Reader>, Option<Reader>),
}

impl Child {
    /// `interactive` flips the defaults to the terminal (stdin/stdout/stderr
    /// all INHERIT) and resets SIGINT/SIGQUIT in the child; `kill_on_drop`
    /// makes dropping the handle kill the child, which only a run that owns
    /// its child wants (a `:spawn()`ed handle going out of scope must not).
    fn spawn(state: CommandState, interactive: bool, kill_on_drop: bool) -> LuaResult<Self> {
        // Aliases were already bound by `:stdout()`/`:stderr()`, so both
        // targets are concrete by now.
        let default_out = if interactive {
            Redirect::Inherit(InheritTarget::Same)
        } else {
            Redirect::Null
        };
        let stdout_target = state.stdout.clone().unwrap_or_else(|| default_out.clone());
        let stderr_target = state.stderr.clone().unwrap_or(default_out);

        if let Some(path) = conflicting_files(&stdout_target, &stderr_target) {
            return Err(mlua::Error::runtime(format!(
                "stdout and stderr both target '{}' via separate handles, \
                 which would give each its own write offset and corrupt the \
                 file; reuse one proc.file() handle to merge them",
                path.display()
            )));
        }

        let mut cmd = Command::new(&state.program);
        cmd.args(&state.args);
        if state.env_clear {
            cmd.env_clear();
        }
        for op in &state.env_ops {
            match op {
                EnvOp::Set(k, v) => cmd.env(k, v),
                EnvOp::Remove(k) => cmd.env_remove(k),
            };
        }
        if let Some(dir) = &state.current_dir {
            cmd.current_dir(dir);
        }
        cmd.kill_on_drop(kill_on_drop);
        if interactive {
            // SAFETY: only async-signal-safe `signal(2)` calls between fork and exec.
            unsafe {
                cmd.pre_exec(|| {
                    libc::signal(libc::SIGINT, libc::SIG_DFL);
                    libc::signal(libc::SIGQUIT, libc::SIG_DFL);
                    Ok(())
                });
            }
        }

        // Open every target before spawning, so a target that cannot be opened
        // fails the call before a child exists.
        let mut input = None;
        let stdin = state
            .stdin
            .clone()
            .unwrap_or(if interactive { Stdin::Inherit } else { Stdin::Null });
        match &stdin {
            Stdin::Null => cmd.stdin(Stdio::null()),
            Stdin::Inherit => cmd.stdin(Stdio::inherit()),
            Stdin::File(_) => cmd.stdin(Stdio::from(stdin.open_file()?.expect("a file source"))),
            Stdin::Bytes(bytes) => {
                input = Some(bytes.clone());
                cmd.stdin(Stdio::piped())
            }
        };

        // A capture handle shared by both streams has to become one pipe that
        // both write into; `Stdio::piped()` twice would create two independent
        // pipes and stderr's output would be dropped.
        let merged = merged_capture(&stdout_target, &stderr_target);

        let mut merged_reader = None;
        if merged.is_some() {
            let (out, err, reader) = redirect::merged_pipe()?;
            cmd.stdout(Stdio::from(out));
            cmd.stderr(Stdio::from(err));
            merged_reader = Some(reader);
        } else {
            cmd.stdout(to_stdio(&stdout_target)?);
            cmd.stderr(to_stdio(&stderr_target)?);
        }

        let mut child = cmd.spawn().map_err(|e| {
            mlua::Error::runtime(format!(
                "failed to run '{}': {e}",
                state.program.to_string_lossy()
            ))
        })?;

        // Dropping `cmd` closes this process's copies of the pipe writers, so
        // the readers below see EOF when the child exits rather than hanging.
        drop(cmd);

        if let (Some(bytes), Some(pipe)) = (input, child.stdin.take()) {
            redirect::feed(pipe, bytes);
        }

        // Drain each capture on its own task, from here rather than from
        // `wait()`: reading both inline would deadlock once one pipe's buffer
        // filled, and reading lazily would block the child before then.
        let readers = if let Some(reader) = merged_reader {
            (Some(reader), None)
        } else {
            (
                child.stdout.take().map(read_to_end),
                child.stderr.take().map(read_to_end),
            )
        };

        Ok(Self {
            program: state.program.to_string_lossy().into_owned(),
            child: Some(child),
            captured: Captured::of(&stdout_target, &stderr_target),
            readers,
        })
    }

    /// Everything `wait` needs, moved out of the `Child` in one go.
    ///
    /// Taken under a *scoped* borrow so the await that follows holds none: a
    /// `:wait()` abandoned mid-flight (a `task.timeout` expiring, say) would
    /// otherwise keep mlua's borrow alive for as long as the dropped future
    /// existed, and every later method on that child -- `:kill()` above all --
    /// would fail with a borrow error rather than doing its job.
    ///
    /// Taking is only half of it: an abandoned wait must also *give the pieces
    /// back*, or the handle is consumed by a wait that never happened. See
    /// `WaitGuard`.
    fn take_for_wait(
        &mut self,
    ) -> LuaResult<(tokio::process::Child, (Option<Reader>, Option<Reader>))> {
        let child = self.child.take().ok_or_else(consumed)?;
        Ok((child, (self.readers.0.take(), self.readers.1.take())))
    }

    /// Wait for exit and build the result table.
    ///
    /// Takes the child and its readers by value rather than borrowing `self`,
    /// so no mlua borrow is held across an await (see `take_for_wait`).
    ///
    /// Every await here is a real yield point, so a child running under
    /// `task.spawn` never pins the runtime's only thread -- concurrency in
    /// `task` is only as real as the awaits underneath it.
    ///
    /// The capture readers are joined *before* the child is awaited, so a
    /// grandchild that inherited the pipe and outlives the child keeps this
    /// waiting until it closes it too.
    async fn wait_on(
        lua: &Lua,
        program: &str,
        captured: &Captured,
        child: &mut tokio::process::Child,
        readers: &mut (Option<Reader>, Option<Reader>),
    ) -> LuaResult<Table> {
        let stdout_bytes = join_reader(readers.0.take(), "stdout").await?;
        let stderr_bytes = join_reader(readers.1.take(), "stderr").await?;

        let status = child
            .wait()
            .await
            .map_err(|e| mlua::Error::runtime(format!("failed waiting for '{program}': {e}")))?;
        Self::result(lua, captured, status, &stdout_bytes, &stderr_bytes)
    }

    /// The result table for a reaped child.
    fn result(
        lua: &Lua,
        captured: &Captured,
        status: std::process::ExitStatus,
        stdout_bytes: &[u8],
        stderr_bytes: &[u8],
    ) -> LuaResult<Table> {
        let result = lua.create_table()?;
        result.raw_set("success", status.success())?;
        match status.code() {
            Some(code) => result.raw_set("status", code)?,
            // Killed by a signal; no exit code exists.
            None => result.raw_set("status", Value::Nil)?,
        }
        set_signal_fields(&result, status.signal())?;

        if let Some(kind) = captured.merged {
            // One handle for both streams: everything arrived on the shared
            // pipe, so both fields expose the same interleaved value rather
            // than leaving one silently empty.
            let value = capture_to_value(lua, kind, stdout_bytes)?;
            result.raw_set("stdout", value.clone())?;
            result.raw_set("stderr", value)?;
        } else {
            if let Some(kind) = captured.stdout {
                result.raw_set("stdout", capture_to_value(lua, kind, stdout_bytes)?)?;
            }
            if let Some(kind) = captured.stderr {
                result.raw_set("stderr", capture_to_value(lua, kind, stderr_bytes)?)?;
            }
        }

        Ok(result)
    }
}

fn consumed() -> mlua::Error {
    mlua::Error::runtime(
        "this child has already been waited for (:wait() and :kill() both consume it)",
    )
}

/// Holds a child's moved-out pieces for the duration of a wait, and puts them
/// back if that wait is abandoned.
///
/// `:wait()` has to take the child out from under mlua's borrow (holding the
/// borrow across the await would brick the handle) — but a future can be
/// *dropped* mid-await, which `task.timeout` does routinely. Without this, the
/// pieces would go down with the dropped future and the handle would report
/// "already been waited for" despite no wait having completed, leaving a live
/// child that could no longer be killed.
///
/// The guard owns the pieces *for* the wait and lends them to it, so it is
/// still alive — and still able to restore them — if the future is dropped
/// part-way through.
struct WaitGuard<'a> {
    ud: &'a AnyUserData,
    child: Option<tokio::process::Child>,
    readers: (Option<Reader>, Option<Reader>),
}

impl WaitGuard<'_> {
    /// Run the wait, keeping ownership of the pieces throughout.
    ///
    /// The wait *borrows* the child rather than consuming it, so an abandoned
    /// future leaves it here for `Drop` to restore. On success the guard is
    /// disarmed by the caller, since the child has been reaped.
    async fn run(&mut self, lua: &Lua, program: &str, captured: &Captured) -> LuaResult<Table> {
        let child = self.child.as_mut().expect("armed guard holds a child");
        Child::wait_on(lua, program, captured, child, &mut self.readers).await
    }

    /// The wait finished and reaped the child, so there is nothing to give back.
    fn disarm(&mut self) {
        self.child = None;
    }
}

impl Drop for WaitGuard<'_> {
    fn drop(&mut self) {
        let Some(child) = self.child.take() else {
            return; // disarmed: the wait finished and owns the pieces now.
        };
        // A failed borrow here would mean something else is mid-method on this
        // same child, which cannot happen on a single-threaded runtime; if it
        // somehow did, dropping the child is still the honest outcome.
        if let Ok(mut this) = self.ud.borrow_mut::<Child>() {
            this.child = Some(child);
            this.readers = (self.readers.0.take(), self.readers.1.take());
        }
    }
}

impl UserData for Child {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        // `add_async_function` with a scoped borrow, *not* `add_async_method_mut`:
        // the latter holds mlua's borrow for the whole future, so a `:wait()`
        // abandoned mid-flight (`task.timeout` expiring around one) left the
        // borrow alive as long as the dropped future and bricked the handle --
        // the follow-up `:kill()` failed with a borrow error instead of killing.
        methods.add_async_function("wait", |lua, ud: AnyUserData| async move {
            let (program, captured, mut guard) = {
                let mut this = ud.borrow_mut::<Self>()?;
                let (child, readers) = this.take_for_wait()?;
                let guard = WaitGuard {
                    ud: &ud,
                    child: Some(child),
                    readers,
                };
                (this.program.clone(), this.captured.clone(), guard)
            };
            let result = guard.run(&lua, &program, &captured).await?;
            guard.disarm();
            Ok(result)
        });

        // Non-blocking `:wait()`: `nil` while running; once exited, the result
        // table, consuming the child like `:wait()`. The borrow is scoped for
        // the same reason as there (the reader joins below are awaits).
        methods.add_async_function("try_wait", |lua, ud: AnyUserData| async move {
            let (captured, status, mut readers) = {
                let mut this = ud.borrow_mut::<Self>()?;
                let program = this.program.clone();
                let child = this.child.as_mut().ok_or_else(consumed)?;
                let status = child.try_wait().map_err(|e| {
                    mlua::Error::runtime(format!("failed waiting for '{program}': {e}"))
                })?;
                let Some(status) = status else {
                    return Ok(Value::Nil);
                };
                this.child = None;
                let readers = (this.readers.0.take(), this.readers.1.take());
                (this.captured.clone(), status, readers)
            };
            let out = join_reader(readers.0.take(), "stdout").await?;
            let err = join_reader(readers.1.take(), "stderr").await?;
            Ok(Value::Table(Self::result(&lua, &captured, status, &out, &err)?))
        });

        // `Child::id` also goes to None once the child has been reaped, which
        // is the same condition the taken-`child` arm reports.
        methods.add_method("id", |_, this, ()| {
            match this.child.as_ref().and_then(tokio::process::Child::id) {
                Some(id) => Ok(id),
                None => Err(mlua::Error::runtime(
                    "this child has already been waited for (:wait() and :kill() both consume it)",
                )),
            }
        });

        // Send an arbitrary signal *without* consuming the child: a signal is a
        // request, not a verdict -- SIGTERM may be ignored, delayed or handled,
        // and SIGHUP/SIGUSR1 need not end the process at all. `:wait()` remains
        // the caller's to do, which is also what keeps the pid live so a second
        // signal (the TERM-then-KILL escalation) can follow.
        methods.add_method("signal", |_, this, sig: Value| {
            let number = signal_number(&sig)?;
            let Some(pid) = this.child.as_ref().and_then(tokio::process::Child::id) else {
                return Err(mlua::Error::runtime(
                    "this child has already been waited for (:wait() and :kill() both consume it)",
                ));
            };
            send_signal(pid, number).map_err(|e| {
                mlua::Error::runtime(format!("failed to signal '{}': {e}", this.program))
            })
        });

        // SIGKILL the child, then reap it so callers cannot leave a zombie by
        // forgetting to wait. Returns what :wait() would, including any output
        // the child managed to write before dying.
        // Scoped borrow for the same reason `:wait()` uses one: the reap below
        // is an await, and holding the borrow across it would brick the handle
        // for anything that ran afterwards.
        methods.add_async_function("kill", |lua, ud: AnyUserData| async move {
            let (program, captured, mut guard) = {
                let mut this = ud.borrow_mut::<Self>()?;
                let program = this.program.clone();
                match this.child.as_mut() {
                    // `start_kill` only signals; `Child::kill` would reap too,
                    // and the reaping belongs to the wait below so both paths
                    // build the result table the same way.
                    Some(child) => child.start_kill().map_err(|e| {
                        mlua::Error::runtime(format!("failed to kill '{program}': {e}"))
                    })?,
                    None => {
                        return Err(mlua::Error::runtime(
                            "this child has already been waited for (:wait() and :kill() both consume it)",
                        ));
                    }
                }
                let (child, readers) = this.take_for_wait()?;
                let guard = WaitGuard {
                    ud: &ud,
                    child: Some(child),
                    readers,
                };
                (program, this.captured.clone(), guard)
            };
            let result = guard.run(&lua, &program, &captured).await?;
            guard.disarm();
            Ok(result)
        });
    }
}

/// Build the `proc` module table.
pub fn module(lua: &Lua) -> LuaResult<Table> {
    let exports = lua.create_table()?;

    // Accepts either a program name or a list whose first element is the
    // program and the rest are its arguments.
    exports.raw_set(
        "command",
        lua.create_function(|_, spec: Value| {
            let (program, args) = parse_command_spec(spec)?;
            let builder = CommandBuilder::new(program);
            builder.state.lock().unwrap().args.extend(args);
            Ok(builder)
        })?,
    )?;

    // Redirect targets (`proc.string()`, `proc.file(..)`, `proc.NULL`, ...) live
    // here rather than in a module of their own, even though `ssh` takes the
    // same values -- they are arguments to a command, and this is where commands
    // are built.
    crate::scripting::redirect::install(lua, &exports)?;

    // `proc.SIGTERM` and friends, for `Child:signal()`. Numbers, so they are
    // also what `result.signal` compares against.
    install_signal_constants(&exports)?;

    // `proc.trap`/`proc.untrap`/`proc.exit`: the one place a script opts into
    // reacting to a signal sent to *this* process (as opposed to `:signal()`,
    // which sends one to a child). One `Rc` per `Lua`, so every `proc.trap`
    // call and every signal's worker share the same map.
    let traps: std::rc::Rc<std::cell::RefCell<Traps>> = Default::default();

    // proc.trap(sig, f) -> nil
    //
    // Registers `f` to run on every delivery of `sig` from here on; an
    // earlier trap for the same signal is replaced, not stacked. Signals with
    // no trap keep their default disposition entirely -- registering the
    // first trap for a number is what takes it off that disposition, so
    // deliveries before the first `proc.trap` call for a given signal are not
    // caught by anything this installs.
    exports.raw_set("trap", {
        let traps = std::rc::Rc::clone(&traps);
        lua.create_function(move |_, (sig, handler): (Value, mlua::Function)| {
            let signal = signal_number(&sig)?;
            if !trappable(signal) {
                return Err(mlua::Error::runtime(format!(
                    "signal {} cannot be trapped -- the kernel never delivers it to a handler",
                    signal_name(signal).unwrap_or_else(|| signal.to_string())
                )));
            }
            // Register with the OS *before* touching any state, synchronously,
            // so a signal the OS rejects (SIGILL/SIGFPE/SIGSEGV/SIGBUS, a bogus
            // number) raises here. Whether a worker exists is `workers`'
            // question, not `handlers`': `proc.untrap` empties `handlers` for
            // this signal without touching `workers`, so a re-`trap` after an
            // `untrap` must not spawn a second one.
            let new_stream = if traps.borrow().workers.contains(&signal) {
                None
            } else {
                let kind = tokio::signal::unix::SignalKind::from_raw(signal);
                Some(tokio::signal::unix::signal(kind).map_err(|e| {
                    mlua::Error::runtime(format!("cannot trap signal {signal}: {e}"))
                })?)
            };
            traps.borrow_mut().handlers.insert(signal, Trap { handler });
            if let Some(stream) = new_stream {
                traps.borrow_mut().workers.insert(signal);
                spawn_trap_worker(&traps, signal, stream);
            }
            Ok(())
        })?
    })?;

    // proc.untrap(sig) -> nil
    //
    // Removes whatever handler is registered for `sig`; a signal with nothing
    // registered is a no-op, not an error, so a script can call this
    // defensively without first checking whether it ever trapped the signal.
    // The worker spawned for `sig` (if any) keeps running rather than being
    // torn down, so a later `proc.trap` for the same number needs no new
    // registration -- see `spawn_trap_worker`.
    exports.raw_set("untrap", {
        let traps = std::rc::Rc::clone(&traps);
        lua.create_function(move |_, sig: Value| {
            let signal = signal_number(&sig)?;
            traps.borrow_mut().handlers.remove(&signal);
            Ok(())
        })?
    })?;

    // proc.exit(code) -> never returns
    //
    // A direct `std::process::exit`, not a Luau-level unwind: nothing runs
    // between the call and the process ending -- no `Drop`, no pending
    // `task`, no VFS flush -- on purpose, so a trap handler that decides the
    // process must end right now is not itself at the mercy of whatever else
    // might be stuck (a hung `child:wait()`, a wedged pool). A script that
    // wants its cleanup to run first should arrange that before calling this,
    // not rely on it.
    exports.raw_set(
        "exit",
        lua.create_function(|_, code: Option<i32>| -> LuaResult<()> {
            std::process::exit(code.unwrap_or(0));
        })?,
    )?;

    Ok(exports)
}

/// Install the `proc` module as a global.
pub fn register(lua: &Lua) -> LuaResult<()> {
    lua.globals().raw_set("proc", module(lua)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mlua::ObjectLike;

    /// Run `script` on its own runtime.
    ///
    /// `enable_all` is what makes this work: `tokio::process` needs the signal
    /// driver to reap children and the I/O driver to poll their pipes. The
    /// `LocalSet` is required too, since the readers are `spawn_local`. Together
    /// they match how the host drives scripts (see `scripting::task`).
    fn eval(script: &str) -> LuaResult<()> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build test runtime");
        let local = tokio::task::LocalSet::new();
        local.block_on(&runtime, async {
            let lua = Lua::new();
            // Redirect targets are installed onto the same table.
            lua.globals().set("proc", module(&lua)?)?;
            lua.load(script).exec_async().await
        })
    }

    #[test]
    fn defaults_discard_both_streams() {
        // Nothing configured: output must not reach the terminal, and no
        // capture fields appear in the result.
        eval(
            r#"
            local r = proc.command("sh"):arg("-c"):arg("echo o; echo e >&2"):run()
            assert(r.success, "expected success")
            assert(r.stdout == nil, "stdout should not be captured")
            assert(r.stderr == nil, "stderr should not be captured")
        "#,
        )
        .unwrap();
    }

    #[test]
    fn captures_as_string_and_buffer() {
        eval(
            r#"
            local s = proc.command("printf"):arg("hi")
                :stdout(proc.string()):run()
            assert(s.stdout == "hi", "expected string capture")

            local b = proc.command("printf"):arg("hi")
                :stdout(proc.buffer()):run()
            assert(buffer.len(b.stdout) == 2, "expected buffer capture")
        "#,
        )
        .unwrap();
    }

    /// Captured output may be arbitrary bytes, so nothing may be UTF-8 validated.
    #[test]
    fn preserves_non_utf8_bytes() {
        eval(
            r#"
            local r = proc.command("printf"):arg("\\377\\376")
                :stdout(proc.buffer()):run()
            assert(buffer.len(r.stdout) == 2, "expected 2 bytes")
            assert(buffer.readu8(r.stdout, 0) == 255, "expected 0xff")
            assert(buffer.readu8(r.stdout, 1) == 254, "expected 0xfe")
        "#,
        )
        .unwrap();
    }

    /// A shared capture handle must collect both streams into one value.
    #[test]
    fn shared_capture_handle_merges_streams() {
        eval(
            r#"
            local both = proc.string()
            local r = proc.command("sh"):arg("-c"):arg("echo o; echo e >&2")
                :stdout(both):stderr(both):run()
            assert(r.stdout == r.stderr, "both fields should hold the merged value")
            assert(r.stdout:find("o"), "missing stdout output")
            assert(r.stdout:find("e"), "missing stderr output")
        "#,
        )
        .unwrap();
    }

    /// `proc.STDOUT` binds when assigned, like the shell's `2>&1`, so a later
    /// `:stdout()` must not drag stderr along with it.
    #[test]
    fn stream_alias_binds_eagerly() {
        eval(
            r#"
            local cap = proc.string()
            local r = proc.command("sh"):arg("-c"):arg("echo o; echo e >&2")
                :stdout(cap)
                :stderr(proc.STDOUT)
                :stdout(proc.NULL)
                :run()
            -- stderr still points at the capture; stdout was discarded.
            assert(r.stderr:find("e"), "stderr should still be captured")
            assert(not r.stderr:find("o"), "stdout should have been discarded")
        "#,
        )
        .unwrap();
    }

    #[test]
    fn same_path_via_separate_handles_is_rejected() {
        let err = eval(
            r#"
            local a = proc.file("/tmp/de-test-conflict.log")
            local b = proc.file("/tmp/de-test-conflict.log")
            proc.command("echo"):arg("x"):stdout(a):stderr(b):run()
        "#,
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("separate handles"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn nonzero_exit_reports_status_without_throwing() {
        eval(
            r#"
            local r = proc.command("sh"):arg("-c"):arg("exit 7"):run()
            assert(r.status == 7, "expected status 7")
            assert(r.success == false, "expected failure")
        "#,
        )
        .unwrap();
    }

    /// Failure to *launch* is an error, unlike a nonzero exit from a program
    /// that did run.
    #[test]
    fn missing_program_throws() {
        let err = eval(r#"proc.command("de-no-such-binary-xyz"):run()"#).unwrap_err();
        assert!(err.to_string().contains("failed to run"), "got: {err}");
    }

    /// `command{prog, args...}` is equivalent to `command(prog):args{...}`.
    #[test]
    fn command_accepts_program_and_args_as_list() {
        eval(
            r#"
            local r = proc.command({ "printf", "%s-%s", "a", "b" })
                :stdout(proc.string()):run()
            assert(r.stdout == "a-b", "got: " .. tostring(r.stdout))
        "#,
        )
        .unwrap();
    }

    /// Arguments from the list come first, then any added afterwards.
    #[test]
    fn list_form_composes_with_arg() {
        eval(
            r#"
            local r = proc.command({ "printf", "%s-%s" })
                :arg("x"):arg("y")
                :stdout(proc.string()):run()
            assert(r.stdout == "x-y", "got: " .. tostring(r.stdout))
        "#,
        )
        .unwrap();
    }

    #[test]
    fn empty_command_list_throws() {
        let err = eval(r#"proc.command({})"#).unwrap_err();
        assert!(err.to_string().contains("index 1"), "got: {err}");
    }

    /// `:spawn()` then `:wait()` must agree with the one-shot `:run()`.
    #[test]
    fn spawn_and_wait_matches_run() {
        eval(
            r#"
            local child = proc.command({ "printf", "hi" })
                :stdout(proc.string()):spawn()
            assert(type(child:id()) == "number", "expected a pid")
            local waited = child:wait()

            local one_shot = proc.command({ "printf", "hi" })
                :stdout(proc.string()):run()

            assert(waited.stdout == "hi", "got: " .. tostring(waited.stdout))
            assert(waited.stdout == one_shot.stdout, "forms disagree")
        "#,
        )
        .unwrap();
    }

    #[test]
    fn waiting_twice_throws() {
        let err = eval(
            r#"
            local child = proc.command({ "true" }):spawn()
            child:wait()
            child:wait()
        "#,
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("already been waited"),
            "got: {err}"
        );
    }

    /// `kill()` reaps the child itself and reports SIGKILL death, so callers
    /// cannot leave a zombie by forgetting to wait.
    #[test]
    fn kill_reaps_and_reports_signal_death() {
        eval(
            r#"
            local child = proc.command({ "sleep", "30" }):spawn()
            local r = child:kill()
            assert(type(r) == "table", "kill should return the result")
            assert(r.success == false, "expected failure")
            assert(r.status == nil, "signal death should have no exit code")
        "#,
        )
        .unwrap();
    }

    /// The three spellings of one signal must reach the same process, and the
    /// result must say which one killed it.
    #[test]
    fn signal_by_name_prefix_and_number() {
        for spelling in [
            r#""TERM""#,
            r#""SIGTERM""#,
            r#""sigterm""#,
            "15",
            "proc.SIGTERM",
        ] {
            eval(&format!(
                r#"
                local child = proc.command({{ "sleep", "30" }}):spawn()
                child:signal({spelling})
                local r = child:wait()
                assert(r.success == false, "expected failure")
                assert(r.status == nil, "signal death has no exit code")
                assert(r.signal == 15, "expected signal 15, got " .. tostring(r.signal))
                assert(r.signal_name == "SIGTERM", "got " .. tostring(r.signal_name))
            "#
            ))
            .unwrap_or_else(|e| panic!("spelling {spelling}: {e}"));
        }
    }

    /// Unlike `kill()`, signalling leaves the child to be waited for -- which is
    /// what makes the TERM-then-KILL escalation expressible.
    #[test]
    fn signal_does_not_consume_the_child() {
        eval(
            r#"
            -- SIGCONT rather than a trapped SIGTERM: a shell that ignores TERM
            -- is surprisingly hard to arrange (macOS `sh` execs the loop away,
            -- so the trap is gone by the time the signal lands), while CONT is
            -- ignored by an already-running process on every platform. What is
            -- under test is that signalling leaves the child waitable, not the
            -- child's disposition.
            local child = proc.command({ "sleep", "30" }):spawn()
            child:signal("CONT")
            -- Not consumed: still ours to signal again, unlike after :kill().
            assert(type(child:id()) == "number", "child should still be live")
            local r = child:kill()
            assert(r.signal == 9, "expected SIGKILL, got " .. tostring(r.signal))
        "#,
        )
        .unwrap();
    }

    /// A normal exit must leave both signal fields absent, so `if r.signal then`
    /// is the test for how a child died.
    #[test]
    fn normal_exit_reports_no_signal() {
        eval(
            r#"
            local r = proc.command({ "sh", "-c", "exit 3" }):run()
            assert(r.status == 3, "expected status 3")
            assert(r.signal == nil, "a normal exit has no signal")
            assert(r.signal_name == nil, "a normal exit has no signal name")
        "#,
        )
        .unwrap();
    }

    /// A typo must not silently send nothing, which would look like a process
    /// ignoring the signal.
    #[test]
    fn unknown_signal_name_throws() {
        let err = eval(
            r#"
            local child = proc.command({ "sleep", "30" }):spawn()
            child:signal("TREM")
        "#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("unknown signal"), "got: {err}");
        assert!(
            err.to_string().contains("SIGTERM"),
            "should list names: {err}"
        );
    }

    #[test]
    fn signalling_a_reaped_child_throws() {
        let err = eval(
            r#"
            local child = proc.command({ "true" }):spawn()
            child:wait()
            child:signal("TERM")
        "#,
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("already been waited"),
            "got: {err}"
        );
    }

    /// `kill()` consumes the child, like `wait()`.
    #[test]
    fn waiting_after_kill_throws() {
        let err = eval(
            r#"
            local child = proc.command({ "sleep", "30" }):spawn()
            child:kill()
            child:wait()
        "#,
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("already been waited"),
            "got: {err}"
        );
    }

    /// Capture readers drain from spawn time, so a child that outproduces a pipe
    /// buffer must not block before `:wait()` runs.
    #[test]
    fn spawned_child_does_not_block_on_full_pipe() {
        eval(
            r#"
            local child = proc.command({ "sh", "-c", "yes p | head -c 500000" })
                :stdout(proc.string()):spawn()
            -- Unrelated work happens while the child is still writing.
            local other = proc.command({ "printf", "ok" })
                :stdout(proc.string()):run()
            assert(other.stdout == "ok", "blocked before wait")
            local r = child:wait()
            assert(#r.stdout == 500000, "got " .. #r.stdout .. " bytes")
        "#,
        )
        .unwrap();
    }

    /// `proc.ERR`/`proc.OUT` name one of *our* streams, which is what
    /// `Stdio::inherit()` cannot do -- it matches the slot, so a child's stdout
    /// went to our stdout no matter which alias was asked for. Checked by having
    /// the child's output land on a file this process's stderr is pointed at,
    /// since the test harness's own streams are not otherwise observable.
    #[test]
    fn named_inherit_targets_cross_the_streams() {
        use std::os::fd::{AsFd, AsRawFd};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stderr.log");
        let file = std::fs::File::create(&path).unwrap();

        // Point this process's stderr at the file for the duration, then put it
        // back -- `proc.ERR` duplicates fd 2, so this is what it will find.
        let saved = std::io::stderr().as_fd().try_clone_to_owned().unwrap();
        // SAFETY: both fds are open; `dup2` is the standard way to redirect a
        // stream, and the original is restored below.
        unsafe {
            libc::dup2(file.as_fd().as_raw_fd(), 2);
        }
        let result = eval(r#"proc.command({ "echo", "crossed" }):stdout(proc.ERR):run()"#);
        // SAFETY: `saved` is a live duplicate of the original stderr.
        unsafe {
            libc::dup2(saved.as_raw_fd(), 2);
        }
        result.unwrap();

        let written = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            written.trim(),
            "crossed",
            "the child's stdout should have reached this process's stderr"
        );
    }

    /// A dropped `:wait()` must hand the child back, intact and killable.
    ///
    /// It used not to: `add_async_method_mut` holds mlua's borrow for the whole
    /// future's life, so a `:wait()` abandoned mid-flight bricked the handle —
    /// every later method failed with `error mutably borrowing userdata`
    /// instead of doing its job. Now the borrow is scoped and a `WaitGuard`
    /// restores the child and its readers when the future is dropped, so the
    /// follow-up `:kill()` works.
    ///
    /// Driven with `tokio::time::timeout` rather than `task.timeout` because
    /// this module's `eval` registers only `proc` — and because the two differ
    /// in a way worth knowing: this drops the future at once, whereas
    /// `task.timeout` `abort()`s a `spawn_local` driver, which tokio drops only
    /// when it next polls. Under *that* path the guard still runs, just too
    /// late for the next statement, so a child abandoned by `task.timeout`
    /// reports "already been waited for" rather than recovering.
    #[test]
    fn dropped_wait_returns_the_child() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build test runtime");
        let local = tokio::task::LocalSet::new();
        local
            .block_on(&runtime, async {
                let lua = Lua::new();
                lua.globals().set("proc", module(&lua)?)?;
                let child: AnyUserData = lua
                    .load(r#"return proc.command({ "sleep", "30" }):spawn()"#)
                    .eval_async()
                    .await?;

                // Abandon a wait part-way through.
                let wait: mlua::Function = child.get("wait")?;
                let pending = wait.call_async::<mlua::Value>(child.clone());
                assert!(
                    tokio::time::timeout(std::time::Duration::from_millis(100), pending)
                        .await
                        .is_err(),
                    "the wait should still have been pending"
                );

                // The handle is whole again: the pid is readable...
                let id: mlua::Function = child.get("id")?;
                let pid: u32 = id.call_async(child.clone()).await?;
                assert!(pid > 0, "expected a live pid, got {pid}");

                // ...and the child is still killable, output and all.
                let kill: mlua::Function = child.get("kill")?;
                let result: Table = kill.call_async(child.clone()).await?;
                assert_eq!(
                    result.get::<Option<String>>("signal_name")?.as_deref(),
                    Some("SIGKILL"),
                    "the restored child should die of the kill"
                );
                Ok::<_, mlua::Error>(())
            })
            .expect("the dropped wait should have returned the child");
    }

    /// `:exec()` replaces the test process itself, which would abort the test
    /// runner -- so this only checks the up-front rejection of a redirect,
    /// which is the one part reachable without actually calling `execve`.
    #[test]
    fn exec_rejects_configured_redirects() {
        let err = eval(r#"proc.command("true"):stdout(proc.string()):exec()"#).unwrap_err();
        assert!(err.to_string().contains("does not support"), "got: {err}");

        let err = eval(r#"proc.command("true"):stderr(proc.string()):exec()"#).unwrap_err();
        assert!(err.to_string().contains("does not support"), "got: {err}");
    }

    #[test]
    fn interactive_reports_success() {
        eval(
            r#"
            local result = proc.command("true"):interactive()
            assert(result.success == true)
            assert(result.status == 0)
            assert(result.stdout == nil)
            assert(result.stderr == nil)
        "#,
        )
        .unwrap();
    }

    #[test]
    fn interactive_reports_nonzero_exit() {
        eval(
            r#"
            local result = proc.command({ "sh", "-c", "exit 7" }):interactive()
            assert(result.success == false)
            assert(result.status == 7)
        "#,
        )
        .unwrap();
    }

    /// A stream can be redirected explicitly under `:interactive()` (the fzf
    /// case: the selection is captured, the UI stays on the terminal).
    #[test]
    fn interactive_can_capture_stdout() {
        eval(
            r#"
            local r = proc.command({ "echo", "picked" }):stdout(proc.string()):interactive()
            assert(r.stdout == "picked\n", "got " .. tostring(r.stdout))
        "#,
        )
        .unwrap();
    }

    /// `:exec()` accepts only INHERIT (explicit or default).
    #[test]
    fn exec_rejects_anything_but_inherit() {
        let err = eval(r#"proc.command("true"):stdin("x"):exec()"#).unwrap_err();
        assert!(err.to_string().contains("does not support"), "got: {err}");
        let err = eval(r#"proc.command("true"):stdout(proc.NULL):exec()"#).unwrap_err();
        assert!(err.to_string().contains("does not support"), "got: {err}");
        // Explicit INHERIT passes the check and fails later, at exec itself.
        let err = eval(r#"proc.command("de-no-such-binary-xyz"):stdout(proc.INHERIT):exec()"#)
            .unwrap_err();
        assert!(err.to_string().contains("failed to exec"), "got: {err}");
    }

    /// A failed exec must not leave the working directory changed.
    #[test]
    fn failed_exec_restores_the_directory() {
        eval(
            r#"
            local before = proc.command("pwd"):stdout(proc.string()):run().stdout
            local ok = pcall(function()
                proc.command("de-no-such-binary-xyz"):current_dir("/"):exec()
            end)
            assert(not ok)
            local after = proc.command("pwd"):stdout(proc.string()):run().stdout
            assert(before == after, "cwd changed")
        "#,
        )
        .unwrap();
    }

    #[test]
    fn self_alias_raises() {
        let err = eval(r#"proc.command("true"):stdout(proc.STDOUT)"#).unwrap_err();
        assert!(err.to_string().contains("onto itself"), "got: {err}");
        let err = eval(r#"proc.command("true"):stderr(proc.STDERR)"#).unwrap_err();
        assert!(err.to_string().contains("onto itself"), "got: {err}");
    }

    /// `2>&1` works with stdout configured first; the alias shares the capture,
    /// so the merged text is under both fields.
    #[test]
    fn stderr_alias_follows_stdout_capture() {
        eval(
            r#"
            local r = proc.command({ "sh", "-c", "echo o; echo e >&2" })
                :stdout(proc.string()):stderr(proc.STDOUT):run()
            assert(r.stdout:find("o") and r.stdout:find("e"), "got " .. tostring(r.stdout))
        "#,
        )
        .unwrap();
    }

    #[test]
    fn stdin_from_string_buffer_null_and_file() {
        eval(
            r#"
            local r = proc.command("cat"):stdin("from string"):stdout(proc.string()):run()
            assert(r.stdout == "from string", "got " .. tostring(r.stdout))

            r = proc.command("cat"):stdin(buffer.fromstring("from buffer"))
                :stdout(proc.string()):run()
            assert(r.stdout == "from buffer")

            r = proc.command("cat"):stdin(proc.NULL):stdout(proc.string()):run()
            assert(r.stdout == "")
        "#,
        )
        .unwrap();

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("in.txt");
        std::fs::write(&path, "from file").unwrap();
        eval(&format!(
            r#"
            local r = proc.command("cat"):stdin(proc.file("{}")):stdout(proc.string()):run()
            assert(r.stdout == "from file", "got " .. tostring(r.stdout))
        "#,
            path.display()
        ))
        .unwrap();
    }

    /// Input larger than a pipe buffer must not deadlock against the child.
    #[test]
    fn large_stdin_does_not_deadlock() {
        eval(
            r#"
            local input = string.rep("x", 500000)
            local r = proc.command("cat"):stdin(input):stdout(proc.string()):run()
            assert(#r.stdout == 500000, "got " .. #r.stdout)
        "#,
        )
        .unwrap();
    }

    #[test]
    fn stdin_rejects_a_capture_handle() {
        let err = eval(r#"proc.command("cat"):stdin(proc.string())"#).unwrap_err();
        assert!(err.to_string().contains("stdin source"), "got: {err}");
    }

    #[test]
    fn file_append_mode_appends() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.log");
        std::fs::write(&path, "first\n").unwrap();
        eval(&format!(
            r#"
            proc.command({{ "echo", "second" }}):stdout(proc.file("{0}", {{ append = true }})):run()
        "#,
            path.display()
        ))
        .unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "first\nsecond\n");
    }

    /// The same-file check sees through a symlink (same dev/inode).
    #[test]
    fn same_file_via_symlink_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real.log");
        let link = dir.path().join("link.log");
        std::fs::write(&real, "").unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let err = eval(&format!(
            r#"proc.command("echo"):stdout(proc.file("{}")):stderr(proc.file("{}")):run()"#,
            real.display(),
            link.display()
        ))
        .unwrap_err();
        assert!(err.to_string().contains("separate handles"), "got: {err}");
    }

    #[test]
    fn envs_and_env_remove() {
        eval(
            r#"
            local r = proc.command({ "sh", "-c", "echo $A-$B-${C:-unset}" })
                :envs({ A = "1", B = "2" }):env("C", "3"):env_remove("C")
                :stdout(proc.string()):run()
            assert(r.stdout == "1-2-unset\n", "got " .. tostring(r.stdout))
        "#,
        )
        .unwrap();
    }

    /// Luau strings are bytes, so arguments need not be UTF-8.
    #[test]
    fn non_utf8_arguments_reach_the_child() {
        eval(
            r#"
            local r = proc.command("printf"):args({ "%s", "\255\254" })
                :stdout(proc.buffer()):run()
            assert(buffer.len(r.stdout) == 2 and buffer.readu8(r.stdout, 0) == 255)
        "#,
        )
        .unwrap();
    }

    #[test]
    fn try_wait_is_nil_until_exit() {
        eval(
            r#"
            local child = proc.command({ "sleep", "30" }):spawn()
            assert(child:try_wait() == nil, "still running")
            child:signal("TERM")
            local r
            for _ = 1, 200 do
                r = child:try_wait()
                if r then break end
                proc.command({ "sleep", "0.05" }):run()
            end
            assert(r and r.signal_name == "SIGTERM", "never reaped")
            assert(not pcall(function() return child:id() end), "consumed")
        "#,
        )
        .unwrap();
    }

    /// An abandoned `:run()` takes its child with it (`kill_on_drop`).
    #[test]
    fn abandoned_run_kills_its_child() {
        let dir = tempfile::tempdir().unwrap();
        let pidfile = dir.path().join("pid");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let local = tokio::task::LocalSet::new();
        local.block_on(&runtime, async {
            let lua = Lua::new();
            lua.globals().set("proc", module(&lua).unwrap()).unwrap();
            let script = format!(
                r#"proc.command({{ "sh", "-c", "echo $$ > {}; exec sleep 30" }}):run()"#,
                pidfile.display()
            );
            let run = lua.load(script).exec_async();
            let _ = tokio::time::timeout(std::time::Duration::from_millis(500), run).await;
            // Let the runtime's reaper collect the killed child (a zombie still
            // answers signal 0).
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        });
        let pid: i32 = std::fs::read_to_string(&pidfile)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        // SAFETY: signal 0 only probes for existence.
        let alive = unsafe { libc::kill(pid, 0) } == 0;
        assert!(!alive, "the abandoned child should have been killed");
    }

    #[test]
    fn trap_rejects_signals_the_os_refuses() {
        for sig in ["11", "4", "999"] {
            let err = eval(&format!("proc.trap({sig}, function() end)")).unwrap_err();
            assert!(err.to_string().contains("cannot trap"), "{sig}: {err}");
        }
    }

    #[test]
    fn signal_numbers_out_of_range_are_rejected() {
        let err = eval(r#"proc.command("true"):spawn():signal(5000000000)"#).unwrap_err();
        assert!(err.to_string().contains("out of range"), "got: {err}");
    }

    #[test]
    fn rejects_non_redirect_target() {
        let err = eval(r#"proc.command("echo"):stdout("capture")"#).unwrap_err();
        assert!(
            err.to_string().contains("expected a redirect target"),
            "got: {err}"
        );
    }

    // -----------------------------------------------------------------------
    // proc.trap / proc.untrap
    //
    // Tests that actually deliver a signal live in `ito/tests/proc_trap.rs`
    // instead of here: `kill(getpid(), sig)` targets the whole *process*, and
    // Rust runs a file's `#[test]`s as threads within one process (sharing
    // one pid), so two such tests using the same signal number can observe
    // each other's deliveries. Cargo gives each `tests/*.rs` file its own
    // binary and so its own process, which is what makes the signal land
    // only where that file's own test sent it. The two tests below send no
    // signal, so that hazard does not apply to them.
    // -----------------------------------------------------------------------

    #[test]
    fn kill_and_stop_cannot_be_trapped() {
        let err = eval("proc.trap(proc.SIGKILL, function() end)").unwrap_err();
        assert!(err.to_string().contains("cannot be trapped"), "got: {err}");
        let err = eval("proc.trap(proc.SIGSTOP, function() end)").unwrap_err();
        assert!(err.to_string().contains("cannot be trapped"), "got: {err}");
    }

    #[test]
    fn untrap_of_an_untrapped_signal_is_a_no_op() {
        eval("proc.untrap(proc.SIGTERM)")
            .expect("untrapping a never-trapped signal must not raise");
    }
}
