//! `cli` module: declare a command-line interface, then read it.
//!
//! A script declares what it accepts, calls [`parse`] once, and reads the
//! results back through the handles it kept:
//!
//! ```lua
//! cli.about("Copy files to a deployment target.")
//!
//! local env   = cli.opt("e/env=",    "deployment target")
//! local dry   = cli.opt("n/dry-run", "report without writing")
//! local host  = cli.arg("host",      "target machine")
//! local files = cli.arg("files...",  "files to copy")
//!
//! function main()
//!     cli.parse()
//!     cli.assert(env() ~= "foo", "foo is not a valid deployment target")
//!     if dry() then log.info("dry run") end
//!     for _, f in ipairs(files()) do ... end
//! end
//! ```
//!
//! `cli.assert` reports an invariant `cli.opt`/`cli.arg` cannot express by
//! themselves — a value out of range, two options that conflict — the same way
//! a malformed command line is reported: the message on stderr, the usage
//! block's summary beneath it, exit 2. It takes the condition already
//! evaluated plus an optional message, and does nothing when the condition
//! holds. The condition must be a **boolean** (`nil` fails); any other type
//! raises, so `cli.assert(env(), ...)` with a string is an error rather than a
//! pass. Unlike `cli.opt`/`cli.arg`, it is meant to run *after* `cli.parse()`,
//! since it checks parsed values — but nothing stops an earlier call.
//!
//! `cli.parse()` takes the script's own `argv`; `cli.parse(list)` parses the
//! given list of strings instead. A run that printed help or a usage error is
//! over: a `pcall` that swallows the exit gets the same exit again from every
//! later `cli.parse`.
//!
//! A handle is callable, and `h()` is exactly `h:get()` — the short form is the
//! one to reach for; the method spelling stays for the cases where a handle is
//! passed around and `:get()` says what it is. `h:count()` is the *other*
//! question: how many times it appeared, uniform across every kind of handle
//! (see [`Value::count`]).
//!
//! The declarations are *global state*, not a parser object: `cli.opt` and
//! `cli.arg` register into one shared registry that `cli.parse` consumes. That
//! is what lets a handle be a file-level local while the parse happens inside
//! `main`. The cost is that there is exactly one interface per run, so a
//! `require`d module calling `cli.opt` contributes to its caller's command line
//! — put declarations in the entry script (see `docs/src/modules/cli.md`).
//!
//! Reading a handle before `cli.parse()` raises, rather than returning a
//! plausible-looking default: a script that reads too early has a bug, and the
//! zero value would hide it.
//!
//! `cli.about` is optional prose describing the script, printed above the usage
//! line. It is the one part of the help block that cannot be derived from the
//! declarations.
//!
//! It takes up to two strings: a one-line `summary`, and optional `details`.
//! The summary appears on **every** surface — `-h`, `--help`, and a usage
//! error — and `details` is appended below it, after a blank line, on `--help`
//! alone. So the two spellings of help cannot describe the script differently:
//! `--help` is the summary plus more, never something else. That is why this is
//! two arguments rather than one string split on its first blank line, and why
//! the second is named for what it adds rather than for a "long" form it isn't:
//! there is no way to write a pair that disagrees with itself.
//!
//! ```lua
//! cli.about("Copy files to a deployment target.", [[
//! Reads a manifest, resolves each entry against the work directory,
//! and uploads what changed. Nothing is written without --commit.]])
//! ```
//!
//! A usage error prints the summary but not the details: someone who mistyped a
//! flag needs to know what the tool is and what shape it wanted, not a paragraph
//! wedged between the complaint and the usage that corrects it.
//!
//! # Option specs
//!
//! The spec string is fish `argparse`'s, minus the types it has no use for:
//!
//! | spec | meaning | `get()` | `count()` |
//! | --- | --- | --- | --- |
//! | `v/verbose` | flag | `true`/`false` | occurrences (`-vv` → 2) |
//! | `e/env=` | takes a value | string, `nil` when absent | 0 or 1 |
//! | `f/file=?` | optional value | string, `true` if bare, `nil` when absent | 0 or 1 |
//! | `I/include=+` | repeatable value | list of strings, `{}` when absent | elements |
//!
//! A flag reads as a **boolean**, because presence is what it asks and `0` is
//! truthy in Luau — `if force() then` on a count would always be taken. A
//! repeated flag is still counted, read with `:count()`, which keeps `-vv` as a
//! verbosity level without making the common case a trap.
//!
//! A `/` names the halves — whatever is on a side is that half, an empty side
//! is that half omitted — and with no `/` the *length* decides: `f` is short
//! (`-f`), `force` is long (`--force`). So `f/` and `/force` say explicitly
//! what the default would infer, `f/force` declares both, and `/f` is the
//! one-character *long* option the default cannot reach. A spec with nothing on
//! either side (`""`, `"/"`) is an error. Absence is always a value, never `nil` for
//! the collection forms, so `ipairs` and `:count() > 0` work unconditionally.
//!
//! A value option never swallows a following token that looks like a flag
//! (`--env --help` is a missing value; write `--env=--help` to pass one), but a
//! negative number (`-5`) is a value. `-e=` and `--env=` both mean an empty
//! value. A single-value option given twice is a usage error, not last-wins.
//!
//! # The options table
//!
//! The third argument of `cli.opt`/`cli.arg` is either the `details` string or
//! a table: `{details=, default=, required=, choices=, value_name=}`.
//! `required` (options only) makes absence a usage error; `default` is what an
//! absent value reads as (it does not count as an occurrence, and only an
//! optional argument may have one); `choices` restricts the accepted values;
//! `value_name` is the `<name>` the help shows. Flags take none of these.
//!
//! # Positional specs
//!
//! [`arg`] takes a name, with `?` marking it optional and `...` marking it the
//! rest. Registration order is argument order, and optionals may only follow
//! requireds: `REQ OPT REQ` is ambiguous, so it is a spec error rather than a
//! guess. `...` must come last, since nothing after it could ever match.
//!
//! # Errors
//!
//! Two kinds, kept apart because they have different audiences. A malformed
//! spec is the *script's* bug and raises immediately from `cli.opt`/`cli.arg`,
//! before any parsing. A malformed command line is the *user's*, and
//! `cli.parse` reports it with the usage block on stderr and exits 2.
//!
//! `--help` is handled by `cli.parse` too: usage to stdout, exit 0. The two
//! spellings are auto-registered **independently**, each unless the script
//! claimed that exact one: `cli.opt("/help")` takes `--help` for the script and
//! leaves `-h` printing the help. Whatever remains automatic can still reach the
//! whole of the prose — with `--help` claimed, the surviving `-h` prints the
//! details too rather than withholding them for a spelling that will never print
//! them, and its row names only the spelling it actually owns. Both exits leave through a
//! [`CliExit`] sentinel that the runner recognises — printing happens here,
//! because this is what knows whether the output is an answer or a complaint.
//!
//! Both are *terminal*, which is what lets every binary have this module even
//! though it writes to stdout: a run that prints help or a usage block unwinds
//! out of `eval` and so never reaches `emit_reports`/`flush_writes`
//! (`ied`'s diff and its buffered edits). There is
//! nothing for the output to interleave with, and nothing is committed.

use std::cell::RefCell;
use std::fmt;
use std::rc::Rc;

use mlua::{IntoLua, Lua, MetaMethod, UserData, UserDataMethods};

// ---------------------------------------------------------------------------
// The exit sentinel
// ---------------------------------------------------------------------------

/// A `cli.parse()` that printed and wants the process to stop.
///
/// Carries only the code: the text is already on the right stream by the time
/// this is raised, since only `cli` knows whether it wrote an answer (`--help`,
/// stdout) or a complaint (a usage error, stderr).
#[derive(Debug, Clone, Copy)]
pub struct CliExit {
    pub code: i32,
}

impl fmt::Display for CliExit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "cli.parse exited with {}", self.code)
    }
}

impl std::error::Error for CliExit {}

/// Recognise a [`CliExit`] anywhere in `e`'s cause chain.
///
/// mlua wraps an external error in `CallbackError` layers as it crosses the
/// Luau boundary — one per stack frame — so the sentinel is nested arbitrarily
/// deep by the time the host sees it.
pub fn exit_code(e: &mlua::Error) -> Option<i32> {
    match e {
        mlua::Error::ExternalError(err) => err.downcast_ref::<CliExit>().map(|x| x.code),
        mlua::Error::CallbackError { cause, .. } => exit_code(cause),
        mlua::Error::WithContext { cause, .. } => exit_code(cause),
        _ => None,
    }
}

pub(crate) fn exit_error(code: i32) -> mlua::Error {
    mlua::Error::external(CliExit { code })
}

// ---------------------------------------------------------------------------
// Specs
// ---------------------------------------------------------------------------

/// How many values an option takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Arity {
    /// A flag: no value, repeats counted.
    Flag,
    /// `=`: a value is required when the option appears.
    Required,
    /// `=?`: a value may follow, else `true`.
    Optional,
    /// `=+`: repeatable, each occurrence taking a value.
    Many,
}

#[derive(Debug, Clone)]
struct OptSpec {
    short: Option<char>,
    long: Option<String>,
    arity: Arity,
    description: Option<String>,
    /// Longer prose, shown only by `--help`, under this option's own heading
    /// rather than the summary table's row.
    details: Option<String>,
    /// How this spec takes part in the auto-registered help, if at all.
    help: Help,
    /// Default, required, choices and value name (the options-table form).
    extra: Extra,
}

/// What the options-table form of `cli.opt`/`cli.arg` adds to a declaration.
#[derive(Debug, Clone, Default)]
struct Extra {
    /// Read when the option/argument is absent.
    default: Option<String>,
    /// An option that must appear (`cli.opt` only).
    required: bool,
    /// The only values accepted; empty means any.
    choices: Vec<String>,
    /// The `<name>` shown for a value in the help (`cli.opt` only).
    value_name: Option<String>,
}

impl Extra {
    /// The bracketed notes appended to a help description.
    fn notes(&self) -> String {
        let mut s = String::new();
        if self.required {
            s.push_str(" [required]");
        }
        if !self.choices.is_empty() {
            s.push_str(&format!(" [choices: {}]", self.choices.join(", ")));
        }
        if let Some(d) = &self.default {
            s.push_str(&format!(" [default: {d}]"));
        }
        s
    }

    /// Check a supplied value against `choices`.
    fn check(&self, name: &str, value: &str) -> Result<(), String> {
        if self.choices.is_empty() || self.choices.iter().any(|c| c == value) {
            return Ok(());
        }
        Err(format!(
            "invalid value {value:?} for {name} (choices: {})",
            self.choices.join(", ")
        ))
    }
}

/// `description` followed by the notes, for the help rows and sections.
fn with_notes(description: Option<&str>, extra: &Extra) -> Option<String> {
    let notes = extra.notes();
    match (description, notes.is_empty()) {
        (None, true) => None,
        (Some(d), true) => Some(d.to_string()),
        (Some(d), false) => Some(format!("{d}{notes}")),
        (None, false) => Some(notes.trim_start().to_string()),
    }
}

/// Read the third argument of `cli.opt`/`cli.arg`: a `details` string, or an
/// options table (`details`, `default`, `required`, `choices`, `value_name`).
fn read_extras(
    value: Option<mlua::Value>,
    what: &str,
    positional: bool,
) -> mlua::Result<(Option<String>, Extra)> {
    let err = |m: String| mlua::Error::runtime(format!("{what}: {m}"));
    let table = match value {
        None | Some(mlua::Value::Nil) => return Ok((None, Extra::default())),
        Some(mlua::Value::String(s)) => {
            return Ok((Some(s.to_str()?.to_string()), Extra::default()));
        }
        Some(mlua::Value::Table(t)) => t,
        Some(other) => {
            return Err(err(format!(
                "expected a details string or an options table, got {}",
                other.type_name()
            )));
        }
    };
    let mut details = None;
    let mut extra = Extra::default();
    for pair in table.pairs::<mlua::Value, mlua::Value>() {
        let (key, value) = pair?;
        let mlua::Value::String(key) = &key else {
            return Err(err("the options table takes named keys only".to_string()));
        };
        let key = key.to_str()?.to_string();
        let text = |v: &mlua::Value| -> mlua::Result<String> {
            match v {
                mlua::Value::String(s) => Ok(s.to_str()?.to_string()),
                mlua::Value::Integer(_) | mlua::Value::Number(_) => Ok(v.to_string()?),
                other => Err(err(format!(
                    "{key} expects a string, got {}",
                    other.type_name()
                ))),
            }
        };
        match (key.as_str(), &value) {
            ("details", _) => details = Some(text(&value)?),
            ("default", _) => extra.default = Some(text(&value)?),
            ("required", mlua::Value::Boolean(b)) if !positional => extra.required = *b,
            ("required", _) if !positional => {
                return Err(err("required expects a boolean".to_string()));
            }
            ("value_name", _) if !positional => extra.value_name = Some(text(&value)?),
            ("choices", mlua::Value::Table(list)) => {
                for v in list.sequence_values::<mlua::Value>() {
                    extra.choices.push(text(&v?)?);
                }
            }
            ("choices", _) => return Err(err("choices expects a list of strings".to_string())),
            (other, _) => {
                return Err(err(format!("unknown key {other:?} in the options table")));
            }
        }
    }
    Ok((details, extra))
}

/// A spec's role in the auto-registered `-h`/`--help`.
///
/// The two spellings print different pages, so an occurrence has to be
/// attributable to the one that made it — which means two specs, not one
/// carrying both halves. This says how each of them renders, since two rows
/// reading `-h` and `--help` would be a worse account of one feature than the
/// single `-h, --help` row it replaced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Help {
    /// Not a help option: everything the script declared itself.
    No,
    /// Auto-registered, and the only spelling that was — because the script
    /// claimed the other one. An ordinary row naming just itself.
    Lone,
    /// Auto-registered as a pair. `Row` prints `-h, --help` and describes both;
    /// `Folded` is its other half, hidden so the pair reads as one row.
    Row,
    Folded,
}

impl OptSpec {
    /// The `-s, --long=` column of the help block.
    fn label(&self) -> String {
        let mut s = String::new();
        // The pair is one row, spelled as though it were one spec. `Lone` falls
        // through to the ordinary rendering, so a partially-claimed help names
        // only the spelling it actually owns.
        if self.help == Help::Row {
            return "-h, --help".to_string();
        }
        match (self.short, &self.long) {
            (Some(c), Some(l)) => {
                s.push_str(&format!("-{c}, --{l}"));
            }
            (Some(c), None) => s.push_str(&format!("-{c}")),
            (None, Some(l)) => s.push_str(&format!("    --{l}")),
            (None, None) => unreachable!("a spec with neither half is rejected at parse"),
        }
        let v = self.extra.value_name.as_deref().unwrap_or("value");
        match self.arity {
            Arity::Flag => {}
            Arity::Required => s.push_str(&format!("=<{v}>")),
            Arity::Optional => s.push_str(&format!("[=<{v}>]")),
            Arity::Many => s.push_str(&format!("=<{v}>...")),
        }
        s
    }

    /// How this option is named in an error message: its long form if it has
    /// one, since that is what a user is most likely to have typed.
    fn name(&self) -> String {
        match (&self.long, self.short) {
            (Some(l), _) => format!("--{l}"),
            (None, Some(c)) => format!("-{c}"),
            (None, None) => unreachable!("a spec with neither half is rejected at parse"),
        }
    }
}

/// Parse a spec string like `e/env=` into its parts.
///
/// Returns a message describing the mistake, for a script-facing raise.
fn parse_spec(spec: &str) -> Result<(Option<char>, Option<String>, Arity), String> {
    let (names, arity) = if let Some(head) = spec.strip_suffix("=?") {
        (head, Arity::Optional)
    } else if let Some(head) = spec.strip_suffix("=+") {
        (head, Arity::Many)
    } else if let Some(head) = spec.strip_suffix('=') {
        (head, Arity::Required)
    } else {
        (spec, Arity::Flag)
    };

    if names.contains('=') {
        return Err(format!(
            "bad option spec {spec:?}: `=` may only appear at the end, as `=`, `=?` or `=+`"
        ));
    }

    let (short, long) = match names.split_once('/') {
        Some((s, l)) => {
            // A `/` names the halves explicitly: whatever is on a side is that
            // half, and an empty side is that half omitted. So `/f` is a
            // one-character *long* option -- reachable, just not by accident.
            let short = match (s.chars().next(), s.chars().nth(1)) {
                (Some(c), None) => Some(c),
                (None, _) => None,
                _ => {
                    return Err(format!(
                        "bad option spec {spec:?}: the short name before `/` must be exactly one character"
                    ));
                }
            };
            let long = if l.is_empty() {
                None
            } else {
                Some(l.to_string())
            };
            if short.is_none() && long.is_none() {
                return Err(format!(
                    "bad option spec {spec:?}: an option needs a name on one side of the `/`"
                ));
            }
            (short, long)
        }
        // No `/`: length decides, because that is what the spelling can only
        // have meant. A one-character name is short (`f` is `-f` -- nobody
        // writes `--f`), anything longer is long (`force` is `--force`). `f/`
        // still says short-only explicitly, and is now merely redundant.
        None => {
            let mut chars = names.chars();
            match (chars.next(), chars.next()) {
                (None, _) => Err("bad option spec \"\": an option needs a name".to_string())?,
                (Some(c), None) => (Some(c), None),
                _ => (None, Some(names.to_string())),
            }
        }
    };

    if let Some(c) = short
        && !c.is_ascii_alphanumeric()
    {
        return Err(format!(
            "bad option spec {spec:?}: the short name must be a letter or digit"
        ));
    }
    if let Some(l) = &long
        && !l
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(format!(
            "bad option spec {spec:?}: a long name may only contain letters, digits, `-` and `_`"
        ));
    }

    Ok((short, long, arity))
}

/// How many values a positional takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArgKind {
    Required,
    /// `?`: may be absent.
    Optional,
    /// `...`: everything that is left.
    Rest,
}

#[derive(Debug, Clone)]
struct ArgSpec {
    name: String,
    kind: ArgKind,
    description: Option<String>,
    /// Longer prose, shown only by `--help`, under this argument's own
    /// heading rather than the summary table's row.
    details: Option<String>,
    /// `default` and `choices` from the options-table form.
    extra: Extra,
}

impl ArgSpec {
    /// The `<name>` / `[name]` / `[name...]` form used in the usage line.
    fn label(&self) -> String {
        match self.kind {
            ArgKind::Required => format!("<{}>", self.name),
            ArgKind::Optional => format!("[{}]", self.name),
            ArgKind::Rest => format!("[{}...]", self.name),
        }
    }
}

fn parse_arg_name(name: &str) -> Result<(String, ArgKind), String> {
    let (bare, kind) = if let Some(head) = name.strip_suffix("...") {
        (head, ArgKind::Rest)
    } else if let Some(head) = name.strip_suffix('?') {
        (head, ArgKind::Optional)
    } else {
        (name, ArgKind::Required)
    };

    if bare.is_empty() {
        return Err(format!(
            "bad argument name {name:?}: an argument needs a name"
        ));
    }
    if !bare
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(format!(
            "bad argument name {name:?}: only letters, digits, `-` and `_` are allowed"
        ));
    }

    Ok((bare.to_string(), kind))
}

// ---------------------------------------------------------------------------
// Parsed values
// ---------------------------------------------------------------------------

/// What a handle yields once `cli.parse` has run.
#[derive(Debug, Clone)]
enum Value {
    /// A flag, carrying how many times it appeared.
    ///
    /// The count is kept even though reading yields a boolean: `:count()` is
    /// where `-vv` is legible, and presence is the question `if verbose()`
    /// asks. Storing only a `bool` would make the two disagree.
    Flag(u32),
    /// A value-taking option: absent, bare (`=?` with nothing after it), or set.
    Absent,
    Bare,
    One(String),
    /// Absent, but the declaration's `default` stands in: reads as that value
    /// and does not count as an occurrence.
    Default(String),
    /// A `=+` option's accumulated values, or a `...` positional's.
    List(Vec<String>),
}

impl Value {
    /// How many times this appeared on the command line.
    ///
    /// One question with one answer for every kind of handle, which is what
    /// makes it worth having beside the value: occurrences for a flag, elements
    /// for a list (`=+` or `...`), and 0-or-1 for anything that holds a single
    /// value. So `files:count() == 0` and `#files() == 0` agree, and a flag's
    /// `-vv` stays readable after presence became a boolean.
    fn count(&self) -> u32 {
        match self {
            Value::Flag(n) => *n,
            Value::Absent | Value::Default(_) => 0,
            Value::Bare | Value::One(_) => 1,
            Value::List(items) => items.len() as u32,
        }
    }
}

impl mlua::IntoLua for Value {
    fn into_lua(self, lua: &Lua) -> mlua::Result<mlua::Value> {
        match self {
            Value::Flag(n) => Ok(mlua::Value::Boolean(n > 0)),
            Value::Absent => Ok(mlua::Value::Nil),
            Value::Bare => Ok(mlua::Value::Boolean(true)),
            Value::One(s) | Value::Default(s) => lua.create_string(&s).map(mlua::Value::String),
            Value::List(items) => lua.create_sequence_from(items).map(mlua::Value::Table),
        }
    }
}

// ---------------------------------------------------------------------------
// The registry
// ---------------------------------------------------------------------------

/// The shared state behind the whole module: what was declared, and — after
/// [`parse`] — what the command line said about it.
#[derive(Default)]
struct Registry {
    opts: Vec<OptSpec>,
    args: Vec<ArgSpec>,
    /// Parallel to `opts`/`args`; empty until parsed.
    opt_values: Vec<Value>,
    arg_values: Vec<Value>,
    parsed: bool,
    /// Set once `cli.parse`/`cli.assert` has printed and exited, so a `pcall`
    /// that swallowed the sentinel cannot get a second, different parse: every
    /// later `cli.parse` re-raises the same exit.
    exit: Option<i32>,
    /// What the script is for, shown above the usage line; set by `cli.about`.
    /// One line's worth: it heads `-h`, `--help` and a usage error alike.
    summary: Option<String>,
    /// The rest of the prose, shown only by `--help`, always *below* `summary`
    /// rather than instead of it; set by `cli.about`'s second argument.
    details: Option<String>,
    /// The script's own arguments, captured at registration so `cli.parse()`
    /// needs no argument.
    argv: Vec<String>,
}

type Shared = Rc<RefCell<Registry>>;

/// A handle to one declared option or argument.
///
/// Deliberately opaque: it carries an index into the registry rather than a
/// value, because the value does not exist yet when the handle is created.
struct Handle {
    registry: Shared,
    index: usize,
    positional: bool,
}

impl Handle {
    fn get(&self) -> mlua::Result<Value> {
        let reg = self.registry.borrow();
        if !reg.parsed {
            let what = if self.positional {
                reg.args[self.index].name.clone()
            } else {
                reg.opts[self.index].name()
            };
            return Err(mlua::Error::runtime(format!(
                "{what} read before cli.parse() -- declare with cli.opt/cli.arg, \
                 then call cli.parse() before reading"
            )));
        }
        Ok(if self.positional {
            reg.arg_values[self.index].clone()
        } else {
            reg.opt_values[self.index].clone()
        })
    }
}

impl UserData for Handle {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("get", |lua, this, ()| this.get()?.into_lua(lua));
        // Not a second way to read the value, but a different question about it
        // -- and the only way to see `-vv` now that a flag reads as a boolean.
        methods.add_method("count", |_, this, ()| Ok(this.get()?.count()));
        // Calling the handle is the same read: `env()` rather than `env:get()`,
        // which is what a handle mostly gets used for. `:get()` stays, since it
        // is the spelling that reads unambiguously when the handle is passed
        // around rather than read at its declaration site.
        methods.add_meta_method(MetaMethod::Call, |lua, this, ()| this.get()?.into_lua(lua));
    }
}

// ---------------------------------------------------------------------------
// Help
// ---------------------------------------------------------------------------

/// How much of the prose a rendering of the usage block carries.
///
/// The summary is in *both*, which is the whole point of the split: `--help` is
/// the summary plus more, never something else. Only `details` is ever withheld.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Detail {
    /// `-h`, and a usage error: the one-line summary and the shape.
    Summary,
    /// `--help`: the summary, a blank line, then the details.
    Full,
}

/// The usage block.
///
/// No program name anywhere: under a shebang the binary's `argv[0]` is the
/// interpreter, not the script, and a name that is wrong half the time is worse
/// than no name at all. The reader already knows what they typed.
fn usage(reg: &Registry, detail: Detail) -> String {
    let mut out = String::new();

    // The prose comes first, before the shape: someone reading this wants to
    // know what the script does before how to call it. `details` is joined to
    // `summary` by a blank line and never appears without it, so the two
    // spellings of help cannot describe the script differently -- `--help` only
    // ever says more.
    if let Some(summary) = reg.summary.as_deref() {
        out.push_str(summary.trim_end());
        out.push_str("\n\n");
        if let Some(details) = reg.details.as_deref().filter(|_| detail == Detail::Full) {
            out.push_str(details.trim_end());
            out.push_str("\n\n");
        }
    }

    let shape: Vec<String> = reg.args.iter().map(ArgSpec::label).collect();
    out.push_str("Usage:");
    if !reg.opts.is_empty() {
        out.push_str(" [options]");
    }
    for s in &shape {
        out.push(' ');
        out.push_str(s);
    }
    out.push('\n');

    // The auto-help pair is two specs but one row; the other half is skipped
    // everywhere, including the width, so it cannot pad the column either.
    let shown = || reg.opts.iter().filter(|o| o.help != Help::Folded);

    // `--help` gives each declaration its own section, headed by its label
    // and carrying `details` below the one-line `description` -- so the extra
    // prose has somewhere to go that isn't cramped into a table column.
    // `-h` and a usage error keep the compact table, since neither prints
    // `details` and a heading-per-row would just be a table with more
    // whitespace.
    if detail == Detail::Full {
        if !reg.args.is_empty() {
            out.push_str("\nPositionals:\n\n");
            for a in &reg.args {
                let name = if a.kind == ArgKind::Rest {
                    format!("{}...", a.name)
                } else {
                    a.name.clone()
                };
                out.push_str(&render_section(
                    &name,
                    with_notes(a.description.as_deref(), &a.extra).as_deref(),
                    a.details.as_deref(),
                ));
            }
        }

        if !reg.opts.is_empty() {
            out.push_str("\nOptions:\n\n");
            for o in shown() {
                out.push_str(&render_section(
                    &o.label(),
                    with_notes(o.description.as_deref(), &o.extra).as_deref(),
                    o.details.as_deref(),
                ));
            }
        }

        // Each section ends with its own blank-line separator; drop the last
        // one so the block ends on a single trailing newline like the table
        // form does.
        while out.ends_with("\n\n") {
            out.pop();
        }
        return out;
    }

    // Two columns, aligned across both sections so they read as one table.
    let width = reg
        .args
        .iter()
        .map(|a| a.name.len() + if a.kind == ArgKind::Rest { 3 } else { 0 })
        .chain(shown().map(|o| o.label().len()))
        .max()
        .unwrap_or(0);

    if !reg.args.is_empty() {
        out.push_str("\nPositionals:\n");
        for a in &reg.args {
            let name = if a.kind == ArgKind::Rest {
                format!("{}...", a.name)
            } else {
                a.name.clone()
            };
            out.push_str(&render_row(
                &name,
                width,
                with_notes(a.description.as_deref(), &a.extra).as_deref(),
            ));
        }
    }

    if !reg.opts.is_empty() {
        out.push_str("\nOptions:\n");
        for o in shown() {
            out.push_str(&render_row(
                &o.label(),
                width,
                with_notes(o.description.as_deref(), &o.extra).as_deref(),
            ));
        }
    }

    out
}

fn render_row(label: &str, width: usize, description: Option<&str>) -> String {
    match description {
        Some(d) => format!("  {label:<width$}  {d}\n"),
        None => format!("  {label}\n"),
    }
}

/// One `--help` section: a heading, the one-line description, and — if the
/// declaration carried one — its longer `details`, indented below.
fn render_section(label: &str, description: Option<&str>, details: Option<&str>) -> String {
    let mut out = format!("{label}\n");
    if let Some(d) = description {
        out.push_str(&format!("  {d}\n"));
    }
    if let Some(d) = details {
        out.push('\n');
        for line in d.trim_end().lines() {
            if line.is_empty() {
                out.push('\n');
            } else {
                out.push_str(&format!("  {line}\n"));
            }
        }
    }
    out.push('\n');
    out
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

/// Find the spec matching a long name.
fn find_long<'a>(opts: &'a [OptSpec], name: &str) -> Option<(usize, &'a OptSpec)> {
    opts.iter()
        .enumerate()
        .find(|(_, o)| o.long.as_deref() == Some(name))
}

fn find_short(opts: &[OptSpec], c: char) -> Option<(usize, &OptSpec)> {
    opts.iter().enumerate().find(|(_, o)| o.short == Some(c))
}

/// Record one occurrence of `index`, with `value` if one was supplied.
fn record(
    values: &mut [Value],
    spec: &OptSpec,
    index: usize,
    value: Option<String>,
) -> Result<(), String> {
    match spec.arity {
        Arity::Flag => {
            if value.is_some() {
                return Err(format!("{} takes no value", spec.name()));
            }
            let Value::Flag(n) = values[index] else {
                unreachable!("a flag's slot is always a flag")
            };
            values[index] = Value::Flag(n + 1);
        }
        Arity::Required => {
            let Some(v) = value else {
                return Err(format!("{} requires a value", spec.name()));
            };
            spec.extra.check(&spec.name(), &v)?;
            // Silently last-wins would make `:count()` undercount and hide a
            // typo'd command line.
            if !matches!(values[index], Value::Absent) {
                return Err(format!("{} given more than once", spec.name()));
            }
            values[index] = Value::One(v);
        }
        Arity::Optional => {
            if !matches!(values[index], Value::Absent) {
                return Err(format!("{} given more than once", spec.name()));
            }
            values[index] = match value {
                Some(v) => {
                    spec.extra.check(&spec.name(), &v)?;
                    Value::One(v)
                }
                None => Value::Bare,
            };
        }
        Arity::Many => {
            let Some(v) = value else {
                return Err(format!("{} requires a value", spec.name()));
            };
            spec.extra.check(&spec.name(), &v)?;
            let Value::List(items) = &mut values[index] else {
                unreachable!("a repeatable option's slot is always a list")
            };
            items.push(v);
        }
    }
    Ok(())
}

/// Scan the command line for declared options, collecting what is left over.
///
/// Stops short of matching positionals, so a caller can answer `--help` before
/// complaining about a missing argument: a script with a required positional
/// must still be able to explain itself.
///
/// Returns the message for a usage error; the caller decides how to report it.
fn scan_options(reg: &mut Registry) -> Result<Vec<String>, String> {
    let mut opt_values: Vec<Value> = reg
        .opts
        .iter()
        .map(|o| match o.arity {
            Arity::Flag => Value::Flag(0),
            Arity::Many => Value::List(Vec::new()),
            _ => Value::Absent,
        })
        .collect();

    let mut positionals: Vec<String> = Vec::new();
    let argv = reg.argv.clone();
    let mut it = argv.into_iter().peekable();
    // A negative number is a positional unless the script declared a digit as
    // a short option (`-1`), in which case the option wins.
    let digit_short = reg
        .opts
        .iter()
        .any(|o| o.short.is_some_and(|c| c.is_ascii_digit()));
    // After `--` every remaining token is positional, however it is spelled.
    let mut only_positional = false;

    while let Some(token) = it.next() {
        if only_positional {
            positionals.push(token);
            continue;
        }
        if token == "--" {
            only_positional = true;
            continue;
        }

        if let Some(body) = token.strip_prefix("--") {
            let (name, inline) = match body.split_once('=') {
                Some((n, v)) => (n, Some(v.to_string())),
                None => (body, None),
            };
            let Some((index, spec)) = find_long(&reg.opts, name) else {
                // Quoted, because unlike the messages naming a *declared*
                // option this echoes what the user typed -- and unquoted, an
                // empty or space-bearing name would vanish into the sentence.
                return Err(match suggest(&reg.opts, name) {
                    Some(s) => format!("unknown option \"--{name}\" (did you mean \"--{s}\"?)"),
                    None => format!("unknown option \"--{name}\""),
                });
            };
            let spec = spec.clone();
            // A required value may be written `--env=prod` or `--env prod`; only
            // the latter needs to reach forward, and only when nothing was
            // attached. It never swallows something that looks like a flag:
            // `--env --help` is a missing value, `--env=--help` passes it.
            let value = match (inline, spec.arity) {
                (Some(v), _) => Some(v),
                (None, Arity::Required | Arity::Many) => next_value(&mut it, digit_short),
                (None, _) => None,
            };
            record(&mut opt_values, &spec, index, value)?;
            continue;
        }

        // A lone `-` is a conventional stand-in for stdin, so it is a value.
        if token.len() > 1
            && !(is_negative_number(&token) && !digit_short)
            && let Some(body) = token.strip_prefix('-')
        {
            let mut chars = body.chars().peekable();
            while let Some(c) = chars.next() {
                let Some((index, spec)) = find_short(&reg.opts, c) else {
                    return Err(format!("unknown option \"-{c}\""));
                };
                let spec = spec.clone();
                let value = match spec.arity {
                    // The rest of the cluster is the value if there is any
                    // (`-eprod`, `-e=prod`), else the next argument. `-e=`
                    // means an empty value, like `--env=`.
                    Arity::Required | Arity::Many => {
                        let rest: String = chars.by_ref().collect();
                        match rest.strip_prefix('=') {
                            Some(v) => Some(v.to_string()),
                            None if rest.is_empty() => next_value(&mut it, digit_short),
                            None => Some(rest),
                        }
                    }
                    Arity::Optional => {
                        let rest: String = chars.by_ref().collect();
                        match rest.strip_prefix('=') {
                            Some(v) => Some(v.to_string()),
                            None if rest.is_empty() => None,
                            None => Some(rest),
                        }
                    }
                    Arity::Flag => None,
                };
                record(&mut opt_values, &spec, index, value)?;
            }
            continue;
        }

        positionals.push(token);
    }

    // Absent options: a required one is an error, a defaulted one reads as its
    // default (without counting as an occurrence).
    for (spec, value) in reg.opts.iter().zip(opt_values.iter_mut()) {
        if matches!(value, Value::Absent) {
            if spec.extra.required {
                return Err(format!("missing required option {}", spec.name()));
            }
            if let Some(d) = &spec.extra.default {
                *value = Value::Default(d.clone());
            }
        }
    }

    // Options are settled; `--help` can be answered from here on.
    reg.opt_values = opt_values;
    Ok(positionals)
}

/// `-5`, `-1.5`, `-.5`: a negative number, which is a value rather than a flag.
fn is_negative_number(token: &str) -> bool {
    token
        .strip_prefix('-')
        .is_some_and(|r| r.starts_with(|c: char| c.is_ascii_digit() || c == '.'))
        && token.parse::<f64>().is_ok()
}

/// The token after an option that takes a value, unless it looks like a flag
/// (so `--env --help` does not eat `--help`).
fn next_value(
    it: &mut std::iter::Peekable<std::vec::IntoIter<String>>,
    digit_short: bool,
) -> Option<String> {
    let t = it.peek()?;
    let flag_like = t.starts_with('-') && t.len() > 1 && !(is_negative_number(t) && !digit_short);
    if flag_like { None } else { it.next() }
}

/// The declared long option closest to a mistyped `name`, if any is near.
fn suggest(opts: &[OptSpec], name: &str) -> Option<String> {
    let distance = |a: &str, b: &str| {
        let b: Vec<char> = b.chars().collect();
        let mut prev: Vec<usize> = (0..=b.len()).collect();
        for (i, ca) in a.chars().enumerate() {
            let mut cur = vec![i + 1];
            for (j, cb) in b.iter().enumerate() {
                let sub = prev[j] + usize::from(ca != *cb);
                cur.push(sub.min(prev[j + 1] + 1).min(cur[j] + 1));
            }
            prev = cur;
        }
        prev[b.len()]
    };
    opts.iter()
        .filter_map(|o| o.long.as_deref())
        .map(|l| (distance(name, l), l))
        .filter(|(d, l)| *d <= 2 && *d < l.len().max(2))
        .min_by_key(|(d, _)| *d)
        .map(|(_, l)| l.to_string())
}

/// Assign the leftover tokens to the declared positionals.
///
/// Runs only once `--help` has been ruled out, so its complaints are about a
/// command line someone meant to have executed.
fn match_positionals(reg: &mut Registry, positionals: Vec<String>) -> Result<(), String> {
    // Positionals are matched by declaration order. Requireds come first (the
    // registration check guarantees it), so a straight walk assigns correctly:
    // each required takes one, each optional takes one if any are left, and a
    // trailing `...` takes the remainder.
    let mut arg_values: Vec<Value> = Vec::with_capacity(reg.args.len());
    let mut rest = positionals.into_iter();
    for spec in &reg.args {
        let label = format!("<{}>", spec.name);
        match spec.kind {
            ArgKind::Required => match rest.next() {
                Some(v) => {
                    spec.extra.check(&label, &v)?;
                    arg_values.push(Value::One(v));
                }
                None => return Err(format!("missing required argument <{}>", spec.name)),
            },
            ArgKind::Optional => arg_values.push(match rest.next() {
                Some(v) => {
                    spec.extra.check(&label, &v)?;
                    Value::One(v)
                }
                None => match &spec.extra.default {
                    Some(d) => Value::Default(d.clone()),
                    None => Value::Absent,
                },
            }),
            ArgKind::Rest => {
                let items: Vec<String> = rest.by_ref().collect();
                for v in &items {
                    spec.extra.check(&label, v)?;
                }
                arg_values.push(Value::List(items));
            }
        }
    }

    let leftover: Vec<String> = rest.collect();
    if !leftover.is_empty() {
        return Err(format!(
            "unexpected argument{} {}",
            if leftover.len() == 1 { "" } else { "s" },
            leftover
                .iter()
                .map(|s| format!("{s:?}"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    reg.arg_values = arg_values;
    reg.parsed = true;
    Ok(())
}

/// Report a complaint about the command line, and remember that the run is over.
///
/// The usage block, on stderr, exit 2. Straight there rather than through
/// `tracing`, because this is the program explaining its own interface, not a
/// log line. Summary only: someone who mistyped a flag wants the complaint next
/// to the shape that corrects it -- the one-line summary says what the tool is,
/// but the details would wedge a paragraph between the error and its usage.
fn reject(r: &mut Registry, message: String) -> mlua::Error {
    use std::io::Write;

    let text = usage(r, Detail::Summary);
    // Point at the help the script still has, if any.
    let hint = r
        .opts
        .iter()
        .find(|o| matches!(o.help, Help::Row | Help::Lone) && o.long.is_some())
        .map(|_| "--help")
        .or_else(|| {
            r.opts
                .iter()
                .any(|o| matches!(o.help, Help::Lone | Help::Folded) && o.short == Some('h'))
                .then_some("-h")
        })
        .map(|h| format!("\nFor more information, try '{h}'.\n"))
        .unwrap_or_default();
    let mut err = std::io::stderr().lock();
    let _ = write!(err, "error: {message}\n\n{text}{hint}");
    let _ = err.flush();
    r.exit = Some(2);
    exit_error(2)
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

/// Register the `cli` module on `lua`, over the script's own `argv`.
pub fn register(lua: &Lua, argv: &[String]) -> mlua::Result<()> {
    let registry: Shared = Rc::new(RefCell::new(Registry {
        argv: argv.to_vec(),
        ..Registry::default()
    }));
    let module = lua.create_table()?;

    let reg = registry.clone();
    module.set(
        "about",
        lua.create_function(move |_, (summary, details): (String, Option<String>)| {
            let mut r = reg.borrow_mut();
            if r.parsed {
                return Err(mlua::Error::runtime(
                    "cli.about called after cli.parse() -- the usage block is already rendered",
                ));
            }
            // Last call wins rather than raising: unlike a duplicate option there
            // is nothing ambiguous about it. (A module `require`d *after* the
            // entry's own call therefore overrides it; call it last if that
            // matters.) Both halves move together --
            // a one-argument call clears any details a previous call set, since
            // otherwise the survivor would head prose it was never written for.
            r.summary = Some(summary);
            r.details = details;
            Ok(())
        })?,
    )?;

    let reg = registry.clone();
    module.set(
        "opt",
        lua.create_function(
            move |_, (spec, description, third): (String, Option<String>, Option<mlua::Value>)| {
                let (short, long, arity) = parse_spec(&spec).map_err(mlua::Error::runtime)?;
                let (details, extra) = read_extras(third, "cli.opt", false)?;
                let bad = |m: &str| mlua::Error::runtime(format!("cli.opt {spec:?}: {m}"));
                if arity == Arity::Flag
                    && (extra.required || extra.default.is_some() || !extra.choices.is_empty())
                {
                    return Err(bad("a flag takes no default, required or choices"));
                }
                if arity == Arity::Many && extra.default.is_some() {
                    return Err(bad("a repeatable option takes no default"));
                }
                if extra.required && extra.default.is_some() {
                    return Err(bad("required and default contradict each other"));
                }
                if let Some(d) = &extra.default
                    && !extra.choices.is_empty()
                    && !extra.choices.contains(d)
                {
                    return Err(bad("the default is not one of the choices"));
                }
                let mut r = reg.borrow_mut();
                if r.parsed {
                    return Err(mlua::Error::runtime(
                        "cli.opt called after cli.parse() -- declare every option first",
                    ));
                }
                // A duplicate is a mistake in the script, and silently letting one
                // win would make which one depend on file order.
                if let Some(c) = short
                    && find_short(&r.opts, c).is_some()
                {
                    return Err(mlua::Error::runtime(format!(
                        "option -{c} is already declared"
                    )));
                }
                if let Some(l) = &long
                    && find_long(&r.opts, l).is_some()
                {
                    return Err(mlua::Error::runtime(format!(
                        "option --{l} is already declared"
                    )));
                }
                r.opts.push(OptSpec {
                    short,
                    long,
                    arity,
                    description,
                    details,
                    help: Help::No,
                    extra,
                });
                let index = r.opts.len() - 1;
                drop(r);
                Ok(Handle {
                    registry: reg.clone(),
                    index,
                    positional: false,
                })
            },
        )?,
    )?;

    let reg = registry.clone();
    module.set(
        "arg",
        lua.create_function(
            move |_, (name, description, third): (String, Option<String>, Option<mlua::Value>)| {
                let (bare, kind) = parse_arg_name(&name).map_err(mlua::Error::runtime)?;
                let (details, extra) = read_extras(third, "cli.arg", true)?;
                if extra.default.is_some() && kind != ArgKind::Optional {
                    return Err(mlua::Error::runtime(format!(
                        "cli.arg {name:?}: only an optional argument (`{bare}?`) takes a default"
                    )));
                }
                if let Some(d) = &extra.default
                    && !extra.choices.is_empty()
                    && !extra.choices.contains(d)
                {
                    return Err(mlua::Error::runtime(format!(
                        "cli.arg {name:?}: the default is not one of the choices"
                    )));
                }
                let mut r = reg.borrow_mut();
                if r.parsed {
                    return Err(mlua::Error::runtime(
                        "cli.arg called after cli.parse() -- declare every argument first",
                    ));
                }
                if r.args.iter().any(|a| a.name == bare) {
                    return Err(mlua::Error::runtime(format!(
                        "argument <{bare}> is already declared"
                    )));
                }
                // Ordering rules, checked at declaration so the mistake is reported
                // at the line that made it. Anything after a `...` is unreachable,
                // and a required after an optional cannot be assigned unambiguously.
                if let Some(last) = r.args.last() {
                    if last.kind == ArgKind::Rest {
                        return Err(mlua::Error::runtime(format!(
                            "argument <{bare}> follows <{}...>, which already takes every \
                         remaining argument",
                            last.name
                        )));
                    }
                    if kind == ArgKind::Required && last.kind == ArgKind::Optional {
                        return Err(mlua::Error::runtime(format!(
                            "required argument <{bare}> follows optional [{}] -- optional \
                         arguments must come last",
                            last.name
                        )));
                    }
                }
                r.args.push(ArgSpec {
                    name: bare,
                    kind,
                    description,
                    details,
                    extra,
                });
                let index = r.args.len() - 1;
                drop(r);
                Ok(Handle {
                    registry: reg.clone(),
                    index,
                    positional: true,
                })
            },
        )?,
    )?;

    let reg = registry.clone();
    module.set(
        "parse",
        // An optional list of tokens to parse instead of the script's `argv`.
        lua.create_function(move |_, tokens: Option<Vec<String>>| {
            let mut r = reg.borrow_mut();
            // A `pcall` may have swallowed an earlier exit: re-raise it rather
            // than parsing a half-consumed registry a second time.
            if let Some(code) = r.exit {
                return Err(exit_error(code));
            }
            if r.parsed {
                return Err(mlua::Error::runtime("cli.parse() called twice"));
            }
            if let Some(tokens) = tokens {
                r.argv = tokens;
            }

            // `-h`/`--help` unless the script wanted those spellings for itself,
            // in which case it owns them and we add nothing.
            //
            // Two *separate* specs rather than one carrying both spellings,
            // because they no longer print the same page: an occurrence has to
            // be attributable to the spelling that made it, and one slot cannot
            // say which half was typed. They are rendered as one row all the
            // same (see `label`), so the split does not reach the reader.
            let want_short = find_short(&r.opts, 'h').is_none();
            let want_long = find_long(&r.opts, "help").is_none();
            let both = want_short && want_long;
            // Read before the pushes below borrow `r.opts` mutably.
            let has_details = r.details.is_some();
            // Whichever spellings were added, between them they must be able to
            // reach every part of the prose: `-h` is the summary only when a
            // `--help` exists to carry the details. Claim `--help` for the
            // script and the auto `-h` becomes the whole help, not half of it.
            let short_detail = if both { Detail::Summary } else { Detail::Full };
            // Only worth pointing at a fuller page when there is one to point
            // at: with no details, or with only one spelling registered, both
            // print the same thing.
            let split = both && has_details;
            let short_index = want_short.then(|| {
                r.opts.push(OptSpec {
                    short: Some('h'),
                    long: None,
                    arity: Arity::Flag,
                    description: Some(
                        if split {
                            "show a summary (--help for more)"
                        } else {
                            "show this help"
                        }
                        .to_string(),
                    ),
                    details: None,
                    // When `--help` is there too it carries the row; alone (the
                    // script claimed `--help` itself) this one is the row.
                    help: if both { Help::Folded } else { Help::Lone },
                    extra: Extra::default(),
                });
                r.opts.len() - 1
            });
            let long_index = want_long.then(|| {
                r.opts.push(OptSpec {
                    short: None,
                    long: Some("help".to_string()),
                    arity: Arity::Flag,
                    description: Some(
                        if split {
                            "show help (-h for a summary)"
                        } else {
                            "show this help"
                        }
                        .to_string(),
                    ),
                    details: None,
                    // Only names `-h` in its label when it actually owns it.
                    help: if both { Help::Row } else { Help::Lone },
                    extra: Extra::default(),
                });
                r.opts.len() - 1
            });

            // The help a mistyped command line can still ask for: a usage error
            // must not hide `--help`, so this looks at the raw tokens.
            let help_asked = |r: &Registry| {
                let tokens = r.argv.iter().take_while(|t| t.as_str() != "--");
                let mut found = None;
                for t in tokens {
                    if t == "--help" && long_index.is_some() {
                        return Some(Detail::Full);
                    }
                    if t == "-h" && short_index.is_some() {
                        found = Some(short_detail);
                    }
                }
                found
            };
            let show_help = |r: &mut Registry, detail: Detail| {
                use std::io::Write;
                let text = usage(r, detail);
                let mut out = std::io::stdout().lock();
                let _ = write!(out, "{text}");
                let _ = out.flush();
                r.exit = Some(0);
                exit_error(0)
            };

            let positionals = match scan_options(&mut r) {
                Ok(rest) => rest,
                Err(message) => {
                    if let Some(detail) = help_asked(&r) {
                        return Err(show_help(&mut r, detail));
                    }
                    return Err(reject(&mut r, message));
                }
            };

            // Before the positionals are matched, so `--help` works on a script
            // whose arguments are required -- which is exactly the script whose
            // help someone needs.
            // `--help` is checked first: given both, the fuller page is the one
            // that was asked for.
            let asked = |i: Option<usize>| i.is_some_and(|i| r.opt_values[i].count() > 0);
            let detail = if asked(long_index) {
                Some(Detail::Full)
            } else if asked(short_index) {
                Some(short_detail)
            } else {
                None
            };
            if let Some(detail) = detail {
                return Err(show_help(&mut r, detail));
            }

            match match_positionals(&mut r, positionals) {
                Ok(()) => Ok(()),
                Err(message) => Err(reject(&mut r, message)),
            }
        })?,
    )?;

    let reg = registry.clone();
    module.set(
        "assert",
        // Luau decides truthiness, not this function: a condition must be a
        // boolean (or nil, which fails), so `cli.assert("x", ...)` is a bug
        // rather than a pass.
        lua.create_function(move |_, (ok, message): (mlua::Value, Option<String>)| {
            match ok {
                mlua::Value::Boolean(true) => return Ok(()),
                mlua::Value::Boolean(false) | mlua::Value::Nil => {}
                other => {
                    return Err(mlua::Error::runtime(format!(
                        "cli.assert expects a boolean condition, got {}",
                        other.type_name()
                    )));
                }
            }
            let mut r = reg.borrow_mut();
            let message = message.unwrap_or_else(|| "assertion failed".to_string());
            Err(reject(&mut r, message))
        })?,
    )?;

    lua.globals().set("cli", module)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Lua with `cli` registered over `argv`.
    fn lua(argv: &[&str]) -> Lua {
        let lua = Lua::new();
        let argv: Vec<String> = argv.iter().map(|s| s.to_string()).collect();
        register(&lua, &argv).expect("register");
        lua
    }

    /// Evaluate `src` and return what it produced, as a string.
    fn eval(argv: &[&str], src: &str) -> Result<String, mlua::Error> {
        lua(argv).load(src).eval::<String>()
    }

    #[test]
    fn spec_forms_parse() {
        assert_eq!(
            parse_spec("v/verbose").unwrap(),
            (Some('v'), Some("verbose".into()), Arity::Flag)
        );
        assert_eq!(
            parse_spec("e/env=").unwrap(),
            (Some('e'), Some("env".into()), Arity::Required)
        );
        assert_eq!(
            parse_spec("f/file=?").unwrap(),
            (Some('f'), Some("file".into()), Arity::Optional)
        );
        assert_eq!(
            parse_spec("I/include=+").unwrap(),
            (Some('I'), Some("include".into()), Arity::Many)
        );
        assert_eq!(
            parse_spec("long-only").unwrap(),
            (None, Some("long-only".into()), Arity::Flag)
        );
        assert_eq!(parse_spec("x/").unwrap(), (Some('x'), None, Arity::Flag));
    }

    #[test]
    fn a_bare_name_is_short_or_long_by_length() {
        // One character can only have meant `-f`; `--f` is nobody's intent.
        assert_eq!(parse_spec("f").unwrap(), (Some('f'), None, Arity::Flag));
        assert_eq!(
            parse_spec("f=").unwrap(),
            (Some('f'), None, Arity::Required)
        );
        // Two or more is long, with the `/` merely omitted.
        assert_eq!(
            parse_spec("go").unwrap(),
            (None, Some("go".into()), Arity::Flag)
        );
    }

    #[test]
    fn malformed_specs_are_rejected() {
        for spec in ["", "ab/cd=x", "e=e/env", "bad name", "/"] {
            assert!(parse_spec(spec).is_err(), "{spec:?} should be rejected");
        }
    }

    #[test]
    fn a_slash_names_the_halves_explicitly() {
        // Whatever is on a side of the `/` is that half, so a leading `/` is
        // long-only -- including the one-character long option, which the
        // length default would otherwise make unreachable.
        assert_eq!(
            parse_spec("/f").unwrap(),
            (None, Some("f".into()), Arity::Flag)
        );
        assert_eq!(
            parse_spec("/force").unwrap(),
            (None, Some("force".into()), Arity::Flag)
        );
        assert_eq!(parse_spec("f/").unwrap(), (Some('f'), None, Arity::Flag));
        assert_eq!(
            parse_spec("/f=").unwrap(),
            (None, Some("f".into()), Arity::Required)
        );
    }

    #[test]
    fn reads_a_required_value_either_spelling() {
        for argv in [
            vec!["--env", "prod"],
            vec!["--env=prod"],
            vec!["-e", "prod"],
            vec!["-eprod"],
        ] {
            let got = eval(
                &argv,
                r#"local e = cli.opt("e/env=") cli.parse() return e:get()"#,
            )
            .unwrap_or_else(|e| panic!("{argv:?}: {e}"));
            assert_eq!(got, "prod", "{argv:?}");
        }
    }

    #[test]
    fn calling_a_handle_reads_it() {
        let got = eval(
            &["--env", "prod"],
            r#"local e = cli.opt("e/env=") cli.parse() return e() .. "/" .. e:get()"#,
        )
        .unwrap();
        assert_eq!(got, "prod/prod");
    }

    #[test]
    fn calling_before_parse_raises() {
        let err = eval(&[], r#"local e = cli.opt("e/env=") return e()"#).unwrap_err();
        assert!(
            err.to_string().contains("before cli.parse()"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn flags_read_as_a_boolean_and_count_separately() {
        // A cluster repeats the flag; presence is the value, the repeat is the
        // count. `0` being truthy in Luau is why these are not one answer.
        let got = eval(
            &["-vvv"],
            r#"local v = cli.opt("v/verbose") cli.parse()
               return tostring(v:get()) .. " " .. v:count()"#,
        )
        .unwrap();
        assert_eq!(got, "true 3");
    }

    #[test]
    fn count_is_uniform_across_handle_kinds() {
        let got = eval(
            &["-I", "a", "--include", "b", "x", "y", "z"],
            r#"
            local i = cli.opt("I/include=+")
            local e = cli.opt("e/env=")
            local host = cli.arg("host")
            local rest = cli.arg("rest...")
            cli.parse()
            return string.format("%d %d %d %d", i:count(), e:count(), host:count(), rest:count())
            "#,
        )
        .unwrap();
        assert_eq!(got, "2 0 1 2");
    }

    #[test]
    fn absent_values_have_stable_shapes() {
        let got = eval(
            &[],
            r#"
            local f = cli.opt("v/verbose")
            local e = cli.opt("e/env=")
            local i = cli.opt("I/include=+")
            cli.parse()
            return string.format("%s %s %s", tostring(f:get()), tostring(e:get()), #i:get())
            "#,
        )
        .unwrap();
        assert_eq!(got, "false nil 0");
    }

    #[test]
    fn repeatable_options_accumulate() {
        let got = eval(
            &["-I", "a", "--include", "b", "--include=c"],
            r#"local i = cli.opt("I/include=+") cli.parse() return table.concat(i:get(), ",")"#,
        )
        .unwrap();
        assert_eq!(got, "a,b,c");
    }

    #[test]
    fn optional_value_is_true_when_bare() {
        let got = eval(
            &["--file"],
            r#"local f = cli.opt("f/file=?") cli.parse() return tostring(f:get())"#,
        )
        .unwrap();
        assert_eq!(got, "true");
    }

    #[test]
    fn positionals_fill_in_order() {
        let got = eval(
            &["h1", "a.txt", "b.txt"],
            r#"
            local host = cli.arg("host")
            local files = cli.arg("files...")
            cli.parse()
            return host:get() .. ":" .. table.concat(files:get(), ",")
            "#,
        )
        .unwrap();
        assert_eq!(got, "h1:a.txt,b.txt");
    }

    #[test]
    fn double_dash_ends_options() {
        // The point of `--`: a value that looks exactly like a declared option
        // still reaches the positional.
        let got = eval(
            &["--", "--env"],
            r#"
            local e = cli.opt("e/env=")
            local a = cli.arg("what")
            cli.parse()
            return tostring(e:get()) .. " " .. a:get()
            "#,
        )
        .unwrap();
        assert_eq!(got, "nil --env");
    }

    #[test]
    fn reading_before_parse_raises() {
        let err = eval(&[], r#"local e = cli.opt("e/env=") return e:get()"#).unwrap_err();
        assert!(
            err.to_string().contains("before cli.parse()"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn duplicate_declarations_raise() {
        let err = eval(&[], r#"cli.opt("e/env=") cli.opt("e/other=") return ''"#).unwrap_err();
        assert!(
            err.to_string().contains("already declared"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn required_after_optional_is_a_spec_error() {
        let err = eval(&[], r#"cli.arg("a?") cli.arg("b") return ''"#).unwrap_err();
        assert!(
            err.to_string().contains("must come last"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn nothing_may_follow_a_rest_argument() {
        let err = eval(&[], r#"cli.arg("files...") cli.arg("b") return ''"#).unwrap_err();
        assert!(
            err.to_string().contains("already takes every"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn unknown_option_exits_two() {
        let err = eval(&["--nope"], r#"cli.parse() return ''"#).unwrap_err();
        assert_eq!(exit_code(&err), Some(2));
    }

    #[test]
    fn missing_required_argument_exits_two() {
        let err = eval(&[], r#"cli.arg("host") cli.parse() return ''"#).unwrap_err();
        assert_eq!(exit_code(&err), Some(2));
    }

    #[test]
    fn leftover_arguments_exit_two() {
        let err = eval(&["a", "b"], r#"cli.arg("only") cli.parse() return ''"#).unwrap_err();
        assert_eq!(exit_code(&err), Some(2));
    }

    #[test]
    fn help_exits_zero() {
        let err = eval(&["--help"], r#"cli.opt("e/env=") cli.parse() return ''"#).unwrap_err();
        assert_eq!(exit_code(&err), Some(0));
    }

    #[test]
    fn help_beats_a_missing_required_argument() {
        // The script whose help you need is precisely the one you cannot yet
        // invoke correctly, so `--help` cannot wait on the arguments matching.
        let err = eval(&["--help"], r#"cli.arg("host") cli.parse() return ''"#).unwrap_err();
        assert_eq!(exit_code(&err), Some(0));
    }

    #[test]
    fn claiming_one_help_spelling_leaves_the_other_automatic() {
        // `cli.opt("/help")` takes only the long spelling; `-h` still prints.
        let err = eval(
            &["-h"],
            r#"local m = cli.opt("/help") cli.parse() return ''"#,
        )
        .unwrap_err();
        assert_eq!(exit_code(&err), Some(0));

        // ...and the claimed spelling falls through to the script.
        let got = eval(
            &["--help"],
            r#"local m = cli.opt("/help") cli.parse() return tostring(m())"#,
        )
        .unwrap();
        assert_eq!(got, "true");
    }

    #[test]
    fn claiming_long_help_makes_short_help_the_whole_page() {
        // With `--help` gone to the script, the auto `-h` is the only help
        // there is, so it must reach the details rather than withholding them
        // for a spelling that will never print them.
        let lua = lua(&["-h"]);
        let err = lua
            .load(r#"cli.about("s", "d") cli.opt("/help") cli.parse()"#)
            .exec()
            .unwrap_err();
        assert_eq!(exit_code(&err), Some(0));

        // The rendering that `-h` reached, checked directly: `usage` is what
        // the exit path above called, and Detail::Full is what it passed.
        let reg = Registry {
            summary: Some("s".into()),
            details: Some("d".into()),
            ..Registry::default()
        };
        assert_eq!(usage(&reg, Detail::Full), "s\n\nd\n\nUsage:\n");
    }

    #[test]
    fn a_partially_claimed_help_row_does_not_name_the_other_half() {
        // The auto `--help` may only render as `-h, --help` when it owns `-h`
        // too; otherwise the row would claim a flag belonging to the script.
        let lone = OptSpec {
            short: None,
            long: Some("help".into()),
            arity: Arity::Flag,
            description: Some("show this help".into()),
            details: None,
            help: Help::Lone,
            extra: Extra::default(),
        };
        assert_eq!(lone.label(), "    --help");
    }

    #[test]
    fn a_script_may_claim_help_itself() {
        // Claiming the spelling means owning it: no auto-exit, just a flag.
        let got = eval(
            &["-h"],
            r#"local h = cli.opt("h/help") cli.parse() return tostring(h:get())"#,
        )
        .unwrap();
        assert_eq!(got, "true");
    }

    #[test]
    fn sentinel_survives_the_lua_boundary() {
        // The sentinel is nested in CallbackError layers by the time a pcall-free
        // script propagates it; `exit_code` has to see through them.
        let err = eval(
            &["--nope"],
            r#"local function go() cli.parse() end go() return ''"#,
        )
        .unwrap_err();
        assert_eq!(exit_code(&err), Some(2));
    }

    /// The prose a script's `cli.about` calls left in the registry.
    ///
    /// The `--help` path writes to the process's own stdout, which a test
    /// cannot capture, so assertions about *what was set* go through here and
    /// assertions about *how it renders* go through `usage` directly.
    fn about_of(lua: &Lua, src: &str) -> (Option<String>, Option<String>) {
        lua.load(src).exec().expect("declare");
        let handle: mlua::AnyUserData = lua
            .load(r#"return cli.arg("probe")"#)
            .eval()
            .expect("probe arg");
        let handle = handle.borrow::<Handle>().expect("handle");
        let reg = handle.registry.borrow();
        (reg.summary.clone(), reg.details.clone())
    }

    /// A registry carrying both halves of the prose.
    fn described() -> Registry {
        Registry {
            summary: Some("Copy files to a deployment target.".into()),
            details: Some("Reads a manifest and uploads what changed.".into()),
            ..Registry::default()
        }
    }

    #[test]
    fn about_heads_the_usage_block() {
        let mut reg = Registry {
            summary: Some("Copy files to a deployment target.".into()),
            ..Registry::default()
        };
        reg.args.push(ArgSpec {
            name: "host".into(),
            kind: ArgKind::Required,
            description: None,
            details: None,
            extra: Extra::default(),
        });
        let text = usage(&reg, Detail::Summary);
        assert_eq!(
            text,
            "Copy files to a deployment target.\n\nUsage: <host>\n\nPositionals:\n  host\n"
        );
        let text = usage(&reg, Detail::Full);
        assert_eq!(
            text,
            "Copy files to a deployment target.\n\nUsage: <host>\n\nPositionals:\n\nhost\n"
        );
    }

    #[test]
    fn details_follow_the_summary_and_never_replace_it() {
        // The invariant behind the two-argument form: `--help` is the summary
        // plus more, so there is no pair of strings that makes the two spellings
        // describe the script differently.
        assert_eq!(
            usage(&described(), Detail::Full),
            "Copy files to a deployment target.\n\n\
             Reads a manifest and uploads what changed.\n\n\
             Usage:\n"
        );
    }

    #[test]
    fn short_help_keeps_the_summary_and_drops_the_details() {
        assert_eq!(
            usage(&described(), Detail::Summary),
            "Copy files to a deployment target.\n\nUsage:\n"
        );
    }

    #[test]
    fn details_without_a_summary_are_impossible() {
        // `cli.about` cannot express one, and rendering ignores a stray one
        // rather than printing details under no heading.
        let reg = Registry {
            summary: None,
            details: Some("orphaned".into()),
            ..Registry::default()
        };
        assert_eq!(usage(&reg, Detail::Full), "Usage:\n");
    }

    #[test]
    fn about_takes_both_halves() {
        let lua = lua(&[]);
        let got = about_of(&lua, r#"cli.about("summary", "details")"#);
        assert_eq!(got, (Some("summary".into()), Some("details".into())));
    }

    #[test]
    fn a_one_argument_about_clears_earlier_details() {
        // Both halves move together: the surviving summary must not end up
        // heading prose written for a different one.
        let lua = lua(&[]);
        let got = about_of(&lua, r#"cli.about("first", "details") cli.about("second")"#);
        assert_eq!(got, (Some("second".into()), None));
    }

    #[test]
    fn usage_without_about_is_unchanged() {
        let reg = Registry::default();
        assert_eq!(usage(&reg, Detail::Full), "Usage:\n");
    }

    #[test]
    fn the_help_pair_renders_as_one_row() {
        let mut reg = Registry::default();
        reg.opts.push(OptSpec {
            short: Some('h'),
            long: None,
            arity: Arity::Flag,
            description: Some("show a summary (--help for more)".into()),
            details: None,
            help: Help::Folded,
            extra: Extra::default(),
        });
        reg.opts.push(OptSpec {
            short: None,
            long: Some("help".into()),
            arity: Arity::Flag,
            description: Some("show help (-h for a summary)".into()),
            details: None,
            help: Help::Row,
            extra: Extra::default(),
        });
        assert_eq!(
            usage(&reg, Detail::Summary),
            "Usage: [options]\n\nOptions:\n  -h, --help  show help (-h for a summary)\n"
        );
        assert_eq!(
            usage(&reg, Detail::Full),
            "Usage: [options]\n\nOptions:\n\n-h, --help\n  show help (-h for a summary)\n"
        );
    }

    #[test]
    fn about_after_parse_raises() {
        let err = eval(&[], r#"cli.parse() cli.about("x") return ''"#).unwrap_err();
        assert!(
            err.to_string().contains("already rendered"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn assert_true_is_a_no_op() {
        let got = eval(&[], r#"cli.parse() cli.assert(true, "unused") return "ok""#).unwrap();
        assert_eq!(got, "ok");
    }

    #[test]
    fn assert_false_exits_two_with_the_message() {
        let err = eval(
            &[],
            r#"cli.parse() cli.assert(false, "bad value") return ''"#,
        )
        .unwrap_err();
        assert_eq!(exit_code(&err), Some(2));
    }

    #[test]
    fn assert_without_a_message_has_a_default() {
        let err = eval(&[], r#"cli.parse() cli.assert(false) return ''"#).unwrap_err();
        assert_eq!(exit_code(&err), Some(2));
    }

    #[test]
    fn usage_lists_both_sections() {
        let lua = lua(&["--help"]);
        let err = lua
            .load(
                r#"
                cli.opt("e/env=", "deployment target")
                cli.arg("host", "target machine")
                cli.parse()
                "#,
            )
            .exec()
            .unwrap_err();
        assert_eq!(exit_code(&err), Some(0));
    }

    fn code(argv: &[&str], src: &str) -> Option<i32> {
        eval(argv, src).err().and_then(|e| exit_code(&e))
    }

    #[test]
    fn a_value_option_does_not_swallow_a_flag() {
        let src = r#"local e = cli.opt("e/env=") local f = cli.opt("f/flag") cli.parse() return tostring(e())"#;
        assert_eq!(code(&["--env", "--flag"], src), Some(2));
        assert_eq!(code(&["-e", "--help"], src), Some(0), "help still answers");
        assert_eq!(eval(&["--env=--flag"], src).unwrap(), "--flag");
        assert_eq!(eval(&["-e=--flag"], src).unwrap(), "--flag");
    }

    #[test]
    fn negative_numbers_are_values() {
        let src = r#"local n = cli.arg("n") local e = cli.opt("e/env=") cli.parse() return n() .. "," .. tostring(e())"#;
        assert_eq!(eval(&["-5"], src).unwrap(), "-5,nil");
        assert_eq!(eval(&["-e", "-1.5", "-2"], src).unwrap(), "-2,-1.5");
        // A declared digit option wins over the number reading.
        let digit = r#"local one = cli.opt("1") cli.parse() return tostring(one())"#;
        assert_eq!(eval(&["-1"], digit).unwrap(), "true");
    }

    #[test]
    fn short_and_long_equals_agree_on_an_empty_value() {
        let src = r#"local f = cli.opt("f/file=?") cli.parse() return "[" .. tostring(f()) .. "]""#;
        assert_eq!(eval(&["-f="], src).unwrap(), "[]");
        assert_eq!(eval(&["--file="], src).unwrap(), "[]");
        assert_eq!(eval(&["-f"], src).unwrap(), "[true]");
    }

    #[test]
    fn a_repeated_single_value_option_is_a_usage_error() {
        let src = r#"local e = cli.opt("e/env=") cli.parse() return e()"#;
        assert_eq!(code(&["-e", "a", "-e", "b"], src), Some(2));
        let many = r#"local e = cli.opt("e/env=+") cli.parse() return #e()"#;
        assert_eq!(eval(&["-e", "a", "-e", "b"], many).unwrap(), "2");
    }

    #[test]
    fn a_pcall_cannot_turn_an_exit_into_a_second_parse() {
        let src = r#"
            local e = cli.opt("e/env=")
            local ok1 = pcall(cli.parse)
            local ok2, err = pcall(cli.parse)
            return tostring(ok1) .. tostring(ok2)"#;
        assert_eq!(eval(&["--nope"], src).unwrap(), "falsefalse");
    }

    #[test]
    fn parse_takes_an_explicit_list() {
        let src = r#"local w = cli.arg("word") cli.parse({"hi"}) return w()"#;
        assert_eq!(eval(&["ignored", "args"], src).unwrap(), "hi");
        assert!(eval(&[], r#"cli.parse({1})"#).is_err());
    }

    #[test]
    fn help_beats_an_unknown_option() {
        let src = r#"cli.opt("e/env=") cli.parse() return ''"#;
        assert_eq!(code(&["--nope", "--help"], src), Some(0));
        assert_eq!(code(&["--nope"], src), Some(2));
    }

    #[test]
    fn a_near_miss_suggests_the_declared_option() {
        let mut reg = Registry::default();
        reg.argv = vec!["--evn".into()];
        reg.opts.push(OptSpec {
            short: None,
            long: Some("env".into()),
            arity: Arity::Required,
            description: None,
            details: None,
            help: Help::No,
            extra: Extra::default(),
        });
        let err = scan_options(&mut reg).unwrap_err();
        assert!(err.contains("did you mean \"--env\""), "{err}");
        reg.argv = vec!["--zzzzzz".into()];
        assert!(!scan_options(&mut reg).unwrap_err().contains("did you mean"));
    }

    #[test]
    fn the_options_table_adds_default_required_and_choices() {
        let src = r#"
            local env = cli.opt("e/env=", "target", {default = "dev", choices = {"dev", "prod"}})
            local tag = cli.opt("t/tag=", "tag", {required = true, value_name = "tag"})
            local pos = cli.arg("mode?", "mode", {default = "fast", choices = {"fast", "slow"}})
            cli.parse()
            return env() .. "," .. tag() .. "," .. pos() .. "," .. env:count()"#;
        assert_eq!(eval(&["-t", "x"], src).unwrap(), "dev,x,fast,0");
        assert_eq!(eval(&["-t", "x", "-e", "prod", "slow"], src).unwrap(), "prod,x,slow,1");
        assert_eq!(code(&[], src), Some(2), "a required option is missing");
        assert_eq!(code(&["-t", "x", "-e", "qa"], src), Some(2), "not a choice");
        assert_eq!(code(&["-t", "x", "medium"], src), Some(2), "positional not a choice");
    }

    #[test]
    fn the_options_table_is_validated_at_the_declaration() {
        for bad in [
            r#"cli.opt("v/verbose", "", {default = "x"})"#,
            r#"cli.opt("e/env=", "", {required = true, default = "x"})"#,
            r#"cli.opt("e/env=", "", {default = "x", choices = {"a"}})"#,
            r#"cli.opt("e/env=", "", {bogus = 1})"#,
            r#"cli.arg("a", "", {default = "x"})"#,
        ] {
            assert!(eval(&[], &format!("{bad} return ''")).is_err(), "{bad}");
        }
    }

    #[test]
    fn help_shows_value_names_and_notes() {
        let mut reg = Registry::default();
        reg.opts.push(OptSpec {
            short: Some('e'),
            long: Some("env".into()),
            arity: Arity::Required,
            description: Some("target".into()),
            details: None,
            help: Help::No,
            extra: Extra {
                default: Some("dev".into()),
                choices: vec!["dev".into(), "prod".into()],
                value_name: Some("target".into()),
                ..Extra::default()
            },
        });
        let text = usage(&reg, Detail::Summary);
        assert!(
            text.contains("-e, --env=<target>  target [choices: dev, prod] [default: dev]"),
            "{text}"
        );
    }

    #[test]
    fn assert_requires_a_boolean() {
        assert_eq!(code(&[], r#"cli.parse() cli.assert(false) return ''"#), Some(2));
        assert_eq!(code(&[], r#"cli.parse() cli.assert(nil) return ''"#), Some(2));
        let err = eval(&[], r#"cli.parse() cli.assert("yes") return ''"#).unwrap_err();
        assert!(err.to_string().contains("expects a boolean"), "{err}");
        assert!(eval(&[], r#"cli.parse() cli.assert(1) return ''"#).is_err());
    }
}
