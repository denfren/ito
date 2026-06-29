//! The command-line surface of each binary.
//!
//! One [`Parser`] per binary — [`IqCli`], [`IedCli`], [`ItoCli`] — each
//! flattening the shared [`Args`] groups it applies. A binary's flag set is
//! therefore whatever its struct declares: there is no runtime filtering, and no
//! way for a flag to exist but do nothing.
//!
//! The groups, smallest first:
//!
//! - [`CommonArgs`] — everything every binary has: the script source, `-C`,
//!   `-v`, the `--doc`/`--completion` exits.
//! - [`InputArgs`] — the optional `INPUT` positional, exposed as the `input`
//!   global. Every flag-parsing binary flattens it: handing a script its data on
//!   the command line is orthogonal to what else that script may reach.
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
        help = "Increase log verbosity (-v info, -vv debug, -vvv trace). Logs go to stderr.",
    )]
    pub verbose: u8,

    #[arg(
        short = 'C',
        long = "directory",
        value_name = "DIR",
        action = clap::ArgAction::Append,
        global = true,
        help = "Change to DIR before doing anything else (what `fs` paths resolve against; the `fs` root under iq/ied). Repeatable; each is relative to the previous one. SCRIPT is still resolved against the directory you invoked from.",
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
    /// `eval "$(iq --completion zsh)"` or write it to the shell's
    /// completion directory.
    #[arg(long = "completion", value_name = "SHELL", value_enum)]
    pub completion: Option<Shell>,

    /// Interpret SCRIPT as an inline Luau expression/program instead of a
    /// name or path.
    #[arg(short = 's', long = "script")]
    pub inline: bool,

    #[arg(
        short = 'r',
        long = "require",
        value_name = "DIR",
        help_heading = "Run control",
        help = "Allow a stdin or -s script to `require` modules from DIR, confined to that tree. Without it such scripts cannot import at all. Not valid with a SCRIPT file, which imports relative to itself."
    )]
    pub require_root: Option<PathBuf>,

    /// The script to run: a file path, '-' for stdin, or (with -s) an
    /// inline Luau expression/program.
    ///
    /// Paths are resolved against the directory you invoked from, before
    /// any -C takes effect. Run with no arguments to list the scripts in
    /// the current directory.
    ///
    /// Arguments for the script itself go after a bare '::', which ends
    /// this program's own arguments; they reach the script verbatim as
    /// the global `argv`.
    #[arg(value_name = "SCRIPT")]
    pub script: Option<String>,
}

/// The data a script operates on, as the `input` global.
///
/// Flattened by every flag-parsing binary. What a script may *reach* is the
/// capability boundary between them; what it is *handed* is not, so there is no
/// reason for the spelling to differ — `ied fix.luau data.json` and
/// `ito run.luau -` mean what `iq q.luau data.json` does.
///
/// Reading the positional is [`crate::run::Startup::input`], installing it
/// [`crate::run::Engine::with_input`]; both were always binary-agnostic.
#[derive(Args, Debug, Clone, Default)]
pub struct InputArgs {
    /// The data to operate on, as the `input` global: a file path, or '-' for
    /// stdin.
    ///
    /// Omit it and `input` is nil — a script that only needs its own arguments,
    /// or one reading files through `fs`, has no use for it.
    ///
    /// `input` is the raw text. Parse it for the format you expect
    /// (`json.parse(input)`, `yaml.parse(input)`, …): stdin carries no
    /// indication of its own format, so guessing would only be right sometimes.
    #[arg(value_name = "INPUT")]
    pub input: Option<String>,
}

impl InputArgs {
    /// Reject `SCRIPT` and `INPUT` both being `-`.
    ///
    /// They are separate sources, so either may be stdin — `iq - data.json`
    /// reads the script from a pipe and the data from a file. Both at once is
    /// the one impossible case: stdin cannot be read twice, and splitting it
    /// would need a delimiter nobody asked for.
    pub fn validate(&self, common: &CommonArgs) -> Result<(), String> {
        if common.script.as_deref() == Some("-") && self.input.as_deref() == Some("-") {
            return Err(
                "SCRIPT and INPUT cannot both be '-': stdin can only be read once. \
                 Use -s for an inline script, or name a file for one of them"
                    .into(),
            );
        }
        Ok(())
    }
}

/// Previewing and committing buffered writes.
///
/// Every flag here reads or releases the pending write set, so the group only
/// makes sense for a run that has one. A binary that writes directly (or not at
/// all) does not flatten it, which is why `iq --diff` is an unknown flag rather
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

/// `iq` — query structured files. The script's return value is the output.
#[derive(Parser, Debug)]
#[command(
    name = "iq",
    version,
    about = "Query structured files with a sandboxed Luau script (read-only)",
    long_about = "Query structured files with a sandboxed Luau script.\n\n\
        The script's return value is the output: it must be a string, and it is \
        written to stdout verbatim. There is no print().\n\n\
        A read-only `fs` rooted at the working directory (see -C) provides \
        reads, alongside the format modules (json, yaml, toml, hcl) and \
        helpers. Nothing can be changed: fs.write and fs.remove are not \
        defined, and there is no way to run commands or open network \
        connections.\n\n\
        Use `ied` to edit files, or `ito` for the full toolset."
)]
pub struct IqCli {
    #[command(flatten)]
    pub common: CommonArgs,

    #[command(flatten)]
    pub input: InputArgs,

    #[arg(
        short = 'f',
        long = "fs",
        help_heading = "Run control",
        help = "Expose the working directory as the `fs` module (read-only). Without it `fs` is not defined."
    )]
    pub filesystem: bool,

    #[arg(
        short = 'q',
        long = "quiet",
        help_heading = "Output",
        help = "Suppress script output."
    )]
    pub quiet: bool,
}

/// `ied` — edit files in the sandbox, with buffered, previewable writes.
#[derive(Parser, Debug)]
#[command(
    name = "ied",
    version,
    about = "Edit structured files with a sandboxed Luau script",
    long_about = "Edit structured files with a sandboxed Luau script.\n\n\
        The script gets a virtual `fs` rooted at the working directory (see \
        -C). Writes are buffered rather than applied, and nothing reaches disk \
        unless you pass --write: by default a run is a preview, which is what \
        makes it safe to use as a linter or config check. Use --diff, \
        --list-changed and --exit-code to inspect the pending changes, in any \
        combination with --write.\n\n\
        Files are the only thing it can reach: there is no way to run \
        commands, open network connections, or read the environment. Use `ito` \
        for those, or `iq` for read-only queries.\n\n\
        An optional INPUT (a path, or '-' for stdin) is handed to the script \
        as the `input` global, raw and unparsed."
)]
pub struct IedCli {
    #[command(flatten)]
    pub common: CommonArgs,

    #[command(flatten)]
    pub input: InputArgs,

    #[command(flatten, next_help_heading = "Output")]
    pub report: ReportArgs,

    #[arg(
        short = 'q',
        long = "quiet",
        help_heading = "Output",
        display_order = 3,
        help = "Suppress script print() output."
    )]
    pub quiet: bool,
}

/// `ito` — the full toolset: direct writes plus commands, SSH, EC2 and tasks.
#[derive(Parser, Debug)]
#[command(
    name = "ito",
    version,
    about = "Run a Luau script with the full ito standard library",
    long_about = "Run a Luau script with the full ito standard library.\n\n\
        The script gets an `fs` over the real filesystem: relative paths \
        resolve against the working directory (see -C), absolute paths and \
        `..` reach outside it, and traversals report absolute paths. \
        Writes go to disk as they happen, not at the end: this run can also \
        spawn subprocesses, and buffering writes they cannot see would be a \
        promise that a single `process` call breaks. Because nothing is held \
        back there is nothing to preview, so the --write/--diff/--list-changed \
        flags do not exist here.\n\n\
        On top of what `ied` provides it can run local commands (proc), run \
        them remotely (ssh), query AWS EC2 and read from S3 (aws.ec2, aws.s3), \
        and overlap any of that concurrently (task).\n\n\
        An optional INPUT (a path, or '-' for stdin) is handed to the script \
        as the `input` global, raw and unparsed.\n\n\
        Arguments for the script go after a bare `::`. For a script that owns \
        its whole command line -- a `#!/usr/bin/env -S irun` script -- run it \
        with `irun`, which is this program with no flags of its own.\n\n\
        Use `ied` for buffered, previewable edits, or `iq` for read-only \
        queries."
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

    #[command(flatten)]
    pub input: InputArgs,

    #[arg(
        short = 'q',
        long = "quiet",
        help_heading = "Output",
        help = "Suppress script print() output."
    )]
    pub quiet: bool,
}

impl CommonArgs {
    /// The one invocation a binary with no flags can describe: a script path and
    /// nothing else.
    ///
    /// `irun` parses no arguments, but still goes through [`crate::run::Startup`]
    /// so script resolution (and its shebang handling) has one implementation
    /// rather than two. Building the struct here rather than at the call site
    /// keeps a new field from having to be spelled out in a binary that has no
    /// way to set it.
    pub fn script_only(script: String) -> Self {
        Self {
            verbose: 0,
            directory: Vec::new(),
            doc: None,
            doc_plain: false,
            doc_no_pager: false,
            completion: None,
            inline: false,
            require_root: None,
            script: Some(script),
        }
    }
}

pub(crate) struct LoadedScript {
    pub(crate) label: String,
    pub(crate) source: String,
}

impl IqCli {
    /// Reject argument combinations that cannot mean anything.
    pub fn validate(&self) -> Result<(), String> {
        // `-C` sets the `fs` root, so without `-f` there is no root to set.
        // Silently ignoring it would leave someone believing they had scoped a
        // query that in fact has no filesystem at all.
        if !self.common.directory.is_empty() && !self.filesystem {
            return Err(
                "-C sets the `fs` root, but `fs` is not exposed; pass -f/--fs to use it".into(),
            );
        }
        self.input.validate(&self.common)
    }
}

impl IedCli {
    /// Reject argument combinations that cannot mean anything.
    pub fn validate(&self) -> Result<(), String> {
        self.input.validate(&self.common)
    }
}

impl ItoCli {
    /// Reject argument combinations that cannot mean anything.
    pub fn validate(&self) -> Result<(), String> {
        self.input.validate(&self.common)
    }
}

/// Parsing that splits the script's own arguments off the command line first.
///
/// Everything after a bare `::` belongs to the script, so it is partitioned out
/// before clap sees anything — clap would reject those tokens as unknown flags,
/// and they are not ours to interpret in the first place.
///
/// Implemented once and blanket-applied, so no binary can forget the split or
/// do it differently.
pub trait ParseWithArgv: Parser + Sized {
    fn parse_with_argv() -> (Self, crate::scripting::Argv) {
        let raw: Vec<String> = std::env::args().collect();
        let (argv, clean) = crate::scripting::Argv::split(&raw[1..]);
        let cli = Self::parse_from(std::iter::once(raw[0].clone()).chain(clean));
        (cli, argv)
    }
}

impl<T: Parser> ParseWithArgv for T {}
