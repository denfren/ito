# ito

## Workflow

- Make sure the project builds (`cargo build`) before committing.
- The changelog lives at `CHANGELOG.md` (repo root, [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) format) and is **hand-written** — update it as part of the release commit.

### Release steps

To cut a release `vX.Y.Z`, in order:

1. Update `CHANGELOG.md` — renaming the unreleased section to `## [X.Y.Z] - YYYY-MM-DD`
2. Bump `version` in `ito/Cargo.toml` to `X.Y.Z`.
3. `cargo build` — updates `Cargo.lock` with the new version.
4. Commit (`chore(release): vX.Y.Z`) and tag `vX.Y.Z`.
5. Ask before pushing the commit and tag.

### Documentation to update on surface changes

When the **CLI surface** changes (flags, exit codes, stream routing,
defaults):

- `docs/src/binaries/<binary>.md` — the affected program's chapter. Each
  covers one binary's operating model, its module list, and its gotchas.
  Written **self-contained**: a chapter never mentions or compares itself
  to the others, because a reader arrives at exactly one of them.
- `README.md` — the capability summary and the `## Scripting` module list.
- `ito/cli.rs` — clap `help`/`long_about`/`about` strings are themselves
  user-facing; keep them in sync.
- `docs/src/intro.md` — only if the change touches what it documents (the
  `--doc` help workflow, the `<binary> [<flags>] <script> [<args>...]`
  grammar and `-C` framing, or the exit-code summary).
- `docs/src/args.md` — only for the script-argument surface (`argv`):
  everything after `<script>` belongs to the script.

When the **Luau/stdlib surface** changes (functions, modules, builders,
injected globals):

- `docs/src/modules/<module>.md` — the per-module chapter. These files
  are the single source for both the website and the embedded `--doc`
  help (see below).
- `docs/src/luau/*.md` — the language chapters, if Luau usage guidance
  changes.
- `ito/help.rs` — the `TOPICS` registry: it `include_str!`s the
  `docs/src/**` chapters and drives `--doc` / `--doc <topic>` /
  `--doc all`. **Adding a new module = add a `docs/src/modules/<name>.md`
  chapter, a `TOPICS` entry, and a `docs/src/SUMMARY.md` line.**
- `docs/src/SUMMARY.md` — the mdbook table of contents; add/remove a line
  when a chapter is added/removed.

Notes:

- **Say "Luau", not "Lua"**, in prose and in user-facing strings. The
  exceptions are where the word genuinely means the parent language: "Lua
  patterns" (the name of that pattern syntax, which Luau inherits), the
  sentence in `ito-vs-luau.md` defining Luau as a Lua 5.1 dialect, and
  Rust identifiers like `mlua::Lua`.
- **One book, two front doors.** Every binary embeds every topic, so
  the registry is shared and `--doc <topic>` reaches everything from
  either. A bare `--doc` prints the invoking program's own chapter
  followed by the topic list; `help::doc` takes the program name (from
  the clap `Command`) to pick it. Keep both binary chapters in
  `TOPICS`.
- **Placeholder convention in prose docs** (README, `docs/src/**`,
  `help.rs` markdown strings, this file): write argument placeholders
  lowercase and angle-bracketed — `<path>` for required, `[<path>]` for
  optional, `<name> <value>` for pairs. Do *not* uppercase them
  (`PATH`/`STR` collides visually with the `$PATH` env var). This applies
  only to our hand-written docs; clap's own `--help` output (the
  `value_name` strings in `cli.rs`) keeps clap's default uppercasing and
  is left as-is.
- `docs/book/` is generated mdbook output (`mdbook build docs/`). Never
  hand-edit it; it regenerates from `docs/src/**`.
- The same `docs/src/**` chapters serve two surfaces at once: the mdbook
  website and the embedded `--doc` help. Editing the chapter is usually
  the only content change needed; the registry/SUMMARY edits are wiring.

### `docs/src/modules/*.md` structure

Every module chapter has the same skeleton. Only the sections that apply
are present — never an empty placeholder.

1. `# \`module\` — one-line scope`. Backticks around a global name; the
   text after the dash is a lower-case sentence fragment.
2. A short intro: what the module is for, and in one sentence its
   capability delta (`not in ied`, pure vs. ambient), naming the feature
   test (`if fs.append then`) where one exists.
3. One or more summary tables (see below).
4. `## Example` — one short, realistic snippet.
5. `## Conventions` — rules that cut across several functions, including
   the builder guidance below. Topic sections (`## Value mapping`,
   `## Naming`) may sit beside it; the general-rules section is always
   named `## Conventions`, never `## General rules` or `## Rules`. A
   chapter whose cross-cutting rules are one named topic may name that
   section instead (`## Value mapping` in `json`/`toml`;
   `## Not a round-trip` / `## Format-preserving edits` in `yaml`).
6. One section per callable, in summary-table order, then any topic
   sections (`## Tagging`, `## Notes on structural edits`).

#### Summary table

The main table is exactly two columns: `| Function | Effect |`.

- Use `Function` for callables, `Method` for handle methods in a chapter
  with both, `Value` for constants, `Name` for a mixed table.
- The second column is always `Effect`. There is no `Signature`,
  `Returns` or `Description` column: fold the arguments into the first
  cell (`json.parse(text, [name])`) and the result into the effect.
- An `Availability` third column is allowed only when availability
  genuinely varies per row and is central to the chapter (`fmt`, `ito`,
  the write table in `fs`).
- Auxiliary family tables may use the columns that family needs
  (`| Shorthand | Builder | Returns |`, `| Constant | Value |`), but they
  are secondary and never replace the main index.

#### Per-callable section

```markdown
## `module.function`

One prose sentence: what it is.

**Signature:** `module.function(arg: type, opt?: type) -> ret`

**Parameters:**
- `arg` — what it is, its accepted types, its default.
- `opt` — ... (optional)

**Returns:** the type and shape, including `nil`/empty cases.

**Errors:** what raises, what returns `nil`/`false`/`{}`, with the stable
message prefix where one exists.

```lua
local x = module.function("a")
print(x)   --> "..."
```
```

Rules:

- The heading is exactly `## \`<module>.<function>\`` — fully qualified,
  no argument list, no ` — description`. A family of aliases/overloads
  shares one heading, each segment naming the callable
  (`## \`hash.md5\` / \`hash.sha1\` / ...`); a later segment may elide a
  repeated prefix (`## \`fmt.starts_with\` / \`ends_with\` / \`contains\``).
  A method is `## \`<Type>:<method>\`` (`pool:spawn`, `cursor:kind`,
  `J2Engine:template`); a method documented under its constructor may be
  `### \`<Type>:<method>\`` inside that constructor's `##` section (the
  `fs.finder` methods are the model).
- Field labels are exactly, bold and with a colon: `**Signature:**`,
  `**Parameters:**`, `**Returns:**`, `**Errors:**`. Never `Return value`,
  `Error conditions`, `Shape`, `Availability`, or a colon-less variant.
- `**Signature:**` is one inline-code line. Use a fenced ```lua block
  only when it genuinely wraps or lists several overloads. Type every
  argument: `name: string`, `name?: string`, `...: any`; the arrow is
  `->`, not `→`. Optional arguments use `?` in the signature and
  `[<name>]` in prose/tables.
- `**Parameters:**` describes each argument. With more than one, use
  one bullet per argument: `- \`name\` — what it is, its type limits, its
  default.` A single argument may be inline (`` `name` — … ``), and a call
  with no arguments omits the label or writes `none.`
- `**Returns:**` states the type and shape; several values are written
  `(a, b)`. Omit it only when the call returns nothing.
- `**Errors:**` says precisely what raises versus what returns
  `nil`/`false`/`{}` (and why). Omit it only when the call cannot raise.
- The example is one bare fenced ```lua block after `**Errors:**`, with
  `-->` marking an expression's result and `--` for commentary. There is
  no `**Example:**` label.
- `**Methods:**` is the one extra bold label, used by a builder
  constructor to index its methods (see below).
- Cross-links use the chapter file and the heading's actual slug
  (``[`fmt.trimmed`](fmt.md#fmttrimmed)``).

#### Documenting builders

A builder is a constructor that returns a handle, configuration methods
that return the handle so calls chain, and a terminal that consumes it.
The per-callable blocks document each method; `## Conventions` must also
answer the whole-chain questions, in this order:

1. **Constructor → chain → terminal.** Name the constructor
   (`hcl.builder()`, `proc.command(program)`, `task.pool(n)`,
   `aws.ec2.describe_instances()`, `encoding.base64.engine()`), say that
   every configuration method returns the builder, and name the terminal
   (`:to_string()`, `:run()`/`:spawn()`, `:await()`, `:send()`,
   `:interact()`, `encode`/`decode`). A terminal is never `with_*`.
2. **Mutable or immutable.** Say whether a setter mutates a shared handle
   in place (aliases observe later calls) or returns a *new* builder (the
   base keeps its settings). Both exist: `hcl`, `proc`, `j2`,
   `ui.prompt` and `fmt.table` mutate a shared handle and return it;
   `encoding.base64.engine` returns a new engine from every step. Do not
   write "chainable" without saying which.
3. **Method index.** The constructor's section carries a `**Methods:**`
   list or table naming every setter and terminal, so the reader sees the
   whole surface before the individual sections. The individual sections
   follow in that order.
4. **When validation happens.** Say whether a bad value raises at the
   setter or at the terminal: `j2.with_syntax` raises at the setter,
   `ui.prompt` validates at `:interact()`, `aws` ids are checked at the
   entry call or at `:send()`. This is the most useful fact after the
   chain shape.
5. **Terminal result and async.** `:send()` → `(envelope, data)`;
   `:run()` → the result table; `:await()` → the task results;
   `:interact()` → the answer; `:to_string()` → text. Mark the async ones
   (`:send()`, `:run()`, `:interact()`) explicitly.
6. **Aliasing and replacement.** Say whether a setter replaces the whole
   value (`aws...:tags()`, `:items()`, `:defaults()`) or accumulates
   (`:tag()`, `:arg()`, `:item()`); a reader must not have to guess.

If a builder has one uniform method family, a `**Methods:**` table under
its constructor is enough; a method with non-obvious behaviour still gets
its own `###`/`##` section and still appears in the index. Never document
a builder only as a prose example with no method index.

Naming follows `docs/src/conventions.md`: fluent configuration methods use
`with_*`; a terminal is not `with_*`; the constructor is called with `.`
and handle methods with `:`.

Ground truth is always the Rust source in `ito/scripting/`, not the
existing prose — a rewrite pass re-derives every claim from the
`register`/method definitions and the module's own tests rather than
trusting what's already written. When redoing this pass across many
modules at once, spawn **one agent per module file**, each pointed at its
own doc file plus the Rust source file(s) that back it (per the table in
"Scripting standard library" below for which files back which global);
running them in parallel is safe because each owns a disjoint doc file
and reads (doesn't write) the shared Rust source.

## Architecture

Two binaries share one Luau runner and one stdlib. `ito` is the normal,
full-featured one; `ied` is the restricted variant, and docs describe
only what is *absent* there. The restriction's rationale is
**reproducibility**: an `ied` run is a function of work-directory state +
script, so anything ambient (clock, host zone, environment, randomness,
processes, network, terminal, paths outside `-C`) is withheld — keep that
in mind when deciding whether a new global belongs in `ied`. The capability
boundary is **the absence of the globals themselves**, not a policy check,
so `if fs.write then` is a usable feature test:

| | `ied` | `ito` |
| --- | --- | --- |
| read files, parse formats | ✓ | ✓ |
| `fs` namespace | rooted at `-C` | the host filesystem |
| `fs.write`/`fs.remove` | buffered | direct |
| `fs.finder():grep()`/`:grep_files()` | — | ✓ |
| preview writes (`-d`/`-l`/`-e`), commit (`-w`) | ✓ | — |
| `proc`/`ssh`/`aws.ec2`/`task`, `os.getenv` | — | ✓ |
| `fs.cwd`, `fs.script`, `fs.require_root`/`require_dir`, `proc.pid`, `ito.slug` | — | ✓ |
| `hash` values, `encoding`, `ansible.vault.decrypt`/`is_vault` | ✓ | ✓ |
| `hash.*_file`, `ansible.vault.encrypt`/`load_password_file` | — | ✓ |
| `print` (vanilla, stdout) | ✓ | ✓ |
| `io.stdout`/`stderr`/`stdin` handles, `io.write`/`read`/`lines`/`flush` | — | ✓ |
| `ui.prompt`/`ui.progress` | — | ✓ |
| `time` value conversions, incl. named zones (bundled tzdb) | ✓ | ✓ |
| `time.now`/`till`/`ago`, `_local` parse/format/add/sub | — | ✓ |
| `fmt` styles, text helpers, tables, `fmt.colors` | ✓ (auto = off) | ✓ |
| `fmt.is_tty`, colour auto-detection | — | ✓ |

Each binary has **its own clap `Parser`** (`IedCli`/`ItoCli`),
so a flag exists only where it means something — `ito --diff` is an
unknown flag, not an accepted no-op. Shared groups (`CommonArgs`,
`ReportArgs`) are flattened in by whichever binary applies them.

Per-binary specifics:

- **`ied`** — buffered edits. Writes queue and are applied **only** with
  `-w`/`--write`; a bare run computes the change set, reports on it, and
  drops it (so it is also the read-only query tool). `-d`/`--diff` and
  `-l`/`--list-changed` are mutually exclusive and each **imply `-q`**.
  Its output surface is `print` alone (no `io`, so stderr is reachable
  only via `log.*`); `-q` is only about `print` — it makes it a silent
  no-op (`Config::print`), leaving reports, logs and exit codes alone, so
  a report owns stdout. `ito` has no `-q` at all (`-v`/`-vv` raise log
  verbosity only): scripts control their own output.
- **Output sink.** `print` is vanilla (`tostring` per argument,
  tab-joined, newline; a plain table prints `table: 0x…`, not JSON) and
  never rerouted to the log (under `-q` it is silent). `print` (`print.rs`) and the `ito`-only `io`
  handles (`io.rs`) all go through one `print::emit(stream, bytes)`:
  progress bars paused, flushed on every write (a deliberate difference
  from vanilla's buffering, so output interleaves with logs and bars), and
  a broken pipe (`| head`) unwinds quietly to exit 141 (128 + SIGPIPE: the run was cut short; a `PIPE_BROKEN` flag keeps that true even if the script `pcall`ed the error). `io.stdout:write` is strict like vanilla
  (strings and numbers only) and chains; there is no `io.open` (`fs` owns
  files) and no `io.output`/`io.input` setters (`proc` redirects cover
  that). `log.*` keeps its own JSON table rendering.
- **`ito`** — the full toolset. Its `fs` is the **host filesystem**
  (`rfs.rs`), not a rooted VFS: relative paths resolve against the `-C`
  work directory, absolute paths and `..` reach outside it, and outward
  symlinks are followed. Traversals report **work-directory-relative**
  paths (matching the VFS), falling back to absolute only for a path
  genuinely outside it. `-C` is where relative paths start, not a
  boundary — a binary that can spawn `rm` is not confined by one. Direct writes plus `proc`/`ssh`/`aws.ec2`/
  `task`, and `os.getenv` (read-only, added onto the stdlib `os` table
  under `host_io`; there is deliberately no setter — a command's own
  `:env()` scopes a variable to the child instead of mutating process-wide
  state every later task would see). The host-location readers live here
  for the same reason: `fs.cwd()`, plus `fs.require_root()`/
  `fs.require_dir()`/`fs.script()` (the `require` anchors and the running
  script as values — `require_dir` reads the *calling* chunk's name off the
  Luau stack, so a `require`d module gets its own directory; all are
  registered onto `fs` under `host_io` by `sl::ito::register_host_paths`),
  and `proc.pid()` (`register_host_pid`), `ito.slug()`. The clock joins them:
  `time.now`, `time.till`/`time.ago`, plus the `_local` parse/format/add/sub
  family, which read the host's time zone (`sl::time::register_host_time`).
  No preview flags: nothing is held back to preview. Also carries
  `--ai-skill-init`.

The grammar is `<binary> [<flags>] <script> [<args>...]` for both.
The binary's flags — `-C` (repeatable, make-style) for the working
directory and `fs` root, `-v`, `--doc [<topic>]`, `--completion <shell>`,
plus the binary's own — go **before** `<script>` (a **path** — no name
lookup or search list). Everything after it is the script's, reaching it
verbatim as the global `argv` (see `scripting/argv.rs`) and `cli.parse`,
flag-looking tokens included, so a `#!/usr/bin/env -S ito` script owns
its whole command line. This is one clap positional
(`CommonArgs::script_and_args`, `trailing_var_arg` +
`allow_hyphen_values`) rather than `script` + `args`, because clap only
stops taking flags once the *last* positional starts collecting. A bare
invocation lists the scripts found in `./`, `ito/` and `.ito/`. Script
paths resolve against the **invocation cwd, before `-C` applies** (so a
`#!/usr/bin/env -S ito -C <dir>` shebang script is still found — the
kernel passes it as `./<script>`). A leading shebang line is turned into
a Luau comment (`comment_out_shebang` in `run.rs`) rather than removed, so line
numbers stay correct.

Reports and `print()` go to stdout; all logging goes to stderr.

Exit codes: `0` success (no changes under `--exit-code`); `1` success
*with* changes (only under `-e`/`--exit-code`, `git diff --exit-code`
style — a "change" is a buffered write differing from disk); `2` generic
failure not the script's fault (usage, `-C`, script load, output/flush);
`3` the script's fault (parse/compile or runtime error); `141` the output
pipe went away (`| head`; 128 + SIGPIPE, the run was cut short). These are
modeled by `RunError { code, message }` in `run.rs` (`From<String>`
defaults to `2`; `RunError::script` is `3`).

### Workspace layout

A Cargo workspace (root `Cargo.toml`, `resolver = "3"`). Shared
dependency versions live in `[workspace.dependencies]`; members
inherit via `.workspace = true`. Members:

- **`ito/`** — the library crate plus **two** binaries (sources at the
  crate root, not `src/`; every `[[bin]]` and `[lib]` uses an explicit
  `path`). `bin/{ito,ied}.rs` are each a short `main` over the
  phases in `run.rs`; what a binary *does* is the `Config` it names, so
  the capability surface is one struct literal per binary rather than a
  call sequence each could get differently.
- **`crates/yamlito/`** — the standalone YAML parser/CST-editor crate
  (see below). A dependency of `ito`: the `yaml` module is backed by its
  value model (`yamlito::Yaml`), though the CST-editing layer is not yet
  exposed.

### `ito` crate layout (`ito/`)

| File | Responsibility |
| --- | --- |
| `bin/{ito,ied}.rs` | One `main` each: parse, `Startup`, name a `Config`, `Engine::build`, `eval`, emit. They differ in **which `Config` constructor they name** and what they do with the result — nothing else. |
| `run.rs` | The run phases. `Startup::begin` applies `-C` and captures the invocation cwd (script paths resolve against the latter, *before* the chdir, so a shebang script survives it); `Startup::script` loads the script (or lists the available ones when none was given); `handle_info_flags` handles the print-and-exit flags (`--doc` → `help::doc` with the program name, `--completion`). **`Config` is the whole of what differs between the binaries** — `fs: Fs::{Rooted{writes},Host}`, `host_io`, `ambient`, `print` (whether `print` writes), plus `argv` (uniform, not a capability). `Config::ied`/`ito` are the per-binary profiles. `Engine::build` is the *only* constructor: it makes the `mlua`/Luau state (`mlua::Lua::new()` installs Luau's default `require`, **replaced** by ito's `ItoRequirer` via `scripting::require::register` — confined to the entry script's directory tree, the *require root*, independent of the `fs`/`-C` root; see `docs/src/luau/require.md`), registers the core (including `print`), `cli`+`argv`, `fs`, host I/O (including `io`), drops ambient state, then `j2`, then `sandbox(true)`. Registration order is settled *inside* `build` rather than being a rule callers follow: `proc` before `ssh` (redirect targets), `j2` after the `fs` choice (`Fs::Rooted` → `j2::register`, `Fs::Host` → `register_host` (`register_without_fs` is currently unused) — an include is a file read, so it gets exactly the access `fs` got), `sandbox` last (it freezes the tables `remove_random`/`add_getenv`/`time::register_host_time` reach into). `Engine::vfs()` is an `Option` (`Some` only under `Fs::Rooted`), and only `ied` calls it. `Engine::eval` runs the chunk (named `@<abs path minus extension>`, so `require`'s caller-detection — which keys off the chunk name, Luau's `debug.info` convention — resolves relative to the entry script), then calls a global `main` if defined, then **drains outstanding `task`s**. Afterwards `emit_reports` and `flush_writes` (`ied`); the drain-before-flush rule is `eval`'s own doing. Everything is `async` under a current-thread tokio runtime + `LocalSet` (`main_with`). Also holds `RunError`/`Outcome` and `init_tracing`. |
| `cli.rs` | clap layer: one `Parser` per binary (`IedCli`/`ItoCli`), each flattening the shared groups it applies — so a binary's flag set is what its struct declares, with no runtime filtering. `CommonArgs` is `-C` + `-v` + the `--doc`/`--completion` exits + the trailing `SCRIPT [ARGS]...` positional (`script()`/`argv()` split it); `ReportArgs` is `-w`/`-d`/`-l`/`-e` (only `IedCli`). |
| `help.rs` | `--doc` implementation: the `TOPICS` registry (`include_str!`s `docs/src/**`, including both binary chapters), topic lookup, the grouped listing, the per-program default page, and the styled-markdown-through-a-pager `emit` (honors `--doc-plain`/`--doc-no-pager`/`NO_COLOR`). |
| `lib.rs` | Library crate root: `pub mod cli; pub mod help; pub mod run; pub mod scripting;`. `run` and `cli` are public because the binaries live outside the library. |

### Scripting standard library (`ito/scripting/`)

Each module lives in its own file (or directory) and exposes a Rust
`register(&Lua, …)` that installs a global table. **The Rust file name
matches the Luau global**, so `mod.rs` reads like the script-visible
surface: `fs` is the exception — it has *two* implementations, `vfs.rs`
(the rooted VFS `ied` gets, named for the type) and `rfs.rs` (the
real filesystem `ito` gets) — as is `redirect.rs`, which is not a module
of its own (see below). The
per-module doc comment at the top of each file is the authoritative
surface description; consult it rather than duplicating it here.

Globals: `fs`, `path`, `time`, `json`, `toml`, `yaml`, `hcl`, `j2`, `re`,
`hash`, `encoding`, `ansible` (holding `ansible.vault`), `fmt` (holding
`fmt.table`), `log`, `io`, `ito`, `proc`, `ssh`, `aws` (holding `aws.config`,
`aws.ec2`, `aws.s3`, `aws.autoscaling`, `aws.cloudtrail`, `aws.cloudwatch`),
`task`, `argv`, `cli`, `ui` (holding `ui.prompt`, `ui.progress`), `null` (the
null sentinel, `null.rs`), `table.array` (the array marker, added onto Luau's
`table` by `codec::register_array`), plus `string` method additions (sugar
over `fmt`). `mod.rs` re-exports `Argv`, `Vfs` and `PathMapper`.

Two structural notes:

- **`redirect.rs` is a Rust module but not a Luau one.** It holds the
  stream redirect targets (`Redirect`, `redirect_from_value`), installed
  onto the **`proc` table** by `proc::module` via `redirect::install`. A
  redirect target is only ever an argument to `:stdout()`/`:stderr()`, so
  it lives beside the commands that take it. `ssh` consumes the *same*
  `Redirect` values through the same `redirect_from_value`, which is what
  makes `proc.string()` configure a remote command as well as a local
  one — so `proc` must be registered before `ssh`.
- `task` is the odd one out — its `register` returns a `Tasks` drain
  handle the host has to await (see below).
- **`ui` is a directory (`ui/mod.rs`, `ui/prompt.rs`, `ui/progress.rs`), a
  namespace of namespaces rather than a flat module**, since more (`ui.table`,
  `ui.tree`, …) are expected to join `ui.prompt`/`ui.progress` later. Both
  submodules are host-only for the same reason `proc`/`ssh` are: terminal
  interaction is ambient state a query's return value or an edit preview must
  not depend on. `ui.prompt`'s blocking dialogs run through
  `tokio::task::spawn_blocking`, the same shape `hash.rs`'s `_file` reads use,
  since a terminal prompt has no async form and this is the single-threaded
  runtime every I/O module shares. `ui.prompt`'s theme choice reads
  `fmt::ansi::enabled()` (made `pub` for this) so a prompt's colour tracks
  `fmt.colors()` the way `string:red()` does, rather than a separate switch.

There is **no injected-global magic left**: no `MAIN`, no `ITO_VERSION`,
no `debug_print`. A `main` function marks the entry point (the runner
calls it when present, and *not* for `require`d modules),
`ito.version_check` asserts compatibility, and `log.debug` logs.

`cli.rs` is the *opt-in* parser over those arguments, registered by
`Engine::build` for both binaries. `cli.opt`/`cli.arg` declare into
one process-wide registry and return opaque handles, read either by calling
them (`handle()`, the documented spelling) or via `handle:get()` — one
`Value` read behind both a `Call` metamethod and a method, since a handle
that has left its declaration site reads better with the name on it.
`handle:count()` is the *other* question over that same `Value` — how many
times it appeared — uniform across every handle kind (occurrences for a
flag, elements for `=+`/`...`, 1-or-0 for a single value), so
`files:count()` and `#files()` cannot disagree. A **flag reads as a
boolean** and keeps its count only behind `:count()`: `0` is truthy in
Luau, so a counting `flag()` made `if flag() then` always true — the trap
sat in the common case. Relatedly, a spec with no `/` picks its half by
**length** (`f` → `-f`, `force` → `--force`): a bare one-character name
could only ever have meant a short option, and spelling it `--f` was never
anyone's intent. The `/` remains the explicit override, naming the halves
by side (`f/`, `/force`, `f/force`) — which is what keeps `/f` → `--f`
expressible rather than lost to the default.
`cli.parse` consumes the registry, and reading before that raises. It is
registered for every binary despite writing to stdout, which `ied`'s
reports share — it can still have it because what it prints is **terminal**:
the run unwinds out of `eval`, so it never reaches `emit_reports`/
`flush_writes` and cannot interleave with them or commit anything. Nothing
in it is capability-bearing. Both exits
leave through a `CliExit` sentinel that `Engine::eval` recognises via
`cli::exit_code` (walking mlua's `CallbackError` nesting) and turns into a
`RunError` with an **empty message** — `main_with` skips `tracing::error!`
for those, since the text is already on the stream `cli` chose. Help is
answered *before* positionals are matched, so a script with required
arguments can still print its own help. Spec grammar is fish `argparse`'s
(`v/verbose`, `e/env=`, `f/file=?`, `I/include=+`); a malformed spec or a
duplicate raises at the declaring call, a malformed command line exits 2.
No program name appears in the usage line — under a shebang `argv[0]` is
the interpreter, not the script.

`argv.rs` is the script's own arguments: `CommonArgs::argv` takes
everything after `SCRIPT`, and `register` installs it as the global `argv` — a plain list of
strings, **not** prefixed with the program name. Nothing is parsed for the
script: there is no option map, no type coercion and no name validation,
because every script wants a different grammar. `ito.opt`/`ito.flag` and
the `key=value`/`+flag`/`-S`/`-N`/`-B` forms they read are gone; don't
re-add them.

A couple of notes that aren't obvious from any single file:

- **Two `fs` implementations, one Luau global.** `vfs.rs` is the rooted
  VFS (`ied`): a `PathMapper` translates a root-relative
  script-visible path to a real one and rejects `..`/symlink escapes.
  `rfs.rs` is the host filesystem (`ito`): no root, no mapper, no base —
  `-C` already did `set_current_dir`, so `std::fs` resolves relative
  paths against the work directory for free. For any path the VFS
  accepted, `rfs` names the same file; escapes are the only delta —
  including traversal output, which `out_path` renders relative to the
  work directory (so it matches the VFS's spelling) and leaves absolute
  only when the path lies outside it, there being no shorter honest form.
  `absolutize` is still the internal representation: globs match against
  it and the cycle checks compare real locations, so only the rendering
  on the way back to the script is short.

  The walkers, `GlobFilter`, `Finder` and friends are **duplicated**
  between the two rather than sharing a "rooted?" flag: that keeps `rfs`
  absolute-native internally with no conditionals threaded through
  `vfs.rs`, and it survives a later crate split. `write_now`/`remove_now` are
  `pub(crate)` in `vfs.rs` and shared, so parent-creation and error
  wording can't drift. `j2` follows `fs`: `register` (rooted, confined to
  the VFS root), `register_host` (confined to the explicitly-set include
  root — there being no wider root), `register_without_fs`.

- **`fs.finder():grep()`/`:grep_files()` are terminals, not filters**
  (`finder.rs`), and host-only for now — installed by
  `add_methods_with_follow`, so the rooted finder simply lacks them. They
  are the only finder methods that *open* a matched file, and `Source::
  render` settles which paths may be named, not which bytes may be read;
  exposing them on `ied` means extending containment to reads.
  Terminal rather than chainable because a `:grep(a):grep(b)` would owe an
  answer about whether contents AND or OR the way `Clause`s do, and
  because ending the chain is what lets the result carry the line and its
  capture groups rather than just a path. The engine is ripgrep's
  `grep-searcher` + `grep-regex`, which is **not** a second dialect:
  `grep-regex` builds a `regex-automata` matcher from `regex-syntax`, so a
  pattern means what `re` says it means — that is why `re` was left alone.
  What it buys is line splitting, `BinaryDetection::quit` and mmap. Groups
  come from a *separate* `regex::Regex` over the matched line, because
  `grep-matcher` maps a group name to its index and never the reverse, so
  named groups cannot be enumerated through it; it is compiled once per
  search and only when the pattern has groups. **`:limit(n)` bounds
  matches, not paths** — the one place a limit changes meaning between
  terminals — so `grep` runs the walk through `run_unlimited` and spends
  the bound during the search, where it can stop reading. An unreadable
  file or a non-file hit is skipped rather than fatal, the judgement
  `sort_hits` already makes about an unreadable `stat`.

- **Write modes** (`vfs.rs`): `Writes::{Denied,Buffered}` decides
  how `fs::write`/`fs::remove` reach disk, and which of them exist at all.
  `Buffered` (`ied`) queues into `Rc<RefCell<Pending>>` (a per-path
  `BTreeMap<PathBuf, Action>` shared by the module's closures and the
  host's `Vfs`, last call wins) and the host flushes after the script
  returns **only under `-w`/`--write`**; scripts cannot read their own
  buffered writes. `Denied` registers no write functions at all (no binary
  uses it since `iq` was dropped).
  There is no `Direct` — `ito` gets `rfs` instead, which writes
  immediately and also carries the operations with no previewable form
  (`fs.mkdir`/`remove_all`/`rename`/`copy`).

  There is deliberately **no script-facing `fs.flush`/`fs.clear`**: when
  the buffer reaches disk is the host's call alone, which is what lets
  `--diff`/`--list-changed` promise that nothing was written. Don't
  re-add them.
- **`time` is one global registered in two halves** (`time.rs`), the same
  shape `ito.rs` uses: `register` installs the pure value conversions
  everywhere, `register_host_time` adds the clock (`now`, `till`, `ago`)
  and the `_local` half under `host_io`. **Every parser/formatter is named
  `parse_<format>[_<zone>]`/`format_<format>[_<zone>]`**: no zone suffix
  means the format carries its own zone (`rfc3339`, `rfc2822`); `_utc`
  means the value carries none and is read/written in UTC; `_tz` takes an
  extra `zone` argument and resolves it against the **bundled** tzdb
  (`tzdb-bundle-always`, ~310KB), so it lives in both binaries; `_local`
  reads/writes in the host's time zone (its DST rules at that instant, not just the current offset) and is host-only. Only `civil`
  (an offset-less date/date-time) and `fmt` (a caller-supplied strftime
  pattern) get the full suffix family — `rfc3339`/`rfc2822` get none, since
  a `_utc` variant on a format that already carries its own zone would be a
  redundant assertion. There is deliberately **no** zone-less overload
  defaulting to the host zone and no `local_zone()`: either would smuggle
  ambient state into a function that looks pure. `parse_civil_*` rejects
  input that already carries an offset, rather than silently ignoring it.
  `format_fmt_*`/`parse_fmt_*` take the zone as a separate argument from
  the pattern, and reject a pattern that looks like an offset/zone (a
  pattern without `%` formats to itself, which is a plausible-looking wrong
  answer). `register_host_time` implements `add_local`/`sub_local` with the
  same `shift` helper `add_utc`/`sub_utc`/`add_tz`/`sub_tz` use, parameterized
  only by zone, so the calendar and coercion behaviour cannot drift between
  them. Backed by `jiff` with `default-features = false` plus `tz-system` (the
  host zone for `_local`) and `tzdb-bundle-always` (named zones).
  **Deltas are one surface**: `time.parse_delta` is the single coercion
  (number → seconds, string → `humantime::parse_duration`, a bare number in
  a string is an error) that *every* delta-taking function calls —
  `add_*`/`sub_*`, `truncate`/`count`, `format_delta`, `task.wait` and the
  other `task` deadlines (`task.rs` has no parser of its own, only
  `parse_delta_value`) — so none can disagree about what `1h30m` means.
  There is no ISO 8601 / `P1D` spelling and no jiff `Span` spelling; don't
  re-add one. **A day is a fixed 24h everywhere** (`"1d"`, `86400`,
  `time.days(1)`, including under `add_tz`/`add_local`). Months and years
  are rejected by `parse_delta` *before* humantime sees them
  (`calendar_unit`), because it accepts both against fixed
  30.44/365.25-day approximations — exactly the invented answer this module
  refuses; only `add_*`/`sub_*` take them (`"1M"`, `"2y"`), having a
  timestamp to anchor and so being calendar-correct.
  **Deltas are unsigned** — a duration is a magnitude, `"-90s"` is not a
  spelling — but subtraction is not, so `format_delta` prints a negative
  with a leading `-` rather than refusing the caller's own arithmetic.
  `format_delta` is hand-rolled (`d`/`h`/`m`/`s`) because neither candidate
  renders it: jiff's `SignedDuration` is calendar-less and stops at the
  hour (4 days prints as `96h`), humantime's formatter spells large units
  out (`4days 2h`) and approximates years/months. **The second is the
  display resolution** — the sub-second part is truncated, since a
  computed delta is fractional and rendering it gave
  `173h 44m 24s 737ms 129µs 927ns`; the clock itself is left alone, so a
  *parsed* timestamp keeps the precision it was written with.
  `time.minutes`/`hours`/`days` build a delta from a count;
  `time.truncate(secs, unit)` floors a delta to whole units, in seconds,
  and `time.count(secs, unit)` does the same division but returns the unit
  count (the per-unit `truncate_*`/`count_*` spellings are gone). Both stop
  at the day — a week has no builder here and a month/year has no fixed
  length. `time.till(t)`/
  `time.ago(t)` are `t - now()`/`now() - t`, named for the direction so a
  reader doesn't have to recall a subtraction's argument order; named
  `till` rather than `until` because `until` is a Luau reserved word and
  could never be reached with dot-call syntax. Timestamp arguments accept
  Unix seconds or an RFC 3339 (then RFC 2822) string, via `TimeArg`.
- **`fmt` is one global over a directory** (`fmt/{mod,ansi,text,table}.rs`),
  replacing the former `ansi.rs`/`tablefmt`/`string`-method impls. `mod.rs`
  owns `def`, which registers every string-subject function **twice from one
  implementation** — `fmt.red(s)` and the `string` method `s:red()` — so the
  method is sugar and the two cannot drift; styling reads better with the
  text on the left, which is why the methods stay. `text.rs` is the
  character-/escape-aware helpers (`fmt.strip`, `fmt.width`, `trimmed`,
  `padded_start`/`padded_end`, `replaced_at`, `without_ansi`, …);
  `fmt.width` counts escape-stripped characters by `unicode-width`, so it is
  escape-aware and handles wide glyphs, but is not a full terminal model.
  `table.rs` is `fmt.table.new(columns)` over `comfy_table` (pure, no
  terminal dependence; there is no `t:print`). `ansi.rs` is registered in
  **two halves** for the reason `time` is: `register` (styles, status
  symbols, `fmt.colors`) goes everywhere, while `register_host` (`fmt.is_tty`,
  and the right for `fmt.colors(nil)` *auto* to detect) is host-only — auto
  reads stdout's terminal-ness and `NO_COLOR`/`CLICOLOR`/`CLICOLOR_FORCE`
  (<https://bixense.com/clicolors/>), which is ambient state an `ied --diff`
  must not see. So **auto resolves to off under `ied`** and a script
  there opts in with `fmt.colors(true)`; the styles themselves are pure and
  exist in both. The switch is a process-wide `AtomicU8` (one run, one
  answer, and a `require`d module's setting is visible to its importer),
  which is why `Engine::build` calls `ansi::reset()` — two builds in one
  process (the test suite) must not inherit each other's state (tests that
  move it take `fmt::TEST_LOCK`). `fmt.colors` is a function taking a
  `Variadic`, not an `Option<bool>` field: reading has to *resolve* auto,
  and `fmt.colors()` (read) must be distinguishable from `fmt.colors(nil)`
  (reset to auto). Each style closes with the **specific** reset
  (`39`/`49`/`22`…), not the blanket `0`, so `("x"):red():bold()` keeps
  both — don't "simplify" that to `\27[0m`. `if_color`/`if_nocolor` are the
  alternative-text pair (one optional argument, `""` when omitted); they are
  **eager selectors, not guards** — Luau has evaluated both sides before
  either is called, and laziness is what a plain `if fmt.colors()` is for.
  They deliberately **do not inspect the receiver** for escape sequences: the
  switch decides, which keeps `("[ok]"):if_nocolor("✔")` meaningful and stops
  the answer depending on whether an inner style emitted anything.
  `fmt.ok`/`fail`/`warn`/`info` are status symbols with an ASCII fallback
  chosen by the same switch. No 256-colour/truecolour constructor and no
  style-builder yet.
- **`null` and the array marker are values, not modules.** `null.rs`
  installs the global `null` (mlua's serde null light-userdata, so every
  conversion — codecs and the aws response conversion — agrees on it; `toml`
  has no null and raises). `codec.rs` holds the shared codec rules and
  `register_array` adds `table.array(t?)`, mlua's `array_metatable`, which
  parsers attach to every array they produce so an empty one serializes as
  `[]`. `ito.null`/`json.null`/`ito.is_null` are gone.
- YAML is backed by the `yamlito` crate's value model (`yamlito::Yaml`);
  `yaml::parse`/`to_string` are not a round-trip (emitting owns the
  layout).
- **Everything is async** (`task.rs`): the whole run happens inside a
  current-thread tokio runtime with a `LocalSet` (`main`), and scripts are
  loaded with `exec_async`, because `exec()` cannot drive futures. The
  runtime is *single-threaded on purpose*: mlua's `Lua` is `!Send`, and
  I/O-bound work only needs concurrency, not parallelism — this keeps a
  `Send` bound off every `UserData`. `task.spawn` wraps a Lua thread with
  `Thread::into_async` and drives it via `tokio::task::spawn_local`; a
  plain `coroutine` cannot work here because a coroutine yielding on a
  pending future has nothing to resume it.

  Consequences worth knowing before touching any of this:

  - **An I/O binding must be `async` to be concurrent.** Making `task`
    work is necessary but not sufficient — a module that calls
    `handle.block_on` (or any blocking syscall) pins the one thread and
    silently serializes every task. Use `create_async_function` /
    `add_async_method`, and for genuinely blocking APIs with no async form
    (a `std::fs` read of a large file, say) hand them to
    `tokio::task::spawn_blocking` and await that, as `aws/s3.rs` does. Reach
    for an async-native crate first, though: `proc` runs children on
    `tokio::process` precisely so it needs none of this.
  - **Never hold a `RefCell`/`borrow()` across an `await`.** Another task
    can run at that point and hit a borrow panic. Scope the borrow, clone
    what you need, then await — see `aws.rs`'s `:send()`.
  - **The task drain must stay before the VFS flush** — `Engine::eval`
    drains, `flush_writes` runs after it — or an un-awaited task's
    buffered writes are silently dropped.
  - **`task.pool(n)` is the bounded form**, and deliberately not a gated
    `task.spawn`: `pool:spawn` returns *nothing*, so `await`/`poll` are the
    only path to a result and no second path has to agree about who claimed
    a failure. Identity rides on the task's own return value. Results come
    in completion order; `await`/`poll` **raise** on a drained pool rather
    than returning `nil` (a task may return `nil` first — the ambiguity
    `task.TIMEOUT` exists to avoid), and `done()`/`count()` are one fact
    read two ways. Pooled tasks are registry tasks, so the drain reports an
    uncollected failure exactly as for `task.spawn`, which is why
    `pool:cancel()` is the only way to abandon a sweep without failing the
    run. The permit is acquired *inside the driver*, before the body's first
    resume, so `spawn` never blocks and a queued task has run no script
    code. Reentrancy raises rather than deadlocking: a pooled task holds its
    permit while parked, so `IN_POOL` (a thread-local counter, raised by
    `InPool` across each *poll* of the body and **not** while it is parked —
    holding it across the whole body would make the main script's own
    `pool:await` look reentrant) gates `await`/`poll`. There is no deadline
    argument; `task.timeout` wraps a collection.
  - `require` from concurrent tasks is safe despite `ItoRequirer` being a
    single mutable navigator: Luau resolves an import synchronously within
    one resume, so no task interleaves mid-navigation. Covered by a test;
    don't "fix" it with locking.
  - `lua.sandbox(true)` is compatible with host-spawned threads — they see
    the full stdlib and globals.
  - Module tests that exercise an async surface need their own runtime;
    `proc.rs` and `task.rs` both have an `eval` helper showing the
    current-thread-plus-`LocalSet` shape to copy.

### `yamlito` crate (`crates/yamlito/`)

A standalone YAML parser and lossless CST editor. `ito` depends on it
for the `yaml` module via `builder::Yaml`. See
`crates/yamlito/CLAUDE.md` for its layout and details.
