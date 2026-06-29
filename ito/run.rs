//! The phases of a run, for binaries to compose.
//!
//! A run is startup, then an engine, then evaluation, then output. What differs
//! between the binaries is *one value*: the [`Config`] describing what the
//! script may reach. Everything else is the same code on every path.
//!
//! ```ignore
//! // bin/iq.rs -- the whole binary, minus the early-exit `let else`s
//! let startup = Startup::begin(cli.common)?;          // -v, -C
//! let script = startup.script()?;                     // SCRIPT / - / -s
//! let input = startup.input(..)?;                     // INPUT / - / nothing
//! let engine = Engine::build(&script, Config::iq(cli.filesystem, cli.quiet, input, argv))?;
//! let answer = engine.eval(&script).await?;           // chunk, then main()
//! emit_returned(&answer, &script, "iq", cli.quiet)?;   // return value IS output
//! ```
//!
//! `ied` and `ito` differ only in which [`Config`] constructor they name; `ied`
//! additionally ends with `emit_reports` + `flush_writes`, and `irun` reuses
//! `Config::ito` verbatim.
//!
//! Two orderings are load-bearing and both are enforced rather than documented:
//! registration order lives inside [`Engine::build`], which is the only way to
//! obtain an `Engine`, so no caller can get it wrong; and the task drain must
//! precede a flush (a task's buffered writes are only real once it has
//! finished), which is [`Engine::eval`]'s own doing.

use std::io;
use std::path::{Path, PathBuf};

use crate::cli::{CommonArgs, LoadedScript, ReportArgs};

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
/// `ito`/`irun`, `proc.trap` is the one place a script opts into handling a
/// signal, and a signal nothing trapped keeps its default disposition (SIGINT
/// terminates, as it always did) rather than this function racing the body
/// against it. `iq`/`ied` register no traps and see the same default behaviour
/// they always did.
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
    /// The `-C`-resolved working directory: the `fs` root under `iq`/`ied`, and
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
        // The program name picks the default chapter, so `iq --doc` opens on
        // `iq` while still carrying every other topic.
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
        let Some(arg) = self.common.script.clone() else {
            tracing::debug!("No script specified");
            list_available_scripts(&self.working_dir, &self.program_name());
            return Ok(None);
        };

        // `require_root` is the confinement boundary for `require` (see
        // `scripting::require`), independent of the `fs` root. It is anchored to
        // the invocation directory for the same reason `SCRIPT` is.
        let require_root = self
            .common
            .require_root
            .as_ref()
            .map(|r| self.invocation_dir.join(r));

        if self.common.inline {
            return Ok(Some(Script::inline(arg, require_root, &self.working_dir)));
        }
        if arg == "-" {
            tracing::trace!("Read from STDIN");
            return Ok(Some(Script::stdin(require_root, &self.working_dir)?));
        }

        // A file already has a root: its own directory. Accepting `-r` too would
        // silently mean one of two different things, so it is refused.
        if require_root.is_some() {
            return Err(
                "-r/--require applies to stdin and -s scripts; a SCRIPT file \
                        imports relative to its own directory"
                    .to_string()
                    .into(),
            );
        }
        // Resolved against the invocation cwd, before `-C` took effect, so the
        // absolute path survives the chdir.
        let path = self.invocation_dir.join(&arg);
        if !path.is_file() {
            return Err(format!("no script at '{}'", path.display()).into());
        }
        Ok(Some(Script::file(&path, &self.working_dir)?))
    }

    /// Read the `INPUT` positional: a file path, or `-` for stdin.
    ///
    /// Resolved against the invocation directory like `SCRIPT`, so `-C` moving
    /// the `fs` root does not move the data out from under a relative path.
    pub fn input(&self, arg: Option<&str>) -> Result<Option<String>, RunError> {
        let Some(arg) = arg else { return Ok(None) };
        if arg == "-" {
            use std::io::Read;
            let mut buf = String::new();
            std::io::stdin()
                .read_to_string(&mut buf)
                .map_err(|e| format!("read input from stdin: {e}"))?;
            return Ok(Some(buf));
        }
        let path = self.invocation_dir.join(arg);
        std::fs::read_to_string(&path)
            .map(Some)
            .map_err(|e| format!("read input {}: {e}", path.display()).into())
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
    loaded: LoadedScript,
    /// The `require` confinement boundary, or `None` when imports are denied.
    require_root: Option<PathBuf>,
    /// The `-C`-resolved working directory: the `fs` root where `fs` is rooted,
    /// and what relative paths resolve against where it is not.
    vfs_root: PathBuf,
    /// Chunk name, which `require`'s caller-detection resolves against.
    chunk_name: String,
}

impl Script {
    /// A `-s`/`--script` expression.
    ///
    /// No file, so no directory of its own: imports are denied unless `-r` named
    /// a root. The cwd it happened to be run from is not a boundary anyone chose.
    fn inline(source: String, require_root: Option<PathBuf>, vfs_root: &Path) -> Self {
        let loaded = LoadedScript {
            label: "<expression>".into(),
            source,
        };
        Self::synthetic(loaded, require_root, vfs_root)
    }

    /// A script piped in on stdin. Same rootlessness as inline.
    fn stdin(require_root: Option<PathBuf>, vfs_root: &Path) -> Result<Self, RunError> {
        let loaded = load_script_file(Path::new("-")).map_err(RunError::from)?;
        Ok(Self::synthetic(loaded, require_root, vfs_root))
    }

    /// A script file, which anchors the require root to its own directory.
    fn file(path: &Path, vfs_root: &Path) -> Result<Self, RunError> {
        let loaded = load_script_file(path).map_err(RunError::from)?;
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
            loaded: LoadedScript {
                label: loaded.label,
                source: strip_shebang(loaded.source),
            },
            require_root: Some(require_root),
            vfs_root: vfs_root.to_path_buf(),
            chunk_name,
        })
    }

    /// Shared tail of [`Self::inline`] and [`Self::stdin`]: a script with no file
    /// of its own, so its chunk name has to be synthesized.
    fn synthetic(loaded: LoadedScript, require_root: Option<PathBuf>, vfs_root: &Path) -> Self {
        let chunk_name = match &require_root {
            // Inside the require root, so relative imports resolve there.
            Some(root) => format!(
                "@{}",
                root.join(loaded.label.trim_matches(['<', '>'])).display()
            ),
            // No file and no root: `require` is denied anyway, so the chunk gets
            // an honest `=` label rather than a path it does not have.
            None => format!("={}", loaded.label),
        };
        Self {
            loaded: LoadedScript {
                label: loaded.label,
                source: strip_shebang(loaded.source),
            },
            require_root,
            vfs_root: vfs_root.to_path_buf(),
            chunk_name,
        }
    }

    /// How this script names itself in errors (`<expression>`, `<stdin>`, a path).
    pub fn label(&self) -> &str {
        &self.loaded.label
    }

    /// The `fs` root.
    pub fn vfs_root(&self) -> &Path {
        &self.vfs_root
    }

    /// The `require` confinement boundary, or `None` where imports are denied.
    pub fn require_root(&self) -> Option<&Path> {
        self.require_root.as_deref()
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
/// `proc`/`ssh` `:env()` covers passing a variable to a child without mutating
/// process-wide state that every later task would then observe.
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
    /// No `fs` at all. A script reads only what it was handed.
    None,
    /// Rooted at the working directory, escapes rejected. `writes` decides
    /// whether the write half exists (see [`sl::vfs::Writes`]).
    Rooted { writes: sl::vfs::Writes },
    /// The host filesystem: no root, writes land immediately.
    Host,
}

/// Who owns stdout, and so how the script's output reaches it.
///
/// One choice rather than two, because "is there a `print`?" and "what is
/// written at the end?" are the same question asked twice. A binary whose
/// answer travels back as a value cannot also have a `print` writing to the
/// same stream.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Stdout {
    /// The script's return value is the output: no `print` at all, not even
    /// Luau's built-in.
    Returned,
    /// `print` writes to stdout, or to the log when `echo` is false — which is
    /// how a report that owns stdout avoids being interleaved with it.
    Print { echo: bool },
}

/// What a script may reach: the whole of what differs between the binaries.
///
/// Everything else about a run is identical on every path, so this struct *is*
/// the capability boundary. Written as data rather than a call sequence so a
/// binary's surface can be read (and diffed) in one place — see the
/// per-binary constructors below.
///
/// `input` and `argv` are here for uniformity, not as capabilities: what a
/// script is *handed* is the same everywhere (see `cli::InputArgs`).
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
    pub stdout: Stdout,
    /// The `input` global: raw, unparsed text. `None` leaves it undefined.
    pub input: Option<String>,
    /// The script's own arguments, after `::`.
    pub argv: Argv,
}

impl Config {
    /// `iq` — a filter: the return value is the output, `fs` is opt-in via `-f`
    /// and read-only, and nothing ambient may feed an answer that should depend
    /// only on its input.
    pub fn iq(filesystem: bool, input: Option<String>, argv: Argv) -> Self {
        Self {
            fs: if filesystem {
                Fs::Rooted {
                    writes: sl::vfs::Writes::Denied,
                }
            } else {
                Fs::None
            },
            host_io: false,
            ambient: false,
            stdout: Stdout::Returned,
            input,
            argv,
        }
    }

    /// `ied` — buffered edits. A report owns stdout, so `print` goes to the log
    /// rather than interleaving with the diff or the file list.
    ///
    /// A preview is only worth anything if the run it previews computes the same
    /// change set, so nothing ambient may feed into one.
    pub fn ied(quiet: bool, report: &ReportArgs, input: Option<String>, argv: Argv) -> Self {
        Self {
            fs: Fs::Rooted {
                writes: sl::vfs::Writes::Buffered,
            },
            host_io: false,
            ambient: false,
            stdout: Stdout::Print {
                echo: !(quiet || report.diff || report.changed),
            },
            input,
            argv,
        }
    }

    /// `ito` (and `irun`, which is the same engine with a different argument
    /// source) — the full toolset. Writes land immediately, since a subprocess
    /// would not see a buffered one.
    ///
    /// Keeps the ambient state: a binary that already spawns processes and opens
    /// sockets would gain nothing from being denied the clock.
    pub fn ito(quiet: bool, input: Option<String>, argv: Argv) -> Self {
        Self {
            fs: Fs::Host,
            host_io: true,
            ambient: true,
            stdout: Stdout::Print { echo: !quiet },
            input,
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
        // ito's own. With no root there is nothing to confine to, so `require`
        // becomes one that only explains itself: leaving the default in place
        // would be an unconfined requirer, the opposite of the intent.
        match &script.require_root {
            Some(root) => sl::require::register(&lua, root.clone())?,
            None => sl::require::register_denied(&lua)?,
        }

        // The modules every binary has: formats, paths, regex, logging, string
        // methods, the `ito` module, and the script's own `argv`.
        //
        // No `debug_print`/`MAIN`/`ITO_VERSION`: `log.debug` already logs
        // through tracing, a `main` function already marks the entry point (the
        // runner calls it when present), and `ito.version_check` already
        // asserts compatibility -- which is the only thing a script did with
        // the version.
        sl::json::register(&lua)?;
        sl::path::register(&lua)?;
        sl::re::register(&lua)?;
        sl::toml::register(&lua)?;
        sl::yaml::register(&lua)?;
        sl::hcl::register(&lua)?;
        sl::tablefmt::register(&lua)?;
        sl::time::register(&lua)?;
        // Hashing a value is a pure byte-to-digest conversion, so it goes
        // everywhere; only the `_file` forms read the filesystem, and those are
        // added under `host_io` below.
        sl::hash::register(&lua)?;
        sl::ito::register(&lua, env!("CARGO_PKG_VERSION"))?;
        sl::log::register(&lua)?;
        sl::string::register(&lua)?;
        // The ANSI styling methods are pure string transforms, so every binary
        // has them; `ito.colors` is the switch that decides whether they emit
        // anything, and `reset` clears a previous run's setting (the switch is
        // process-wide, so a second `build` in one process must not inherit it).
        sl::ansi::reset();
        sl::ansi::register(&lua)?;
        sl::ansi::register_switch(&lua)?;

        // `cli` reads the command line as given, so it takes `argv` before
        // `argv::register` hands ownership to the global -- a script may
        // reassign that global, and the declared interface should describe the
        // invocation rather than whatever the script last stored there.
        //
        // It writes to stdout, which `iq` and `ied` otherwise own, and still
        // belongs on both: what it prints is *terminal*, so a run that prints
        // help never reaches `emit_returned`/`emit_reports` and cannot
        // interleave with either. Nothing in it is capability-bearing.
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
            Fs::None => None,
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
            sl::ito::register_host_paths(&lua, script.require_root().map(Path::to_path_buf))?;
            // `ito.pid()` joins them: a pid is ambient state that differs
            // between two runs, which is the same line the rest of this block
            // draws.
            sl::ito::register_host_pid(&lua)?;
            // `ito.slug()` joins them: it's random, the same line
            // `remove_random` draws around `math.random` below.
            sl::ito::register_host_slug(&lua)?;
            // The `hash.*_file` forms, for the reason `fs` itself is host-only
            // here: naming a path to read is a filesystem read, and hashing a
            // file outside a rooted `fs`'s root would leak its contents past
            // the containment that root exists to provide. The pure
            // string/buffer forms were registered with the core, above.
            sl::hash::register_host_files(&lua)?;
            // `io.write`/`io.writeln`/`io.ewrite`/`io.ewriteln`: raw stream
            // writes, host-only for the reason `print`/`Stdout` are one
            // choice above -- `iq`'s stdout is its return value and `ied`'s
            // is its report, so neither can hand a script an uncoordinated
            // second writer onto the same stream.
            sl::io::register_host(&lua)?;
            // The clock-reading and local-zone half of `time`, host-only for
            // the reason `os.getenv` is: each answer depends on ambient state
            // the caller never passed, which an `ied --diff` preview must not
            // read or it could disagree with the `--write` after it.
            sl::time::register_host_time(&lua)?;
            // `ito.is_tty()`, and with it the right for `ito.colors(nil)` to
            // detect: both read ambient state (stdout's terminal-ness, the
            // CLICOLOR variables), so the styling methods stay pure under
            // `iq`/`ied` and a script there turns them on explicitly.
            sl::ansi::register_host_detect(&lua)?;
            sl::proc::register(&lua)?;
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

        match config.stdout {
            // Luau installs its own `print`, so withholding ours is not enough
            // -- the built-in has to go, or a second channel would interleave
            // with the returned value on the same stream.
            Stdout::Returned => lua.globals().raw_remove("print")?,
            Stdout::Print { echo } => {
                let print = if echo {
                    lua.create_function(|_, args: mlua::Variadic<mlua::Value>| {
                        use std::io::Write;
                        let mut out = std::io::stdout().lock();
                        let parts: Vec<String> = args.iter().map(display_value).collect();
                        let _ = writeln!(out, "{}", parts.join("\t"));
                        Ok(())
                    })?
                } else {
                    lua.create_function(|_, args: mlua::Variadic<mlua::Value>| {
                        let parts: Vec<String> = args.iter().map(display_value).collect();
                        tracing::info!(target: "ito::lua::print", "{}", parts.join("\t"));
                        Ok(())
                    })?
                };
                lua.globals().raw_set("print", print)?;
            }
        }

        // Raw text, not parsed: stdin carries no indication of its own format,
        // and a file's extension is a hint rather than a fact, so the script
        // says which parser it wants (`json.parse(input)`). Absent rather than
        // empty when nothing was passed, so a script can tell the two apart.
        if let Some(text) = &config.input {
            let value = lua.create_string(text)?;
            lua.globals().raw_set("input", value)?;
        }

        // `j2` now that the `fs` decision is known -- guessing "allowed" would
        // let a template read the disk in a run that exposes no `fs` at all.
        match config.fs {
            Fs::Rooted { .. } => sl::j2::register(
                &lua,
                vfs.as_ref().expect("a rooted fs built a VFS just above"),
            )?,
            Fs::Host => sl::j2::register_host(&lua)?,
            Fs::None => sl::j2::register_without_fs(&lua)?,
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
            None => RunError::script(format!("script error in {}: {e}", script.loaded.label)),
        };

        let chunk_value: mlua::Value = self
            .lua
            .load(&script.loaded.source)
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

/// Write a script's return value to stdout, verbatim.
///
/// Verbatim means no added trailing newline, so `$(iq ...)` and a pipe both get
/// exactly what was returned.
///
/// A non-string is a script error rather than a coercion: `tostring` on a table
/// yields `table: 0x...`, which is never the answer anyone wanted, so
/// stringifying would turn a visible mistake into silently wrong output.
pub fn emit_returned(
    answer: &mlua::Value,
    script: &Script,
    program: &str,
    quiet: bool,
) -> Result<(), RunError> {
    if quiet {
        return Ok(());
    }
    let label = &script.loaded.label;
    let text = match answer {
        mlua::Value::String(s) => s.to_str()?.to_string(),
        mlua::Value::Nil => {
            return Err(RunError::script(format!(
                "{program} expects {label} to return a string; it returned nothing. \
                 End the script with `return <string>` (json.to_string(..) or \
                 table.concat(..) if the answer is structured)."
            )));
        }
        other => {
            let ty = other.type_name();
            // "an integer", not "a integer".
            let article = if ty.starts_with(['a', 'e', 'i', 'o', 'u']) {
                "an"
            } else {
                "a"
            };
            return Err(RunError::script(format!(
                "{program} expects {label} to return a string; it returned {article} {ty}. \
                 Serialize it first, e.g. `return json.to_string(result)`."
            )));
        }
    };
    use std::io::Write;
    std::io::stdout()
        .lock()
        .write_all(text.as_bytes())
        .map_err(|e| format!("write output: {e}"))?;
    Ok(())
}

/// Print the `--diff`/`--list-changed` report and compute the run's outcome.
///
/// Both report against the *buffered* writes, so they are meaningful whether or
/// not the caller goes on to flush. clap enforces that they are mutually
/// exclusive.
pub fn emit_reports(vfs: &Vfs, report: &ReportArgs) -> Result<Outcome, RunError> {
    // The set of real changes -- buffered writes differing from disk -- is the
    // shared basis for the listing and the exit status.
    let changed = vfs.changed().map_err(|e| format!("changed: {e}"))?;

    use std::io::Write;
    let mut out = std::io::stdout().lock();
    if report.changed {
        for path in &changed {
            writeln!(out, "{path}").map_err(|e| format!("write changed: {e}"))?;
        }
    }
    if report.diff {
        write!(out, "{}", vfs.diff().map_err(|e| format!("diff: {e}"))?)
            .map_err(|e| format!("write diff: {e}"))?;
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

/// `tostring`-style rendering for `print`/`debug_print` arguments.
fn display_value(v: &mlua::Value) -> String {
    v.to_string().unwrap_or_else(|e| format!("<error: {e}>"))
}

/// Install the log subscriber at the verbosity `-v` asked for.
///
/// Call it straight after parsing and before any check that can fail: an error
/// raised earlier has nowhere to be written, and a usage mistake would exit
/// non-zero in silence.
pub fn init_tracing(verbosity: u8) {
    use tracing_subscriber::EnvFilter;
    // ITO_LOG takes precedence, then RUST_LOG (the EnvFilter default).
    let filter = EnvFilter::try_from_env("ITO_LOG")
        .or_else(|_| EnvFilter::try_from_default_env())
        .unwrap_or_else(|_| {
            // `-v` raises the level for the `ito` target only — the runner and
            // the stdlib, which is what a script's own `log.*` calls come out
            // as. Every other crate stays at `warn` however loud `-v` gets:
            // the noise they produce at `debug` and `trace` is about their
            // internals, not the run. `ITO_LOG`/`RUST_LOG` above are the escape
            // hatch when those are wanted.
            let level = match verbosity {
                0 => return EnvFilter::new("warn"),
                1 => "info",
                2 => "debug",
                _ => "trace",
            };
            EnvFilter::new(format!("warn,ito={level}"))
        });
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(io::stderr)
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
fn strip_shebang(mut source: String) -> String {
    if source.starts_with("#!") {
        source.replace_range(..2, "--");
    }
    source
}

pub(crate) fn load_script_file(p: &Path) -> Result<LoadedScript, String> {
    if p.as_os_str() == "-" {
        use std::io::Read;
        let mut source = String::new();
        std::io::stdin()
            .read_to_string(&mut source)
            .map_err(|e| format!("stdin read error: {e}"))?;
        return Ok(LoadedScript {
            label: "<stdin>".into(),
            source,
        });
    }
    let label = p.display().to_string();
    let source = std::fs::read_to_string(p)
        .map_err(|e| format!("script read error {}: {e}", p.display()))?;
    Ok(LoadedScript { label, source })
}
