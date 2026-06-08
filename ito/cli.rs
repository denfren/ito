use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

/// The shells `ito util completions` can generate scripts for.
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum Shell {
    Bash,
    Zsh,
    Fish,
}

impl From<Shell> for clap_complete::Shell {
    fn from(s: Shell) -> Self {
        match s {
            Shell::Bash => clap_complete::Shell::Bash,
            Shell::Zsh => clap_complete::Shell::Zsh,
            Shell::Fish => clap_complete::Shell::Fish,
        }
    }
}

#[derive(Parser)]
#[command(
    name = "ito",
    version,
    disable_help_subcommand = true,
    about = "Rhai script runner",
    long_about = "Run a Rhai script. The script gets a virtual `fs` module rooted at the working directory (see -C). Buffered writes are flushed to disk after the script returns, unless --dry-run is given."
)]
pub struct Cli {
    #[arg(
        short = 'v',
        long = "verbose",
        action = clap::ArgAction::Count,
        global = true,
        help = "Increase log verbosity (-v info, -vv debug, -vvv trace). Logs go to stderr.",
    )]
    pub verbose: u8,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Run an inline Rhai expression or program.
    Exec {
        /// The Rhai expression or program to run.
        #[arg(value_name = "STR")]
        expression: String,

        #[command(flatten)]
        run: RunArgs,
    },
    /// Run a Rhai script from a file (or '-' for stdin).
    ///
    /// With no PATH, discovers a default script in the working
    /// directory: one of `ito.rhai`, `.ito.rhai`, `ito/main.rhai`, or
    /// `.ito/main.rhai`. It is an error for more than one to exist.
    Run {
        /// Script file to run, '-' for stdin, or omit to discover a
        /// default (`ito.rhai`, `.ito.rhai`, `ito/main.rhai`, or
        /// `.ito/main.rhai`) in the working directory.
        #[arg(value_name = "PATH")]
        script_path: Option<PathBuf>,

        #[command(flatten)]
        run: RunArgs,
    },
    /// Utility subcommands (shell completions, …).
    Util {
        #[command(subcommand)]
        command: UtilCommand,
    },
    /// Print scripting help. With no TOPIC, lists the available topics.
    Help {
        /// Topic to print ('all' for everything). Omit to list topics.
        #[arg(value_name = "TOPIC")]
        topic: Option<String>,

        /// Emit raw markdown without ANSI styling (also implied when
        /// stdout is not a terminal, or when NO_COLOR is set).
        #[arg(long = "plain")]
        plain: bool,

        /// Write directly to stdout instead of through a pager.
        #[arg(long = "no-pager")]
        no_pager: bool,
    },
}

#[derive(Subcommand)]
pub enum UtilCommand {
    /// Generate a shell completion script (to stdout).
    ///
    /// Source it from your shell's startup file, e.g.
    /// `eval "$(ito util completions zsh)"` or write it to the shell's
    /// completion directory.
    Completions {
        /// Shell to generate completions for.
        #[arg(value_name = "SHELL", value_enum)]
        shell: Shell,
    },
}

/// Flags shared by `exec` and `run`: working directory, injected args,
/// and output control.
#[derive(Args, Debug, Clone)]
pub struct RunArgs {
    #[arg(
        short = 'C',
        long = "directory",
        value_name = "DIR",
        action = clap::ArgAction::Append,
        help_heading = "Run control",
        help = "Change to DIR before doing anything else (sets the `fs` module root). Repeatable; each is relative to the previous one.",
    )]
    pub directory: Vec<PathBuf>,

    #[arg(
        long = "unsafe-proc",
        help_heading = "Run control",
        help = "Register the `proc` module, allowing the script to execute arbitrary processes. Off by default (safe sandbox)."
    )]
    pub unsafe_proc: bool,

    #[arg(
        long = "unsafe-fs-flush",
        help_heading = "Run control",
        help = "Allow the script to call `fs::flush()`, writing buffered changes to disk mid-run. Off by default (safe sandbox)."
    )]
    pub unsafe_fs_flush: bool,

    #[command(flatten, next_help_heading = "Script arguments")]
    pub script_args: ScriptArgsFlags,

    #[command(flatten, next_help_heading = "Output")]
    pub output: OutputArgs,
}

/// Output control. The action axis (`--dry-run`) is independent of the
/// report axis (`--diff`, `--list-changed`, `-q`). `--diff` and
/// `--list-changed` are mutually exclusive, and either one implies `-q`.
#[derive(Args, Debug, Clone)]
pub struct OutputArgs {
    #[arg(
        short = 'n',
        long = "dry-run",
        display_order = 0,
        help = "Compute buffered writes but do not flush them to disk."
    )]
    pub dry_run: bool,

    #[arg(
        short = 'd',
        long = "diff",
        display_order = 1,
        help = "Print a unified diff of buffered writes against disk (to stdout). Implies -q. Mutually exclusive with --list-changed."
    )]
    pub diff: bool,

    #[arg(
        short = 'l',
        long = "list-changed",
        display_order = 2,
        conflicts_with = "diff",
        help = "Print the script-visible path of each changed file (to stdout). Implies -q. Mutually exclusive with --diff."
    )]
    pub changed: bool,

    #[arg(
        short = 'q',
        long = "quiet",
        display_order = 3,
        help = "Suppress script print() output."
    )]
    pub quiet: bool,

    #[arg(
        short = 'e',
        long = "exit-code",
        display_order = 4,
        help = "Exit 1 if there were changes, 0 if none (2 still means error). Like `git diff --exit-code`."
    )]
    pub exit_code: bool,
}

#[derive(Args, Debug, Clone)]
pub struct ScriptArgsFlags {
    #[arg(
        short = 'S',
        long = "string",
        value_names = ["NAME", "VALUE"],
        num_args = 2,
        action = clap::ArgAction::Append,
        display_order = 0,
        help = "Inject a string global into the script scope.",
    )]
    pub arg_string: Vec<String>,
    #[arg(
        short = 'N',
        long = "numeric",
        value_names = ["NAME", "VALUE"],
        num_args = 2,
        action = clap::ArgAction::Append,
        display_order = 1,
        help = "Inject a numeric global (i64 or f64) into the script scope.",
    )]
    pub arg_numeric: Vec<String>,
    #[arg(
        short = 'B',
        long = "bool",
        value_names = ["NAME", "VALUE"],
        num_args = 2,
        action = clap::ArgAction::Append,
        display_order = 2,
        help = "Inject a boolean global into the script scope.",
    )]
    pub arg_bool: Vec<String>,
}

pub(crate) struct LoadedScript {
    pub(crate) label: String,
    pub(crate) source: String,
    /// The directory `import` paths resolve relative to. For a real file
    /// this is the directory containing the script; for inline/stdin it is
    /// `None` (the caller falls back to the VFS root).
    pub(crate) base_dir: Option<PathBuf>,
}
