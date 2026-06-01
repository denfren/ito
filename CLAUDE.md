# ito

## Workflow

- Make sure the project builds (`cargo build`) before committing.
- Commits that land on a branch (and any commit on a release tag) follow
  [Conventional Commits](https://www.conventionalcommits.org/):
  `type(scope): description`. This requirement takes precedence over any
  general "commit often / messages don't matter" guidance — those loose,
  intermediate commits are fine while iterating, but the history that
  ships must conform so the changelog is complete and accurate. Allowed
  types are `feat`, `fix`, `docs`, `refactor`, `test`, `chore`. The scope
  is optional and free-form (e.g. `feat(yaml): ...`). Mark breaking
  changes with `!` in the subject (e.g. `feat(cli)!: ...`). Only
  `feat`/`fix`/`docs` surface in the changelog.
- The changelog lives at `CHANGELOG.md` (repo root, Keep a Changelog
  format) and is **generated** from commit history by
  [`git-cliff`](https://git-cliff.org/) — do not hand-edit it. Config is
  `cliff.toml`. Regenerate with `git cliff -o CHANGELOG.md`; on release
  bump `ito/Cargo.toml` `version`, tag `vX.Y.Z`, then regenerate.
  `ito/help.rs` `include_str!`s the result for the `ito help changelog`
  topic, so the cliff template stays Keep-a-Changelog-shaped.

### Documentation to update on surface changes

When the **CLI surface** changes (flags, subcommands, exit codes,
stream routing, defaults):

- `README.md` — the `## CLI` section: subcommand list, the flag tables
  (script source, working directory, script arguments, output/logging),
  the exit-code table, and any affected `## Examples`.
- `ito/cli.rs` — clap `help`/`long_about`/`about` strings are themselves
  user-facing; keep them in sync with the README.
- `docs/src/intro.md` — only if the change touches what it documents
  (the `ito help` help workflow, the "no positional input / `-C` is the
  root" framing, or the exit-code summary).
- `docs/src/args.md` — only for the arg-injection flags
  (`-S`/`-N`/`-B`) or the `MAIN` global.

When the **Rhai/stdlib surface** changes (functions, modules, builders,
injected globals):

- `docs/src/modules/<module>.md` — the per-module chapter. These files
  are the single source for both the website and the embedded `ito help`
  help (see below).
- `docs/src/rhai/*.md` — the language chapters, if Rhai language usage
  guidance changes.
- `ito/help.rs` — the `TOPICS` registry: it `include_str!`s the
  `docs/src/**` chapters and drives `ito help` / `ito help <topic>` /
  `ito help all`. **Adding a new module = add a `docs/src/modules/<name>.md`
  chapter, a `TOPICS` entry, and a `docs/src/SUMMARY.md` line.**
- `docs/src/SUMMARY.md` — the mdbook table of contents; add/remove a line
  when a chapter is added/removed.

Notes:

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
  website and the embedded `ito help` help. Editing the chapter is
  usually the only content change needed; the registry/SUMMARY edits are
  wiring.

## Architecture

`ito` is a Rhai script runner with a small standard library for
inspecting and generating config files. A script gets a virtual
filesystem (`fs`) rooted at the working directory, plus modules for
structured formats and helpers (see `ito/scripting/` for the current
set).

Most of the stdlib is a safe sandbox: pure computation plus buffered
VFS writes that only touch disk after the script returns. The
exceptions are gated behind `--unsafe-*` flags (e.g. `--unsafe-proc`
registers `proc`, which spawns processes; `--unsafe-fs-flush` lets a
script flush the VFS mid-run) — they break the sandbox premise, so they
are off by default.

The CLI is subcommand-based:

- `ito exec <str>` — run an inline expression/program.
- `ito run [<path>]` — run a script file (or `-` for stdin); with no
  path, discovers a single default script in the working directory.
- `ito help [<topic>]` — print scripting help (no topic lists topics,
  `all` dumps everything); pages styled markdown through `$PAGER`/`less`.
- `ito util <command>` — utility subcommands; currently
  `ito util completions <shell>` (shell completion script to stdout).

The only path input is the working directory (settable via repeatable,
make-style `-C` flags), which becomes the `fs` root; there is no
positional file input besides the script itself.

Output has two orthogonal axes shared by `exec`/`run`: an **action**
axis (`-n`/`--dry-run` computes buffered writes but does not flush) and a
**report** axis (`-d`/`--diff`, `-l`/`--list-changed`, `-q`; `--diff` and
`--list-changed` are mutually exclusive and each imply `-q`). Reports
and `print()` go to stdout; all logging goes to stderr.

Exit codes: `0` success (no changes under `--exit-code`); `1` success
*with* changes (only under `-e`/`--exit-code`, `git diff --exit-code`
style — a "change" is a buffered write differing from disk); `2` generic
failure not the script's fault (usage, `-C`, script load, output/flush);
`3` the script's fault (parse/compile or runtime error). These are
modeled by `RunError { code, message }` in `main.rs` (`From<String>`
defaults to `2`; `RunError::script` is `3`).

### Workspace layout

A Cargo workspace (root `Cargo.toml`, `resolver = "3"`). Shared
dependency versions live in `[workspace.dependencies]`; members
inherit via `.workspace = true`. Members:

- **`ito/`** — the binary + library crate (sources at the crate root,
  not `src/`; `[[bin]]` and `[lib]` use explicit `path`).
- **`crates/yamlito/`** — the standalone YAML parser/CST-editor crate
  (see below). A dependency of `ito`: the `yaml` module is backed by its
  value model (`yamlito::Yaml`), though the CST-editing layer is not yet
  exposed.

### `ito` crate layout (`ito/`)

| File | Responsibility |
| --- | --- |
| `main.rs` | Entry point and runner. Parses CLI and dispatches on the subcommand: `exec`/`run` go to `run_script`, `help` to `ito::help::doc`, `util completions` to `generate_completions`. `run_script` resolves the working directory from the `-C` flags (make-style: each relative to the previous, absolute resets), builds the Rhai engine, sets the `imports::ImportResolver` (so `import` resolves relative to the script's dir — the VFS root for inline/stdin — and is sandboxed to the root when the script lives inside it), registers the stdlib modules and logging globals, binds injected args + `MAIN`, compiles and evals the script, prints any `--list-changed`/`--diff` report, then flushes the VFS unless `--dry-run`. Returns an `Outcome` (drives `--exit-code`) or a `RunError` (carries the `2`/`3` exit code). |
| `imports.rs` | `ImportResolver`: a `rhai::ModuleResolver` wrapping a base-less `FileModuleResolver` with an optional `PathResolver` sandbox. It computes the absolute target per hop — top-level imports against the entry `base_dir`, nested imports against the importing module's own directory (`source`'s parent), so nested imports chain relative to the importing file, not the entry script. When the entry base is `contained()` under the VFS root, *every* hop's target is containment-checked and escapes rejected; outside the root it is plain file resolution. |
| `cli.rs` | clap layer: `Cli` with a `Command` subcommand enum (`Exec`/`Run`/`Help`/`Util`), plus a nested `UtilCommand` enum (`Completions`). `RunArgs` (shared `-C`, the `--unsafe-*` run-control flags, `ScriptArgsFlags`, `OutputArgs`) flattens into `exec`/`run`; `OutputArgs` is the output/exit-code group (`-n`, `-d`/`--diff`, `-l`/`--list-changed`, `-q`, `-e`/`--exit-code`); `ScriptArgsFlags` is `-S`/`-N`/`-B`; `LoadedScript` is the loaded source. |
| `help.rs` | `ito help` implementation: the `TOPICS` registry (`include_str!`s `docs/src/**`), topic lookup, the grouped listing, and the styled-markdown-through-a-pager `emit` (honors `--plain`/`--no-pager`/`NO_COLOR`). |
| `util.rs` | Small shared helpers (`init_tracing` — logs to stderr). |
| `lib.rs` | Library crate root: `pub mod help; pub mod scripting;`. |

### Scripting standard library (`ito/scripting/`)

Each module lives in its own file (or directory) and exposes a Rust
`register(&mut Engine, …)` that installs a namespaced static module
(`fs::`, `path::`, `json::`, `toml::`, `yaml::`, `hcl::`, `j2::`,
`proc::`, `ito::`, plus `string` method overrides). `mod.rs` lists the
modules and re-exports `ScriptArg`, `Vfs`, and `PathMapper`. The
per-module doc comment at the top of each file is the authoritative
surface description; consult it rather than duplicating it here.

`args.rs` defines `ScriptArg` (the typed inject-a-global enum used by
the CLI) and a `register(&mut Scope, …)` that pushes args as globals
rather than as a module.

A couple of notes that aren't obvious from any single file:

- **VFS buffering** (`vfs.rs`): `fs::write`/`fs::remove` are buffered in
  `Rc<RefCell<Pending>>` (a per-path `BTreeMap<PathBuf, Action>` shared
  by the module's closures and the host's `Vfs`); last call wins. The
  host flushes after the script returns (unless `--dry-run`); scripts
  cannot read their own buffered writes (reads hit disk). `fs::flush`
  exists but is gated behind `--unsafe-fs-flush`.
- **`proc`** (`proc.rs`) is only registered under `--unsafe-proc`.
- YAML is backed by the `yamlito` crate's value model (`yamlito::Yaml`);
  `yaml::parse`/`to_string` are not a round-trip (emitting owns the
  layout).

### `yamlito` crate (`crates/yamlito/`)

A standalone YAML parser and lossless CST editor (lexer, parser, rowan
syntax tree, typed AST, surgical edits, fixers, and Rhai cursor glue
under `scripting/`). Internal paths use `crate::*`. The library layer
(parse, AST, edits, fixers) is fully enabled. `ito` depends on it for
the `yaml` module via `builder::Yaml` (a serde value model, plus
`Yaml::from_tree` for text→value); the rest — surgical edits, fixers,
and the Rhai **scripting** glue (`scripting/`) — compiles but is not
consumed by any binary yet.

Tests live in `crates/yamlito/tests/`: `parser.rs` (CST/AST insta
snapshot tests over `tests/yaml/**/*.yml`, snapshots in
`tests/snapshots/`). These are the library-only tests and run via
`cargo test -p yamlito`.
