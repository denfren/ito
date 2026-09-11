//! The command-line surface of each binary.
//!
//! One [`Parser`] per binary — [`IedCli`], [`ItoCli`] — each
//! flattening the shared [`Args`] groups it applies. A binary's flag set is
//! therefore whatever its struct declares: there is no runtime filtering, and no
//! way for a flag to exist but do nothing.
//!
//! The groups, smallest first:
//!
//! - [`CommonArgs`] — everything every binary has: the script path and its
//!   arguments, `-C`, `-v`, the `--doc`/`--completion` exits.
//! - [`ReportArgs`] — previewing buffered writes (`-d`, `-l`, `-e`) and
//!   committing them (`-w`). Only meaningful where writes are buffered, so only
//!   [`IedCli`] flattens it.

use std::path::PathBuf;

use clap::{Args, Parser, ValueEnum};

/// The shells `--completion` can generate scripts for.
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

/// Flags every binary has.
///
/// The `--doc`/`--completion` pair are early exits rather than run options: they
/// print and stop, so they coexist with a missing `SCRIPT`.
#[derive(Args, Debug, Clone)]
pub struct CommonArgs {
    #[arg(
        short = 'v',
        long = "verbose",
        action = clap::ArgAction::Count,
        global = true,
        help = "Increase log verbosity (-v debug, -vv trace; default: info). Logs go to stderr; ITO_LOG/RUST_LOG override.",
    )]
    pub verbose: u8,

    #[arg(
        short = 'C',
        long = "directory",
        value_name = "DIR",
        action = clap::ArgAction::Append,
        global = true,
        help = "Change to DIR before doing anything else (what `fs` paths resolve against; the `fs` root under ied). Repeatable; each is relative to the previous one. SCRIPT is still resolved against the directory you invoked from.",
    )]
    pub directory: Vec<PathBuf>,

    /// Print documentation and exit. With no TOPIC, prints this program's
    /// overview followed by the list of topics ('all' dumps everything).
    #[arg(long = "doc", value_name = "TOPIC", num_args = 0..=1, default_missing_value = "")]
    pub doc: Option<String>,

    /// Emit raw markdown without ANSI styling when using --doc (also
    /// implied when stdout is not a terminal, or when NO_COLOR is set).
    #[arg(long = "doc-plain", requires = "doc")]
    pub doc_plain: bool,

    /// Write --doc output directly to stdout instead of through a pager.
    #[arg(long = "doc-no-pager", requires = "doc")]
    pub doc_no_pager: bool,

    /// Generate a shell completion script (to stdout) and exit.
    ///
    /// Source it from your shell's startup file, e.g.
    /// `eval "$(ito --completion zsh)"` or write it to the shell's
    /// completion directory.
    #[arg(long = "completion", value_name = "SHELL", value_enum)]
    pub completion: Option<Shell>,

    /// The script to run (a file path), followed by its own arguments.
    ///
    /// SCRIPT is resolved against the directory you invoked from, before any
    /// -C takes effect. Run with no arguments to list the scripts in the
    /// current directory.
    ///
    /// This program's own flags go before SCRIPT. Everything after it belongs
    /// to the script and reaches it verbatim as the global `argv` -- including
    /// arguments spelled like this program's flags.
    //
    // One positional rather than `script` + `args`: clap only stops parsing
    // flags once the *last* positional starts collecting, so with two of them
    // `ito s.luau -v` would still take `-v` for itself.
    #[arg(
        value_name = "SCRIPT",
        trailing_var_arg = true,
        allow_hyphen_values = true
    )]
    pub script_and_args: Vec<String>,
}

/// Previewing and committing buffered writes.
///
/// Every flag here reads or releases the pending write set, so the group only
/// makes sense for a run that has one. A binary that writes directly (or not at
/// all) does not flatten it, which is why `ito --diff` is an unknown flag rather
/// than a no-op.
///
/// Writing is opt-in (`-w`): a bare `ied` run computes the whole change set and
/// discards it. That makes the safe thing the default — the tool's usual job is
/// to answer "would this change anything?" as a linter or config check — and it
/// leaves committing as a single composable flag, so `-dw` previews *and*
/// applies rather than needing a separate mode.
#[derive(Args, Debug, Clone, Default)]
pub struct ReportArgs {
    #[arg(
        short = 'w',
        long = "write",
        display_order = 0,
        help = "Apply the buffered writes to disk after the script returns. Without it nothing is written, so a run is a preview by default."
    )]
    pub write: bool,

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
        short = 'e',
        long = "exit-code",
        display_order = 4,
        help = "Exit 1 if there were changes, 0 if none (2 still means error). Like `git diff --exit-code`."
    )]
    pub exit_code: bool,
}

/// `ied` — the restricted, reproducible variant: buffered, previewable edits to one directory.
#[derive(Parser, Debug)]
#[command(
    name = "ied",
    override_usage = "ied [OPTIONS] [SCRIPT [ARGS]...]",
    version,
    about = "Edit files reproducibly with a restricted Luau script",
    long_about = "Edit files reproducibly with a restricted Luau script.\n\n\
        The script's `fs` is rooted at the working directory (see -C). Writes are buffered rather than applied, and nothing reaches disk \
        unless you pass --write: by default a run is a preview, which is what \
        makes it safe to use as a linter or config check. Use --diff, \
        --list-changed and --exit-code to inspect the pending changes, in any \
        combination with --write.\n\n\
        A run depends only on the working directory's state and the script, so \
        anything ambient is absent: the clock, the environment, commands, \
        the network, and paths outside the working directory. Use `ito` for \
        those."
)]
pub struct IedCli {
    #[command(flatten)]
    pub common: CommonArgs,

    #[command(flatten, next_help_heading = "Output")]
    pub report: ReportArgs,

    #[arg(
        short = 'q',
        long = "quiet",
        help_heading = "Output",
        display_order = 3,
        help = "Silence the script's print() output. Implied by --diff and --list-changed."
    )]
    pub quiet: bool,
}

/// `ito` — the full toolset: direct writes plus commands, SSH, EC2 and tasks.
#[derive(Parser, Debug)]
#[command(
    name = "ito",
    override_usage = "ito [OPTIONS] [SCRIPT [ARGS]...]",
    version,
    about = "Run a Luau script",
    long_about = "Run a Luau script.\n\n\
        Relative `fs` paths \
        resolve against the working directory (see -C), absolute paths and \
        `..` reach outside it, and traversals report absolute paths. \
        Writes go to disk as they happen, not at the end: this run can also \
        spawn subprocesses, and buffering writes they cannot see would be a \
        promise that a single `process` call breaks. Because nothing is held \
        back there is nothing to preview, so the --write/--diff/--list-changed \
        flags do not exist here.\n\n\
        It can run local commands (proc), run them remotely (ssh), query AWS \
        (aws), and overlap any of that concurrently (task).\n\n\
        Flags go before SCRIPT; everything after it is the script's own, so a \
        `#!/usr/bin/env -S ito` script owns its whole command line.\n\n\
        Use `ied` for reproducible, previewable edits."
)]
pub struct ItoCli {
    #[command(flatten)]
    pub common: CommonArgs,

    /// Print the AI skill bootstrap prompt for `ito` and exit.
    ///
    /// Outputs a prompt you can feed to an AI assistant to generate an
    /// up-to-date `ito` skill.
    ///
    /// Declared before the flattened groups so it lands in the default
    /// `Options` section rather than inheriting the next `help_heading`.
    #[arg(long = "ai-skill-init")]
    pub ai_skill_init: bool,
}

impl CommonArgs {
    /// The script path, if one was given.
    pub fn script(&self) -> Option<&str> {
        self.script_and_args.first().map(String::as_str)
    }

    /// The script's own arguments, for the `argv` global and `cli`.
    pub fn argv(&self) -> crate::scripting::Argv {
        crate::scripting::Argv(self.script_and_args.iter().skip(1).cloned().collect())
    }
}
