mod cli;
mod imports;

use std::io;
use std::path::PathBuf;

use crate::cli::{Command, LoadedScript, RunArgs, ScriptArgsFlags, Shell, UtilCommand};
use clap::{CommandFactory, Parser};
use cli::Cli;
use ito::scripting::{ScriptArg, Vfs};

fn main() {
    let cli = Cli::parse();
    init_tracing(cli.verbose);

    let result = match cli.command {
        Command::Exec { expression, run } => {
            let source = ScriptSource::Inline(expression);
            run_script(source, run)
        }
        Command::Run { script_path, run } => run_script(ScriptSource::File(script_path), run),
        Command::Help {
            topic,
            plain,
            no_pager,
        } => ito::help::doc(topic.as_deref(), plain, no_pager)
            .map(|()| Outcome::default())
            .map_err(RunError::from),
        Command::Util { command } => match command {
            UtilCommand::Completions { shell } => {
                generate_completions(shell);
                Ok(Outcome::default())
            }
        },
    };

    match result {
        Err(e) => {
            tracing::error!("{}", e.message);
            std::process::exit(e.code);
        }
        // `--exit-code`: 1 when the run produced changes, else 0.
        Ok(outcome) if outcome.exit_code && outcome.changed => std::process::exit(1),
        Ok(_) => {}
    }
}

pub(crate) fn init_tracing(verbosity: u8) {
    use tracing_subscriber::EnvFilter;
    // ITO_LOG takes precedence, then RUST_LOG (the EnvFilter default).
    let filter = EnvFilter::try_from_env("ITO_LOG")
        .or_else(|_| EnvFilter::try_from_default_env())
        .unwrap_or_else(|_| {
            EnvFilter::new(match verbosity {
                0 => "warn",
                1 => "info",
                2 => "debug",
                _ => "trace",
            })
        });
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(io::stderr)
        .with_target(false)
        .without_time()
        .try_init()
        .ok();
}

fn run_script(source: ScriptSource, run: RunArgs) -> Result<Outcome, RunError> {
    // --diff/--list-changed imply --quiet: their report is the output, so
    // script print() is suppressed.
    let script_echo = !(run.output.quiet || run.output.diff || run.output.changed);
    let args = collect_script_args(run.script_args)?;

    for dir in &run.directory {
        std::env::set_current_dir(dir).map_err(|e| format!("-C {}: {e}", dir.display()))?;
    }
    let vfs_root = std::env::current_dir().map_err(|e| format!("getcwd: {e}"))?;

    // Resolve the script source after `-C` is applied: default discovery
    // is relative to the resolved working directory.
    let loaded = match source {
        ScriptSource::Inline(s) => LoadedScript {
            label: "<expression>".into(),
            source: s,
            base_dir: None,
        },
        ScriptSource::File(Some(p)) => load_script_file(&p).map_err(RunError::from)?,
        ScriptSource::File(None) => {
            let p = discover_default_script(&vfs_root)?;
            load_script_file(&p).map_err(RunError::from)?
        }
    };

    let mut rhai = rhai::Engine::new();
    rhai.set_max_expr_depths(0, 64);

    let vfs = Vfs::new(&vfs_root)?;

    // Resolve `import` paths relative to the script's directory (the VFS
    // root for inline/stdin), sandboxed to the root when the script lives
    // inside it. See `imports`.
    let mapper_root = vfs.mapper();
    let import_base = match &loaded.base_dir {
        Some(dir) => dir
            .canonicalize()
            .map_err(|e| format!("canonicalize {}: {e}", dir.display()))?,
        None => mapper_root.root().to_path_buf(),
    };
    // Host globals also seeded into imported modules. `MAIN` is `false`
    // here: a module is never the entry point. `ITO_VERSION` matches the
    // entry script.
    let mut module_globals = rhai::Scope::new();
    module_globals.push_constant("MAIN", false);
    module_globals.push_constant("ITO_VERSION", env!("CARGO_PKG_VERSION"));
    rhai.set_module_resolver(imports::ImportResolver::new(
        &import_base,
        &mapper_root,
        module_globals,
    ));

    ito::scripting::vfs::register(&mut rhai, &vfs, run.unsafe_fs_flush, run.output.dry_run);
    ito::scripting::json::register(&mut rhai);
    ito::scripting::path::register(&mut rhai);
    ito::scripting::re::register(&mut rhai);
    ito::scripting::toml::register(&mut rhai);
    ito::scripting::yaml::register(&mut rhai);
    ito::scripting::hcl::register(&mut rhai);
    ito::scripting::j2::register(&mut rhai, &vfs);
    ito::scripting::version::register(&mut rhai, env!("CARGO_PKG_VERSION"));
    ito::scripting::string::register(&mut rhai);
    // The `proc` module executes arbitrary processes — off by default;
    // opt in with `--unsafe-proc`.
    if run.unsafe_proc {
        ito::scripting::proc::register(&mut rhai);
    }
    if script_echo {
        rhai.on_print(|s| {
            use std::io::Write;
            let mut out = std::io::stdout().lock();
            let _ = writeln!(out, "{s}");
        });
    } else {
        rhai.on_print(|s| tracing::info!(target: "ito::rhai::print", "{s}"));
    }
    // `debug(...)` always goes through tracing (at debug level), regardless
    // of print echoing.
    rhai.on_debug(|s, src, _pos| match src {
        Some(src) => tracing::debug!(target: "ito::rhai::debug", "{src}: {s}"),
        None => tracing::debug!(target: "ito::rhai::debug", "{s}"),
    });

    let ast = rhai
        .compile(&loaded.source)
        .map_err(|e| RunError::script(format!("compile error in {}: {e}", loaded.label)))?;

    let mut scope = rhai::Scope::new();
    scope.push_constant("MAIN", true);
    scope.push_constant("ITO_VERSION", env!("CARGO_PKG_VERSION"));
    ito::scripting::args::register(&mut scope, &args);

    rhai.eval_ast_with_scope::<rhai::Dynamic>(&mut scope, &ast)
        .map(|_| ())
        .map_err(|e| RunError::script(format!("script error in {}: {e}", loaded.label)))?;

    // The set of real changes (buffered writes differing from disk) is the
    // shared basis for the --list-changed listing and the --exit-code status.
    let changed = vfs.changed();

    // Report axis: --diff and --list-changed are mutually exclusive (clap
    // enforces this). They report against the buffered writes, so they work
    // whether or not we go on to flush.
    {
        use std::io::Write;
        let mut out = std::io::stdout().lock();
        if run.output.changed {
            for path in &changed {
                writeln!(out, "{path}").map_err(|e| format!("write changed: {e}"))?;
            }
        }
        if run.output.diff {
            write!(out, "{}", vfs.diff()).map_err(|e| format!("write diff: {e}"))?;
        }
    }

    let outcome = Outcome {
        exit_code: run.output.exit_code,
        changed: !changed.is_empty(),
    };

    // Action axis: flush to disk unless this is a dry run.
    if !run.output.dry_run {
        vfs.flush()?;
    }
    Ok(outcome)
}

/// Where a run gets its script from before `-C` is resolved.
enum ScriptSource {
    /// An inline expression/program (`ito exec`).
    Inline(String),
    /// A path from `ito run` (`Some` is the given PATH, `-` is stdin;
    /// `None` means discover a default in the working directory).
    File(Option<PathBuf>),
}

/// What a script run reports back to `main` for exit-code purposes.
#[derive(Default)]
struct Outcome {
    /// `--exit-code` was requested.
    exit_code: bool,
    /// The run produced at least one real change (buffered write differing
    /// from disk).
    changed: bool,
}

/// An error from a run, carrying the process exit code to use.
///
/// Exit codes: `2` is the generic failure (usage, environment, script
/// load, output/flush) — not the script's fault. `3` is reserved for
/// failures attributable to the script itself: a parse/compile error or
/// a runtime error. `0`/`1` are success (see [`Outcome`]).
struct RunError {
    code: i32,
    message: String,
}

impl RunError {
    /// A failure attributable to the script (parse or runtime): exit `3`.
    fn script(message: String) -> Self {
        Self { code: 3, message }
    }
}

/// Generic failures (`?` on a `String`/`Result<_, String>`) default to
/// exit `2`.
impl From<String> for RunError {
    fn from(message: String) -> Self {
        Self { code: 2, message }
    }
}

/// Write a shell completion script for `shell` to stdout.
fn generate_completions(shell: Shell) {
    let mut cmd = Cli::command();
    let name = cmd.get_name().to_string();
    clap_complete::generate(
        clap_complete::Shell::from(shell),
        &mut cmd,
        name,
        &mut std::io::stdout(),
    );
}

/// The default script names `ito run` looks for (no PATH), in listing
/// order. It is an error for more than one to exist.
const DEFAULT_SCRIPTS: [&str; 4] = ["ito.rhai", ".ito.rhai", "ito/main.rhai", ".ito/main.rhai"];

pub(crate) fn load_script_file(p: &std::path::Path) -> Result<LoadedScript, String> {
    if p.as_os_str() == "-" {
        use std::io::Read;
        let mut source = String::new();
        std::io::stdin()
            .read_to_string(&mut source)
            .map_err(|e| format!("stdin read error: {e}"))?;
        return Ok(LoadedScript {
            label: "<stdin>".into(),
            source,
            base_dir: None,
        });
    }
    let label = p.display().to_string();
    let source = std::fs::read_to_string(p)
        .map_err(|e| format!("script read error {}: {e}", p.display()))?;
    // Imports resolve relative to the directory containing the script.
    let base_dir = p
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .map(std::path::Path::to_path_buf)
        .or_else(|| Some(std::path::PathBuf::from(".")));
    Ok(LoadedScript {
        label,
        source,
        base_dir,
    })
}

/// Find the single default script in `root`. Errors (exit `2`) if none
/// or more than one of [`DEFAULT_SCRIPTS`] exists.
fn discover_default_script(root: &std::path::Path) -> Result<PathBuf, RunError> {
    let found: Vec<PathBuf> = DEFAULT_SCRIPTS
        .iter()
        .map(|name| root.join(name))
        .filter(|p| p.is_file())
        .collect();
    match found.as_slice() {
        [p] => Ok(p.clone()),
        [] => Err(format!(
            "no script given and no default found in {} (looked for {})",
            root.display(),
            DEFAULT_SCRIPTS.join(", "),
        )
        .into()),
        many => Err(format!(
            "ambiguous default script in {}: {} all exist; pass an explicit path",
            root.display(),
            many.iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", "),
        )
        .into()),
    }
}

pub(crate) fn collect_script_args(
    flags: ScriptArgsFlags,
) -> Result<Vec<(String, ScriptArg)>, String> {
    let mut out: Vec<(String, ScriptArg)> = Vec::new();
    for pair in flags.arg_string.chunks_exact(2) {
        out.push((pair[0].clone(), ScriptArg::String(pair[1].clone())));
    }
    for pair in flags.arg_numeric.chunks_exact(2) {
        let name = &pair[0];
        let v = &pair[1];
        let parsed = if let Ok(i) = v.parse::<i64>() {
            ScriptArg::Int(i)
        } else if let Ok(f) = v.parse::<f64>() {
            ScriptArg::Float(f)
        } else {
            return Err(format!("invalid numeric value for --numeric {name}: {v}"));
        };
        out.push((name.clone(), parsed));
    }
    for pair in flags.arg_bool.chunks_exact(2) {
        let name = &pair[0];
        let v = &pair[1];
        let parsed = match v.as_str() {
            "true" | "True" | "TRUE" => ScriptArg::Bool(true),
            "false" | "False" | "FALSE" => ScriptArg::Bool(false),
            _ => return Err(format!("invalid bool value for --bool {name}: {v}")),
        };
        out.push((name.clone(), parsed));
    }
    Ok(out)
}
