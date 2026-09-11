//! The phases of a run, for binaries to compose.
//!
//! A run is startup, then an engine, then evaluation, then output. What differs
//! between the binaries is *one value*: the [`Config`] describing what the
//! script may reach. Everything else is the same code on every path.
//!
//! ```ignore
//! // bin/ito.rs -- the whole binary, minus the early-exit `let else`s
//! let startup = Startup::begin(cli.common)?;          // -v, -C
//! let script = startup.script()?;                     // SCRIPT
//! let engine = Engine::build(&script, Config::ito(argv))?;
//! engine.eval(&script).await?;                        // chunk, then main()
//! ```
//!
//! `ied` and `ito` differ only in which [`Config`] constructor they name; `ied`
//! additionally ends with `emit_reports` + `flush_writes`.
//!
//! Two orderings are load-bearing and both are enforced rather than documented:
//! registration order lives inside [`Engine::build`], which is the only way to
//! obtain an `Engine`, so no caller can get it wrong; and the task drain must
//! precede a flush (a task's buffered writes are only real once it has
//! finished), which is [`Engine::eval`]'s own doing.

use std::io;
use std::path::{Path, PathBuf};

use crate::cli::{CommonArgs, ReportArgs};

use crate::scripting::{self as sl, Argv, Vfs};

// ---------------------------------------------------------------------------
// Errors and outcomes
// ---------------------------------------------------------------------------

/// An error from a run, carrying the process exit code to use.
///
/// Exit codes: `2` is the generic failure (usage, environment, script load,
/// output/flush) — not the script's fault. `3` is reserved for failures
/// attributable to the script itself: a parse/compile error or a runtime error.
/// `0`/`1` are success (see [`Outcome`]). There is no exit code reserved for
/// death by signal: a signal nothing trapped keeps its default disposition
/// (see `main_with`), so the process ends the way it always would and this
/// type is never consulted for it; `proc.exit` bypasses it too, being a
/// direct `std::process::exit`.
pub struct RunError {
    pub code: i32,
    pub message: String,
}

impl RunError {
    /// A failure attributable to the script (parse or runtime): exit `3`.
    pub fn script(message: String) -> Self {
        Self { code: 3, message }
    }
}

/// Generic failures (`?` on a `String`/`Result<_, String>`) default to exit `2`.
impl From<String> for RunError {
    fn from(message: String) -> Self {
        Self { code: 2, message }
    }
}

/// A failure registering the scripting engine itself (not the script's fault):
/// exit `2`.
impl From<mlua::Error> for RunError {
    fn from(e: mlua::Error) -> Self {
        Self {
            code: 2,
            message: e.to_string(),
        }
    }
}

/// What a run reports back for exit-code purposes.
#[derive(Default, Clone, Copy)]
pub struct Outcome {
    /// `--exit-code` was requested.
    pub exit_code: bool,
    /// The run produced at least one real change (a buffered write differing
    /// from disk).
    pub changed: bool,
}

impl Outcome {
    /// The process exit code this outcome implies.
    pub fn code(self) -> i32 {
        // `--exit-code` borrows git's convention: 1 means "there were changes",
        // which is a success, not a failure.
        if self.exit_code && self.changed { 1 } else { 0 }
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Run `body` on a runtime and exit with its outcome. Never returns.
///
/// Every binary funnels through here so the runtime is built the same way.
/// Single-threaded on purpose: mlua's `Lua` is `!Send`, and I/O-bound work needs
/// concurrency rather than parallelism (see `scripting::task`). The runtime
/// exists even for binaries with no async modules, because evaluating a chunk
/// with `call_async` requires one.
///
/// There is no built-in reaction to SIGINT (or any other signal) here: under
/// `ito`, `proc.trap` is the one place a script opts into handling a
/// signal, and a signal nothing trapped keeps its default disposition (SIGINT
/// terminates, as it always did) rather than this function racing the body
/// against it. `ied` registers no traps and sees the same default behaviour
/// it always did.
pub fn main_with<F>(body: F) -> !
where
    F: AsyncFnOnce() -> Result<Outcome, RunError>,
{
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("failed to start async runtime: {e}");
            std::process::exit(2);
        }
    };
    let local = tokio::task::LocalSet::new();
    match local.block_on(&runtime, body()) {
        Ok(outcome) => std::process::exit(outcome.code()),
        Err(e) => {
            // An empty message means the failure already said its piece on the
            // stream it chose -- `cli.parse()` printing help or a usage block.
            // Logging "" here would add a bare `ERROR` line under it.
            if !e.message.is_empty() {
                tracing::error!("{}", e.message);
            }
            std::process::exit(e.code);
        }
    }
}

// ---------------------------------------------------------------------------
// Phase 1: startup
// ---------------------------------------------------------------------------

/// Everything settled before a script exists: logging, the working directory,
/// and the flags that print something and exit.
///
/// Holds the invocation cwd because script paths resolve against it — captured
/// *before* `-C` moves us, since a relative `SCRIPT` (including the
/// `./script.luau` a shebang exec hands us) names a file where the user ran the
/// command, not where `-C` lands.
pub struct Startup {
    common: CommonArgs,
    invocation_dir: PathBuf,
    /// The `-C`-resolved working directory: the `fs` root under `ied`, and
    /// the directory relative paths resolve against under `ito`, whose `fs` has
    /// no root.
    pub working_dir: PathBuf,
}

/// Handle the flags that print something and exit, if any were given.
///
/// `--doc` and `--completion` are not run options: they answer a question about
/// the tool and stop. Both end the process here rather than returning a "nothing
/// to do" value for every caller to thread onwards — printing help *is* a
/// terminal action, and `--doc` already pages through `less` and waits.
///
/// `command` is the binary's own clap command, so `--completion` generates from
/// that binary's real flag set and this stays ignorant of which one it serves.
///
/// Call after `init_tracing`, so a failure has somewhere to be reported.
pub fn handle_info_flags(common: &CommonArgs, command: clap::Command) -> Result<(), RunError> {
    if let Some(topic) = &common.doc {
        let topic = if topic.is_empty() {
            None
        } else {
            Some(topic.as_str())
        };
        // The program name picks the default chapter, so `ied --doc` opens on
        // `ied` while still carrying every other topic.
        crate::help::doc(
            command.get_name(),
            topic,
            common.doc_plain,
            common.doc_no_pager,
        )?;
        std::process::exit(0);
    }

    if let Some(shell) = common.completion {
        let mut command = command;
        let name = command.get_name().to_string();
        clap_complete::generate(
            clap_complete::Shell::from(shell),
            &mut command,
            name,
            &mut std::io::stdout(),
        );
        std::process::exit(0);
    }

    Ok(())
}

impl Startup {
    /// Apply `-C` and capture the two directories a run resolves paths against.
    ///
    /// Assumes logging is up and the info flags are handled (see
    /// [`init_tracing`], [`handle_info_flags`]).
    pub fn begin(common: CommonArgs) -> Result<Self, RunError> {
        let invocation_dir = std::env::current_dir().map_err(|e| format!("getcwd: {e}"))?;

        // `-C` is make-style: each is relative to the previous, an absolute path
        // resets. Applied before anything reads the working directory, and after
        // `invocation_dir` is captured.
        for dir in &common.directory {
            std::env::set_current_dir(dir).map_err(|e| format!("-C {}: {e}", dir.display()))?;
            let dir = dir.display();
            tracing::debug!(%dir, "Changed CWD");
        }
        let working_dir = std::env::current_dir().map_err(|e| format!("getcwd: {e}"))?;

        Ok(Self {
            common,
            invocation_dir,
            working_dir,
        })
    }

    /// Resolve the script to run, or list what is available.
    ///
    /// Returns `None` when there is no `SCRIPT` and nothing to run — the bare
    /// invocation, which prints the scripts it can find and exits.
    pub fn script(&self) -> Result<Option<Script>, RunError> {
        let Some(arg) = self.common.script() else {
            tracing::debug!("No script specified");
            list_available_scripts(&self.working_dir, &self.program_name());
            return Ok(None);
        };

        // Resolved against the invocation cwd, before `-C` took effect, so the
        // absolute path survives the chdir.
        let path = self.invocation_dir.join(arg);
        if !path.is_file() {
            return Err(format!("no script at '{}'", path.display()).into());
        }
        Ok(Some(Script::file(&path, &self.working_dir)?))
    }

    /// The name this binary was invoked as, for messages that suggest commands.
    fn program_name(&self) -> String {
        std::env::args()
            .next()
            .as_deref()
            .map(Path::new)
            .and_then(Path::file_name)
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "ito".to_string())
    }
}

// ---------------------------------------------------------------------------
// Phase 2: the script
// ---------------------------------------------------------------------------

/// A loaded script, with the two roots that govern it resolved.
pub struct Script {
    /// How this script names itself in errors: its path as given.
    label: String,
    /// The source, shebang commented out.
    source: String,
    /// The `require` confinement boundary: the script's own directory.
    require_root: PathBuf,
    /// The script file itself, absolute and symlink-resolved (`fs.script()`).
    abs_path: PathBuf,
    /// The `-C`-resolved working directory: the `fs` root where `fs` is rooted,
    /// and what relative paths resolve against where it is not.
    vfs_root: PathBuf,
    /// Chunk name, which `require`'s caller-detection resolves against.
    chunk_name: String,
}

impl Script {
    /// A script file, which anchors the require root to its own directory.
    fn file(path: &Path, vfs_root: &Path) -> Result<Self, RunError> {
        let source = std::fs::read_to_string(path)
            .map_err(|e| format!("script read error {}: {e}", path.display()))?;
        // Follow symlinks first: a script installed as `~/.local/bin/<name>`
        // pointing at `~/scripts/<name>.luau` keeps its modules next to the
        // *real* file, not next to the launcher. Anchoring on the link would
        // confine `require` to the bin directory, where nothing it imports is.
        let abs = std::fs::canonicalize(abs_from(vfs_root, path))
            .unwrap_or_else(|_| abs_from(vfs_root, path));
        let require_root = abs
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| vfs_root.to_path_buf());
        // `require` only recognizes chunks named with a leading `@` (Luau's
        // `is_require_allowed` convention). The requirer's automatic first
        // `to_parent` moves from this chunk to its directory, so the name must be
        // a path *inside* the require root. The extension is stripped so the name
        // need not resolve back to the file itself.
        let chunk_name = format!("@{}", abs.with_extension("").display());
        Ok(Self {
            label: path.display().to_string(),
            source: comment_out_shebang(source),
            require_root,
            abs_path: abs,
            vfs_root: vfs_root.to_path_buf(),
            chunk_name,
        })
    }

    /// The `fs` root.
    pub fn vfs_root(&self) -> &Path {
        &self.vfs_root
    }

    /// The script file, absolute and symlink-resolved.
    pub fn abs_path(&self) -> &Path {
        &self.abs_path
    }

    /// The `require` confinement boundary: the script's own directory.
    pub fn require_root(&self) -> &Path {
        &self.require_root
    }
}

// ---------------------------------------------------------------------------
// Phase 3: the engine
// ---------------------------------------------------------------------------

/// Drop `math.random`/`math.randomseed`.
///
/// Both live on the `math` table rather than in globals, so this has to reach
/// into it — and it only works before `seal`, which freezes that table.
fn remove_random(lua: &mlua::Lua) -> Result<(), RunError> {
    let math: mlua::Table = lua.globals().get("math")?;
    math.raw_remove("random")?;
    math.raw_remove("randomseed")?;
    Ok(())
}

/// Add `os.getenv` to the stdlib's `os` table.
///
/// Luau drops `getenv` from its sandboxed `os` (which is left with
/// clock/date/difftime/time), so this restores the Lua 5.1 name rather than
/// inventing one — a script reaching for the environment reaches where it
/// expects to. Read-only on purpose: there is no setter, because a per-command
/// `proc`'s `:env()` covers passing a variable to a child without mutating
/// process-wide state that every later task would then observe (`ssh` has no
/// `:env()`; sshd owns the remote environment).
///
/// Like [`remove_random`], this reaches into a stdlib table and so only works
/// before `seal` freezes it.
///
/// Returns `nil` for an unset variable and `""` for one set to the empty
/// string, keeping the two distinguishable as `os.getenv` always has. A value
/// that is not valid UTF-8 reads as `nil`.
fn add_getenv(lua: &mlua::Lua) -> Result<(), RunError> {
    let os: mlua::Table = lua.globals().get("os")?;
    let getenv = lua.create_function(|_, name: String| Ok(std::env::var(name).ok()))?;
    os.raw_set("getenv", getenv)?;
    Ok(())
}

/// The shape of `fs`, and with it what `j2`'s `{% include %}` may read.
///
/// An include is a file read, so it is granted exactly the access `fs` got —
/// which is why this is one choice rather than two that could disagree.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Fs {
    /// Rooted at the working directory, escapes rejected. `writes` decides
    /// whether the write half exists (see [`sl::vfs::Writes`]).
    Rooted { writes: sl::vfs::Writes },
    /// The host filesystem: no root, writes land immediately.
    Host,
}

/// What a script may reach: the whole of what differs between the binaries.
///
/// Everything else about a run is identical on every path, so this struct *is*
/// the capability boundary. Written as data rather than a call sequence so a
/// binary's surface can be read (and diffed) in one place — see the
/// per-binary constructors below.
///
/// `argv` is here for uniformity, not as a capability: what a script is
/// *handed* is the same everywhere.
pub struct Config {
    pub fs: Fs,
    /// `proc`/`ssh`/`aws`/`task`, plus `os.getenv`.
    pub host_io: bool,
    /// Keep the stdlib's ambient state (`os`, `math.random`).
    ///
    /// Deliberately independent of `host_io` even though today every binary
    /// sets them together: they are separate claims, and a run whose output is
    /// a report on what it *would* do needs determinism for a reason that has
    /// nothing to do with whether it can spawn a process.
    pub ambient: bool,
    /// Whether `print` writes. False under `-q`, and under the reports that own
    /// stdout; the call is then a silent no-op.
    pub print: bool,
    /// The script's own arguments, everything after `SCRIPT`.
    pub argv: Argv,
}

impl Config {
    /// `ied` — buffered edits. A report owns stdout, so `-d`/`-l` imply `-q`:
    /// `print` is silenced rather than landing inside the diff or file list.
    ///
    /// A preview is only worth anything if the run it previews computes the same
    /// change set, so nothing ambient may feed into one.
    pub fn ied(quiet: bool, report: &ReportArgs, argv: Argv) -> Self {
        Self {
            fs: Fs::Rooted {
                writes: sl::vfs::Writes::Buffered,
            },
            host_io: false,
            ambient: false,
            print: !(quiet || report.diff || report.changed),
            argv,
        }
    }

    /// `ito` — the full toolset. Writes land immediately, since a subprocess
    /// would not see a buffered one.
    ///
    /// Keeps the ambient state: a binary that already spawns processes and opens
    /// sockets would gain nothing from being denied the clock.
    pub fn ito(argv: Argv) -> Self {
        Self {
            fs: Fs::Host,
            host_io: true,
            ambient: true,
            print: true,
            argv,
        }
    }
}

/// A Lua state with its surface registered and its globals frozen.
///
/// The only way to obtain one is [`Self::build`], so every ordering constraint
/// among the registrations is settled in there rather than being a rule callers
/// have to follow.
pub struct Engine {
    lua: mlua::Lua,
    /// The rooted VFS, when `fs` was rooted. `None` under the host filesystem
    /// or no `fs` at all — neither has a pending set to report on.
    vfs: Option<Vfs>,
    /// The drain handle outstanding tasks need; `Some` when `host_io` was set.
    tasks: Option<sl::task::Tasks>,
}

impl Engine {
    /// Build the engine `config` describes, sealed and ready to evaluate.
    ///
    /// The registration order here is the whole of what used to be a builder's
    /// worth of sequencing rules. Three of them are load-bearing:
    ///
    /// - `proc` carries the redirect targets (`proc.string()`, `proc.file(..)`)
    ///   that its own and `ssh`'s command builders both take, so it comes first
    ///   of those two.
    /// - `j2` comes after the `fs` decision, because an include is a file read
    ///   and must be granted exactly the access `fs` got. Reading it off
    ///   `config.fs` is what makes that automatic rather than a rule.
    /// - `sandbox(true)` comes last: it freezes the stdlib tables, and
    ///   `remove_random`/`add_getenv` reach *into* those tables.
    pub fn build(script: &Script, config: Config) -> Result<Self, RunError> {
        let lua = mlua::Lua::new();

        // Replace Luau's default `require` (an unconfined `FsRequirer`) with
        // ito's own, confined to the script's directory tree.
        sl::require::register(&lua, script.require_root.clone())?;

        // The modules every binary has: formats, paths, regex, logging, string
        // methods, the `ito` module, and the script's own `argv`.
        //
        // No `debug_print`/`MAIN`/`ITO_VERSION`: `log.debug` already logs
        // through tracing, a `main` function already marks the entry point (the
        // runner calls it when present), and `ito.version_check` already
        // asserts compatibility -- which is the only thing a script did with
        // the version.
        sl::null::register(&lua)?;
        sl::codec::register_array(&lua)?;
        sl::json::register(&lua)?;
        sl::path::register(&lua)?;
        sl::re::register(&lua)?;
        sl::toml::register(&lua)?;
        sl::yaml::register(&lua)?;
        sl::hcl::register(&lua)?;
        sl::time::register(&lua)?;
        // Hashing a value is a pure byte-to-digest conversion, so it goes
        // everywhere; only the `_file` forms read the filesystem, and those are
        // added under `host_io` below.
        sl::hash::register(&lua)?;
        sl::encoding::register(&lua)?;
        // `ansible.vault` is a pure cipher over its arguments, like `hash`, so
        // it goes everywhere too.
        sl::ansible::register(&lua)?;
        sl::ito::register(&lua, env!("CARGO_PKG_VERSION"))?;
        sl::log::register(&lua)?;
        // `print`: vanilla, to stdout, in every binary. The `io` handles are
        // host-only and join under `host_io`.
        sl::print::register(&lua, config.print)?;
        // `fmt`: styles, the colour switch (`fmt.colors`), text helpers and
        // tables; pure, so every binary has it. Also the `string` sugar methods.
        sl::fmt::ansi::reset();
        sl::print::reset();
        sl::fmt::register(&lua)?;

        // `cli` reads the command line as given, so it takes `argv` before
        // `argv::register` hands ownership to the global -- a script may
        // reassign that global, and the declared interface should describe the
        // invocation rather than whatever the script last stored there.
        //
        // It writes to stdout, which `ied`'s reports otherwise own, and still
        // belongs there: what it prints is *terminal*, so a run that prints
        // help never reaches `emit_reports` and cannot interleave with it.
        // Nothing in it is capability-bearing.
        sl::cli::register(&lua, &config.argv.0)?;
        sl::argv::register(&lua, config.argv)?;

        // `fs`, in whichever shape was asked for. A rooted VFS keeps a handle
        // for the report/flush steps; the host filesystem has no root to hold.
        //
        // Under `Fs::Host` a relative path still resolves against the `-C`
        // working directory, since `Startup::begin` already `chdir`ed there, so
        // every path a rooted `fs` would have accepted names the same file.
        // What changes is that `..` and absolute paths reach outside instead of
        // erroring -- the honest model for a binary that can already spawn a
        // subprocess to do the same thing (see `sl::rfs`).
        let vfs = match config.fs {
            Fs::Rooted { writes } => {
                let vfs = Vfs::new(script.vfs_root())?;
                sl::vfs::register_with(&lua, &vfs, writes)?;
                Some(vfs)
            }
            Fs::Host => {
                sl::rfs::register(&lua)?;
                None
            }
        };

        // The modules that reach outside the sandbox, plus `task` to overlap
        // them. `os.getenv` joins them here rather than in the core: reading
        // the ambient environment belongs with the surface that can already
        // reach outside it.
        let tasks = if config.host_io {
            add_getenv(&lua)?;
            // `ito.require_root()`/`ito.require_dir()` join these for the same
            // reason `os.getenv` does: they report host locations, which is
            // only a script's business where its `fs` is the host filesystem
            // too.
            sl::ito::register_host_paths(
                &lua,
                script.require_root().to_path_buf(),
                script.abs_path().to_path_buf(),
            )?;
            // `ito.slug()` joins them: it's random, the same line
            // `remove_random` draws around `math.random` below.
            sl::ito::register_host_slug(&lua)?;
            // The `hash.*_file` forms, for the reason `fs` itself is host-only
            // here: naming a path to read is a filesystem read, and hashing a
            // file outside a rooted `fs`'s root would leak its contents past
            // the containment that root exists to provide. The pure
            // string/buffer forms were registered with the core, above.
            sl::hash::register_host_files(&lua)?;
            // `ansible.vault.load_password_file` (reads or executes a path) and
            // `ansible.vault.encrypt` (random) join `hash.*_file` here rather
            // than the pure half registered with the core.
            sl::ansible::register_host(&lua)?;
            // `io.stdout`/`io.stderr`/`io.stdin` and the default-handle calls:
            // raw stream access, host-only -- `ied`'s stdout is shared with its
            // report, so it does not hand a script a second kind of writer
            // (nor stdin, nor stderr).
            sl::io::register(&lua)?;
            // The clock-reading and local-zone half of `time`, host-only for
            // the reason `os.getenv` is: each answer depends on ambient state
            // the caller never passed, which an `ied --diff` preview must not
            // read or it could disagree with the `--write` after it.
            sl::time::register_host_time(&lua)?;
            // `fmt.is_tty()`, and with it the right for `fmt.colors(nil)` to
            // detect: both read ambient state (terminal-ness, CLICOLOR), so the
            // styles stay pure under `ied`.
            sl::fmt::register_host(&lua)?;
            sl::proc::register(&lua)?;
            // `proc.pid()`: ambient state, the same line this block draws.
            sl::ito::register_host_pid(&lua)?;
            sl::ssh::register(&lua)?;
            sl::aws::register(&lua)?;
            // `ui.prompt`/`ui.progress`: terminal interaction is ambient
            // state a query's return value or an edit preview must not
            // depend on, the same line `proc`/`ssh` draw.
            sl::ui::register(&lua)?;
            Some(sl::task::register(&lua)?)
        } else {
            None
        };

        // Ambient state (`os`, `math.random`) hands a script input the caller
        // never gave it and cannot see, so two runs of one script can disagree.
        // Where the output is a report on what a run *would* do that is a
        // correctness problem rather than a taste one: it lets an `ied --diff`
        // preview differ from the `--write` that follows it, which is the one
        // thing that mode promises.
        //
        // Absent rather than stubbed, matching `fs`: `if os then` is a feature
        // test, and a script that reads the clock fails where it does so.
        if !config.ambient {
            // `os` is clock/date/difftime/time -- no filesystem half to keep.
            lua.globals().raw_remove("os")?;
            remove_random(&lua)?;
        }

        // `j2` now that the `fs` decision is known -- an include is a file read,
        // so it gets exactly the access `fs` got.
        match config.fs {
            Fs::Rooted { .. } => sl::j2::register(
                &lua,
                vfs.as_ref().expect("a rooted fs built a VFS just above"),
            )?,
            Fs::Host => sl::j2::register_host(&lua)?,
        }

        // Luau sandbox mode makes the stdlib tables read-only and proxies
        // further writes into a per-thread environment, so the entry script and
        // its imports cannot tamper with the stdlib or each other.
        lua.sandbox(true)?;

        Ok(Self { lua, vfs, tasks })
    }
}

// ---------------------------------------------------------------------------
// Phase 4: evaluation
// ---------------------------------------------------------------------------

impl Engine {
    /// Evaluate the script and return its value.
    ///
    /// `call_async` rather than `exec_async`: the script may call async stdlib
    /// functions and the host has to drive those futures, and evaluating rather
    /// than merely executing keeps the return value.
    ///
    /// A top-level `main` function, if defined, is called afterwards and its
    /// value wins — a script written that way puts its answer in `main`, and the
    /// chunk itself returns nothing.
    pub async fn eval(&self, script: &Script) -> Result<mlua::Value, RunError> {
        let result = self.eval_inner(script).await;
        // Stop spinners and settle the display, whether or not the script failed.
        sl::ui::progress::teardown();
        result
    }

    async fn eval_inner(&self, script: &Script) -> Result<mlua::Value, RunError> {
        // A `cli.parse()` that printed help or a usage error unwinds through the
        // script as an error, but it is not one: the text is already on the
        // right stream, and the only thing left is the exit code. Recognised
        // here rather than at each call site so no path can turn it into a
        // spurious "script error".
        let fail = |e: mlua::Error| match sl::cli::exit_code(&e) {
            Some(code) => RunError {
                code,
                message: String::new(),
            },
            None => RunError::script(format!("script error in {}: {e}", script.label)),
        };

        let chunk_value: mlua::Value = self
            .lua
            .load(&script.source)
            .set_name(script.chunk_name.clone())
            .call_async(())
            .await
            .map_err(fail)?;

        let answer = match self.lua.globals().get::<mlua::Function>("main") {
            Ok(main_fn) => main_fn.call_async::<mlua::Value>(()).await.map_err(fail)?,
            Err(_) => chunk_value,
        };

        // Every spawned task runs to completion before anything downstream looks
        // at the filesystem: a task may exist purely for its `fs.write` side
        // effects, and those are only real once it has finished. A task that
        // failed without ever being awaited is the script's fault, same as an
        // error on the main thread.
        if let Some(tasks) = &self.tasks {
            tasks.drain().await.map_err(fail)?;
        }

        if sl::print::pipe_broken() {
            // The script may have `pcall`ed the write error away; the run was
            // still cut short.
            return Err(RunError {
                code: sl::print::SIGPIPE_EXIT,
                message: String::new(),
            });
        }
        Ok(answer)
    }

    /// The rooted VFS, for the report and flush steps.
    ///
    /// `None` where `fs` is the host filesystem or absent — neither has a
    /// pending set to report on, and only `ied` (which always builds one) calls
    /// this.
    pub fn vfs(&self) -> Option<&Vfs> {
        self.vfs.as_ref()
    }
}

// ---------------------------------------------------------------------------
// Phase 5: output
// ---------------------------------------------------------------------------

/// Print the `--diff`/`--list-changed` report and compute the run's outcome.
///
/// Both report against the *buffered* writes, so they are meaningful whether or
/// not the caller goes on to flush. clap enforces that they are mutually
/// exclusive. Both imply `-q`, so the script's `print` is already silenced and
/// the report owns stdout.
pub fn emit_reports(vfs: &Vfs, report: &ReportArgs) -> Result<Outcome, RunError> {
    // The set of real changes -- buffered writes differing from disk -- is the
    // shared basis for the listing and the exit status.
    let changed = vfs.changed().map_err(|e| format!("changed: {e}"))?;

    use std::io::Write;
    // A reader that went away (`ied -d | head`) is the SIGPIPE exit, not a failure.
    let werr = |what: &str, e: std::io::Error| {
        if e.kind() == std::io::ErrorKind::BrokenPipe {
            RunError {
                code: sl::print::SIGPIPE_EXIT,
                message: String::new(),
            }
        } else {
            RunError::from(format!("write {what}: {e}"))
        }
    };
    let mut out = std::io::stdout().lock();
    if report.changed {
        for path in &changed {
            writeln!(out, "{path}").map_err(|e| werr("changed", e))?;
        }
    }
    if report.diff {
        write!(out, "{}", vfs.diff().map_err(|e| format!("diff: {e}"))?)
            .map_err(|e| werr("diff", e))?;
    }

    Ok(Outcome {
        exit_code: report.exit_code,
        changed: !changed.is_empty(),
    })
}

/// Apply the buffered writes, if the run asked for them (`-w`).
///
/// Writing is opt-in, so the default path here is to drop the buffer on the
/// floor: the change set was computed only to be reported on.
///
/// Must come after the task drain (see [`Engine::eval`]), or a task's writes are
/// lost.
pub fn flush_writes(vfs: &Vfs, report: &ReportArgs) -> Result<(), RunError> {
    if report.write {
        vfs.flush()?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Install the log subscriber at the verbosity `-v` asked for.
///
/// Call it straight after parsing and before any check that can fail: an error
/// raised earlier has nowhere to be written, and a usage mistake would exit
/// non-zero in silence.
///
/// Records are written through `ui.progress`'s pause, so a log line never
/// garbles a live bar. Level labels are coloured only when stderr is a
/// terminal and `NO_COLOR` is unset.
pub fn init_tracing(verbosity: u8) {
    use std::io::IsTerminal;
    use tracing_subscriber::EnvFilter;
    // ITO_LOG takes precedence, then RUST_LOG; an invalid value is reported
    // rather than silently ignored.
    let from_env = |var: &str| -> Option<EnvFilter> {
        let text = std::env::var(var).ok()?;
        match EnvFilter::try_new(&text) {
            Ok(f) => Some(f),
            Err(e) => {
                eprintln!("warning: ignoring invalid {var}={text:?}: {e}");
                None
            }
        }
    };
    let filter = from_env("ITO_LOG")
        .or_else(|| from_env("RUST_LOG"))
        .unwrap_or_else(|| {
            // `-v` raises the level for the `ito` target only -- the runner and
            // the stdlib, which is what a script's own `log.*` calls come out
            // as. Every other crate stays at `warn` however loud `-v` gets.
            // The `ito` target defaults to `info`, so a script's `log.info` is
            // visible out of the box.
            let level = match verbosity {
                0 => "info",
                1 => "debug",
                _ => "trace",
            };
            EnvFilter::new(format!("warn,ito={level}"))
        });
    let ansi =
        io::stderr().is_terminal() && std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty());
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(|| sl::ui::progress::SuspendedStderr)
        .with_ansi(ansi)
        .with_target(false)
        .without_time()
        .try_init()
        .ok();
}

/// Scan `root` for runnable scripts and print them as ready-to-run commands.
///
/// A convenience for a bare invocation, not a resolution rule: `SCRIPT` is
/// always a path, so every entry is printed as the path you would type. `./`,
/// `ito/` and `.ito/` are scanned because those are where scripts conventionally
/// live — the directory names are shared by all the binaries, so one script tree
/// serves whichever one you reach for.
fn list_available_scripts(root: &Path, program: &str) {
    use std::collections::BTreeSet;

    let mut paths: BTreeSet<String> = BTreeSet::new();

    for subdir in &["", "ito", ".ito"] {
        let Ok(entries) = std::fs::read_dir(root.join(subdir)) else {
            continue;
        };
        for entry in entries.flatten() {
            let fname = entry.file_name();
            let fname = fname.to_string_lossy();
            // Dotfiles are skipped: a `.foo.luau` is hidden on purpose.
            if fname.starts_with('.') || !entry.path().is_file() {
                continue;
            }
            if fname.ends_with(".lua") || fname.ends_with(".luau") {
                let prefix = if subdir.is_empty() {
                    "./".to_string()
                } else {
                    format!("{subdir}/")
                };
                paths.insert(format!("{prefix}{fname}"));
            }
        }
    }

    if paths.is_empty() {
        println!("No scripts found in {}.", root.display());
        println!("Write one and run it by path, e.g. `{program} ./build.luau`.");
    } else {
        println!("Scripts in {}:", root.display());
        for path in &paths {
            println!("  {program} {path}");
        }
    }
    println!();
    println!(
        "Run `{program} --help` for CLI usage, `{program} --doc` for scripting documentation."
    );
}

/// Absolute path for an entry script: `base`-joined if relative, then lexically
/// normalized (no filesystem access, so a missing file is fine — loading already
/// succeeded by this point).
fn abs_from(base: &Path, p: &Path) -> PathBuf {
    let joined = if p.is_absolute() {
        p.to_path_buf()
    } else {
        base.join(p)
    };
    let mut out = PathBuf::new();
    for comp in joined.components() {
        match comp {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// Turn a leading shebang line (`#!...`) into a Luau comment (`--...`) so Luau
/// doesn't choke on the `#`, without shifting any line numbers.
fn comment_out_shebang(mut source: String) -> String {
    if source.starts_with("#!") {
        source.replace_range(..2, "--");
    }
    source
}
