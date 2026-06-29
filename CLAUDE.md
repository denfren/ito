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
  to the others, because a reader arrives at exactly one of them. The
  exception is `irun`, which is defined by its relationship to `ito` and
  so points at that chapter for everything they share.
- `README.md` — the capability summary and the `## Scripting` module list.
- `ito/cli.rs` — clap `help`/`long_about`/`about` strings are themselves
  user-facing; keep them in sync.
- `docs/src/intro.md` — only if the change touches what it documents (the
  `--doc` help workflow, the "no positional input / `-C` is the root"
  framing, or the exit-code summary).
- `docs/src/args.md` — only for the script-argument surface (`argv`):
  the `::` separator, and `irun`'s separator-free form.

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
- **One book, three front doors.** Every binary *with a `--doc`* embeds
  every topic, so the registry is shared and `--doc <topic>` reaches
  everything from any of them. A bare `--doc` prints the invoking
  program's own chapter followed by the topic list; `help::doc` takes the
  program name (from the clap `Command`) to pick it. Keep all four binary
  chapters in `TOPICS` — `irun` has a chapter but no flag to print it
  (it parses nothing), so its page is reached as `ito --doc irun`.
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

Every module chapter follows the same shape: a summary table, a short
motivating example, a general-rules/conventions section for whatever cuts
across multiple functions, then **one section per function** (or method)
with — only the subsections that apply, never a placeholder for one that
doesn't:

- one-line "what is it"
- signature
- parameters, described individually
- return value(s)
- error conditions (raise vs. `nil`/empty, precisely)
- an example

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

Four binaries share one Luau runner and one stdlib. Three of them differ
in what a script may reach; the fourth (`irun`) matches `ito` exactly and
differs only in who owns the command line. The capability boundary is
**the absence of the globals themselves**, not a policy check, so
`if fs.write then` is a usable feature test:

| | `iq` | `ied` | `ito` | `irun` |
| --- | --- | --- | --- | --- |
| read files, parse formats | ✓ (`-f`) | ✓ | ✓ | ✓ |
| `fs` namespace | rooted at `-C` | rooted at `-C` | the host filesystem | the host filesystem |
| `fs.write`/`fs.remove` | — | buffered | direct | direct |
| `fs.finder():grep()`/`:grep_files()` | — | — | ✓ | ✓ |
| preview writes (`-d`/`-l`/`-e`), commit (`-w`) | — | ✓ | — | — |
| `proc`/`ssh`/`aws.ec2`/`task`, `os.getenv` | — | — | ✓ | ✓ |
| `fs.cwd`, `ito.require_root`/`require_dir` | — | — | ✓ | ✓ |
| `time` value conversions, incl. named zones (bundled tzdb) | ✓ | ✓ | ✓ | ✓ |
| `time.now`/`parse`/`format_local` | — | — | ✓ | ✓ |
| ANSI styling methods, `ito.colors` | ✓ (auto = off) | ✓ (auto = off) | ✓ | ✓ |
| `ito.is_tty`, colour auto-detection | — | — | ✓ | ✓ |
| parses its own flags | ✓ | ✓ | ✓ | — |

Each *flag-parsing* binary has **its own clap `Parser`**
(`IqCli`/`IedCli`/`ItoCli`), so a flag exists only where it means
something — `iq --diff` is an unknown flag, not an accepted no-op. Shared
groups (`CommonArgs`, `InputArgs`, `ReportArgs`) are flattened in by
whichever binary applies them. `irun` has no `Parser` at all.

Per-binary specifics:

- **`iq`** — a filter. The script's **return value is the output** (must
  be a string, written to stdout verbatim); there is no `print`. `fs` is
  opt-in via `-f` and read-only; `-C` without `-f` is a usage error.
- **`ied`** — buffered edits. Writes queue and are applied **only** with
  `-w`/`--write`; a bare run computes the change set, reports on it, and
  drops it. `-d`/`--diff` and `-l`/`--list-changed` are mutually
  exclusive and each imply `-q`.
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
  for the same reason: `fs.cwd()`, plus `ito.require_root()`/
  `ito.require_dir()` (the `require` anchors as values —
  `require_dir` reads the *calling* chunk's name off the Luau stack, so a
  `require`d module gets its own directory; both are registered under
  `host_io` by `sl::ito::register_host_paths`). The clock joins them:
  `time.now`, plus `time.parse` and `time.format_local`, which may read
  the host's UTC *offset* (`sl::time::register_host_time`).
  No preview flags: nothing is held back to preview. Also carries
  `--ai-skill-init`.
- **`irun`** — `ito`'s engine composition with **no argument parsing at
  all**: no clap `Parser`, no `CommonArgs`, no `::`. `argv[1]` is the
  script path and every later argument is the script's, reaching it
  verbatim as `argv`. It exists for `#!/usr/bin/env -S irun`, where the
  binary and the script share one command line and the script should win
  it. Nothing is intercepted **on purpose** — not `-C`, not `-v`, not
  `--doc` — because claiming any spelling would take it from the script
  forever; don't "improve" this by adding a flag or a `--` terminator. It
  reuses `Startup` by building a default `CommonArgs` with only `script`
  set, so script resolution (and its shebang handling) has one
  implementation, not two. A bare `irun` is a usage error rather than the
  script listing: whoever reached for it already has a specific file.

Common to the three flag-parsing binaries: `<script>` (a **path** — no name lookup or search
list), `-` for stdin, `-s <str>` for inline, `-r <dir>` to give a
stdin/inline script an import root, `-C` (repeatable, make-style) for the
working directory and `fs` root, `-v`, `--doc [<topic>]`, `--completion
<shell>`. A bare invocation lists the scripts found in `./`, `ito/` and
`.ito/`. Script paths resolve against the **invocation cwd, before `-C`
applies** (so a `#!/usr/bin/env -S ito -C <dir>` shebang script is still
found — the kernel passes it as `./<script>`). The only other positional
is `INPUT`.

`INPUT` (`InputArgs`, flattened by all three) is an optional path or `-`,
read by `Startup::input` and installed as the `input` global by
`Engine::build` — raw, unparsed text, `nil` when omitted so a script
can tell "nothing passed" from "something empty passed". It is **not** a
capability: what a script may *reach* is the boundary between the
binaries, what it is *handed* is not, so the spelling is the same
everywhere and both runtime pieces were always binary-agnostic. `SCRIPT`
and `INPUT` cannot both be `-` (stdin is readable once); each binary's
`validate` delegates that check to `InputArgs::validate`. The read happens
*before* the engine is built, so a missing file fails before a script can
buffer an edit or spawn a command.

Also common to those three: a bare `::` ends the binary's own arguments.
Everything after it is the script's, reaching it verbatim as the global
`argv` (see `scripting/argv.rs`) — the only way to pass *those three* a
script's arguments. `irun` needs no separator, having no arguments of its
own to end.

Reports and `print()` go to stdout; all logging goes to stderr.

Exit codes: `0` success (no changes under `--exit-code`); `1` success
*with* changes (only under `-e`/`--exit-code`, `git diff --exit-code`
style — a "change" is a buffered write differing from disk); `2` generic
failure not the script's fault (usage, `-C`, script load, output/flush);
`3` the script's fault (parse/compile or runtime error). These are
modeled by `RunError { code, message }` in `run.rs` (`From<String>`
defaults to `2`; `RunError::script` is `3`).

### Workspace layout

A Cargo workspace (root `Cargo.toml`, `resolver = "3"`). Shared
dependency versions live in `[workspace.dependencies]`; members
inherit via `.workspace = true`. Members:

- **`ito/`** — the library crate plus **four** binaries (sources at the
  crate root, not `src/`; every `[[bin]]` and `[lib]` uses an explicit
  `path`). `bin/{ito,iq,ied,irun}.rs` are each a short `main` over the
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
| `bin/{ito,iq,ied,irun}.rs` | One `main` each: parse, `Startup`, name a `Config`, `Engine::build`, `eval`, emit. They differ in **which `Config` constructor they name** and what they do with the result — nothing else. |
| `run.rs` | The run phases. `Startup::begin` applies `-C` and captures the invocation cwd (script paths resolve against the latter, *before* the chdir, so a shebang script survives it); `Startup::script`/`input` load the sources; `handle_info_flags` handles the print-and-exit flags (`--doc` → `help::doc` with the program name, `--completion`). **`Config` is the whole of what differs between the binaries** — `fs: Fs::{None,Rooted{writes},Host}`, `host_io`, `ambient`, `stdout: Stdout::{Returned,Print{echo}}`, plus `input`/`argv` (uniform, not capabilities). `Config::iq`/`ied`/`ito` are the per-binary profiles; `irun` reuses `Config::ito` verbatim, so the two cannot drift. `Engine::build` is the *only* constructor: it makes the `mlua`/Luau state (`mlua::Lua::new()` installs Luau's default `require`, **replaced** by ito's `ItoRequirer` via `scripting::require::register` — confined to the entry script's directory tree, the *require root*, independent of the `fs`/`-C` root; see `docs/src/luau/require.md`), registers the core, `cli`+`argv`, `fs`, host I/O, drops ambient state, installs `print`/`input`, then `j2`, then `sandbox(true)`. Registration order is settled *inside* `build` rather than being a rule callers follow: `proc` before `ssh` (redirect targets), `j2` after the `fs` choice (`Fs::Rooted` → `j2::register`, `Fs::Host` → `register_host`, `Fs::None` → `register_without_fs` — an include is a file read, so it gets exactly the access `fs` got), `sandbox` last (it freezes the tables `remove_random`/`add_getenv`/`time::register_host_time` reach into). `Engine::vfs()` is an `Option` (`Some` only under `Fs::Rooted`), and only `ied` calls it. `Engine::eval` runs the chunk (named `@<abs path minus extension>`, so `require`'s caller-detection — which keys off the chunk name, Luau's `debug.info` convention — resolves relative to the entry script), then calls a global `main` if defined, then **drains outstanding `task`s**. Afterwards `emit_returned` (`iq`), `emit_reports` and `flush_writes` (`ied`); the drain-before-flush rule is `eval`'s own doing. Everything is `async` under a current-thread tokio runtime + `LocalSet` (`main_with`). Also holds `RunError`/`Outcome` and `init_tracing`. |
| `cli.rs` | clap layer: one `Parser` per binary (`IqCli`/`IedCli`/`ItoCli`), each flattening the shared groups it applies — so a binary's flag set is what its struct declares, with no runtime filtering. `CommonArgs` is script source + `-C` + `-v` + the `--doc`/`--completion` exits; `ReportArgs` is `-w`/`-d`/`-l`/`-e` (only `IedCli`). `ParseWithArgv` is a blanket trait that splits the command line at the first bare `::` before clap sees it — everything after is the script's, so no binary can forget to. |
| `help.rs` | `--doc` implementation: the `TOPICS` registry (`include_str!`s `docs/src/**`, including all three binary chapters), topic lookup, the grouped listing, the per-program default page, and the styled-markdown-through-a-pager `emit` (honors `--doc-plain`/`--doc-no-pager`/`NO_COLOR`). |
| `lib.rs` | Library crate root: `pub mod cli; pub mod help; pub mod run; pub mod scripting;`. `run` and `cli` are public because the binaries live outside the library. |

### Scripting standard library (`ito/scripting/`)

Each module lives in its own file (or directory) and exposes a Rust
`register(&Lua, …)` that installs a global table. **The Rust file name
matches the Luau global**, so `mod.rs` reads like the script-visible
surface: `fs` is the exception — it has *two* implementations, `vfs.rs`
(the rooted VFS `iq`/`ied` get, named for the type) and `rfs.rs` (the
real filesystem `ito` gets) — as is `redirect.rs`, which is not a module
of its own (see below). The
per-module doc comment at the top of each file is the authoritative
surface description; consult it rather than duplicating it here.

Globals: `fs`, `path`, `time`, `json`, `toml`, `yaml`, `hcl`, `j2`, `re`,
`log`, `io`, `ito`, `proc`, `ssh`, `aws` (holding `aws.ec2`), `task`, `argv`,
`cli`, `ui` (holding `ui.prompt`, `ui.progress`), plus `string` method
additions. `mod.rs` re-exports `Argv`, `Vfs` and `PathMapper`.

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
  `ansi::enabled()` (made `pub` for this) so a prompt's colour tracks
  `ito.colors()` the way `string:red()` does, rather than a separate switch.

There is **no injected-global magic left**: no `MAIN`, no `ITO_VERSION`,
no `debug_print`. A `main` function marks the entry point (the runner
calls it when present, and *not* for `require`d modules),
`ito.version_check` asserts compatibility, and `log.debug` logs.

`cli.rs` is the *opt-in* parser over those arguments, registered by
`Engine::build` for all four binaries. `cli.opt`/`cli.arg` declare into
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
registered for every binary despite writing to stdout, which `iq` (whose
return value *is* stdout) and `ied` (whose reports own it) otherwise hold — both
can still have it because what it prints is **terminal**: the run unwinds
out of `eval`, so it never reaches `emit_returned`/`emit_reports`/
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

`argv.rs` is the script's own arguments: `Argv::split` partitions the raw
command line at the first bare `::` (the CLI does this before clap), and
`register` installs the tail as the global `argv` — a plain list of
strings, **not** prefixed with the program name. Nothing is parsed for the
script: there is no option map, no type coercion and no name validation,
because every script wants a different grammar. `ito.opt`/`ito.flag` and
the `key=value`/`+flag`/`-S`/`-N`/`-B` forms they read are gone; don't
re-add them.

A couple of notes that aren't obvious from any single file:

- **Two `fs` implementations, one Luau global.** `vfs.rs` is the rooted
  VFS (`iq`/`ied`): a `PathMapper` translates a root-relative
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
  exposing them on `iq`/`ied` means extending containment to reads.
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
  buffered writes. `Denied` (`iq`) registers no write functions at all.
  There is no `Direct` — `ito` gets `rfs` instead, which writes
  immediately and also carries the operations with no previewable form
  (`fs.mkdir`/`rmdir`/`rename`/`copy`).

  There is deliberately **no script-facing `fs.flush`/`fs.clear`**: when
  the buffer reaches disk is the host's call alone, which is what lets
  `--diff`/`--list-changed` promise that nothing was written. Don't
  re-add them.
- **`time` is one global registered in two halves** (`time.rs`), the same
  shape `ito.rs` uses: `register` installs the pure value conversions
  everywhere, `register_host_time` adds the clock (`now`)
  and the two functions that may read the host's UTC offset
  (`parse`/`format_local`) under `host_io`. The line is *ambient state*,
  not "does it touch dates": RFC 3339 requires an offset, so parsing a
  full timestamp is a pure conversion, while an offset-less
  `"2026-08-10"` needs the local zone to place it and so cannot be in the
  shared half. `time.parse` is therefore host-only **in full** rather
  than being a shared function that sometimes reads ambient state — a
  script tests `if time.parse then` and falls back to
  `time.parse_rfc3339`. **Named zones are on the pure side**: the zone
  arrives as an *argument* and resolves against the **bundled** tzdb
  (`tzdb-bundle-always`, ~310KB), so `format_tz`/`parse_tz`/`zone_offset`
  give the same answer on every machine and live in all four binaries.
  That is also why the tzdb is bundled rather than read from the system
  (`tzdb-zoneinfo`) — a host-dependent lookup would make them ambient and
  force them out of `iq`/`ied`. The cost is rules frozen at build time,
  which a release refreshes. There is deliberately **no**
  `zone_offset(t)` defaulting to the host zone and no `local_zone()`:
  either would smuggle ambient state into a function that looks pure.
  `format_offset` (a *fixed* offset) is separate from `format_tz` because
  a fixed offset is DST-blind, and conflating them would hide that; each
  rejects the other's argument, and both reject an offset/zone passed
  where a strftime pattern belongs (a pattern without `%` formats to
  itself, so that mistake used to return plausible-looking literal text).
  `register_host_time` also rebinds
  `diff`/`add`/`sub` to the same implementations with the local offset
  supplied, so the calendar and coercion behaviour cannot drift between
  the two registrations. Backed by `jiff` with `default-features = false`
  (no tzdb bundle: `tz-system` reports the host's *offset*, which is all
  these render). **Deltas are one surface, not a layered pair**:
  `time.delta` is the single coercion (number → seconds, string →
  `humantime::parse_duration`) that *every* delta-taking function calls,
  so `add`/`round`/`format_delta`/`task.wait` cannot disagree about what
  `1h30m` means — `task` already used that grammar, and the delta surface
  moved onto it rather than keeping a second one. There is no ISO 8601
  spelling and no `parse_delta*` family; don't re-add them. Months and
  years are rejected *before* humantime sees them (`calendar_unit`),
  because it accepts both against fixed 30.44/365.25-day approximations —
  exactly the invented answer this module refuses; `add`/`sub` take them,
  having a timestamp to anchor. **Deltas are unsigned** — a duration is a
  magnitude, `"-90s"` is not a spelling — but subtraction is not, so
  `format_delta` prints a negative with a leading `-` rather than
  refusing the caller's own arithmetic. `format_delta` is hand-rolled
  (`d`/`h`/`m`/`s`) because neither candidate renders it: jiff's
  `SignedDuration` is calendar-less and stops at the hour (4 days prints
  as `96h`), humantime's formatter spells large units out (`4days 2h`)
  and approximates years/months. **The second is the display
  resolution** — the sub-second part is truncated, since a computed delta
  is fractional and rendering it gave `173h 44m 24s 737ms 129µs 927ns`;
  the clock itself is left alone, so a *parsed* timestamp keeps the
  precision it was written with. `time.round` truncates a delta **toward
  zero** (an age must not overstate) and returns **one** value, so it
  composes into `format_delta`; `round_with_rest` is the two-value form,
  split off because a trailing multi-value call expands into the next
  call's *arguments* — `format_delta(round(d, "h"))` silently passed the
  remainder as the formatter's second argument. Its units stop at the
  second below (nothing finer is displayable) and the week above (for the
  reason a month is not a delta). Numbers, not strings: rounding and
  formatting are separate questions. There is no `time.since`/`until`:
  an age is `time.diff(t, time.now())`, which puts the direction at the
  call site instead of in a name whose sign convention must be recalled.
- **`ansi.rs` is a Rust module but not a Luau one**, like `redirect.rs`: it
  installs its styling onto the **`string` table** (`("x"):red()`), because a
  style applies to text and reads better with the text on the left. It is
  registered in **two halves** for the reason `time` is: `register` +
  `register_switch` (the pure string transforms and `ito.colors`) go
  everywhere, while `register_host_detect` (`ito.is_tty`, and the right for
  `ito.colors(nil)` *auto* to detect) is host-only — auto reads stdout's
  terminal-ness and `NO_COLOR`/`CLICOLOR`/`CLICOLOR_FORCE`
  (<https://bixense.com/clicolors/>), which is ambient state an `ied --diff`
  must not see. So **auto resolves to off under `iq`/`ied`** and a script
  there opts in with `ito.colors(true)`; the methods themselves are pure and
  exist in all four. The switch is a process-wide `AtomicU8` (one run, one
  answer, and a `require`d module's setting is visible to its importer), which
  is why `Engine::build` calls `ansi::reset()` — two builds in one process
  (the test suite) must not inherit each other's state. `ito.colors` is a
  function taking a `Variadic`, not an `Option<bool>` field: reading has to
  *resolve* auto, and `ito.colors()` (read) must be distinguishable from
  `ito.colors(nil)` (reset to auto). Each style closes with the **specific**
  reset (`39`/`49`/`22`…), not the blanket `0`, so `("x"):red():bold()` keeps
  both — don't "simplify" that to `\27[0m`. `if_color`/`if_nocolor` are the
  alternative-text pair (one optional argument, `""` when omitted); they are
  **eager selectors, not guards** — Luau has evaluated both sides before
  either is called, and laziness is what a plain `if ito.colors()` is for.
  They deliberately **do not inspect the receiver** for escape sequences: the
  switch decides, which keeps `("[ok]"):if_nocolor("✔")` meaningful and stops
  the answer depending on whether an inner style emitted anything. No 256-colour/truecolour
  constructor and no `ansi.style{…}` builder yet; `string.width` counts
  escape-stripped `chars()` rather than pulling in `unicode-width`, so it is
  escape-aware but not a full terminal width model.
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
