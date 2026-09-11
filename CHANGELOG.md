# Changelog

## [0.7.0] - 2026-11-10

### Added

- `fmt` module (`ito/scripting/fmt/`): the colour switch (`fmt.colors`,
  host-only `fmt.is_tty`), styles as `fmt.red(s)`/`s:red()`, status symbols
  (`fmt.ok`/`fail`/`warn`/`info`), `fmt.strip`/`fmt.width`, text helpers
  (`trimmed*`, `truncated`, `cropped`, `replaced_at`, `padded_start`/
  `padded_end`, `removed`, `replaced`, `starts_with`/`ends_with`/`contains`,
  `without_prefix`/`without_suffix`, `lines`) and `fmt.table.new`.
- `encoding` module: `encoding.base64.encode`/`decode` and an `engine()`
  builder (`:alphabet`, `:padding`, `:decode_padding`).
- `hash.sha224`/`sha384`, and `hash.<algo>_file_bytes` (raw-buffer file digests).
- `ansible.vault`: `encrypt(plain, key)`/`decrypt(vault, key)` implementing
  the Ansible Vault 1.1 (`AES256`) format, plus `is_vault(vault)`.
  `decrypt` works under `ied` too; `encrypt` is `ito` only.
  `ansible.vault.load_password_file(path)` resolves a vault password the way
  `ansible-vault --vault-password-file` does — it executes `path` and takes
  its stdout if executable, otherwise reads it as plain text — and is `ito`
  only.
- Global `null` and `table.array(t)` (array marker: an empty table serializes
  as `[]`); `json.parse_lines`/`to_string_lines`; an optional name argument to
  the parsers, used in parse errors; `stream:add` on `yaml` streams.
- `fs.stat`, `fs.append`, `fs.chmod`, `fs.temp_dir`, `fs.temp_file`;
  byte-safe `fs.read`/`fs.write`; `fs.script` (`ito` only).
- `re`: `compile`, `escape`, `match`, `count`, and match positions from `find`.
- `path` and `j2` additions: new filters, Python-compat methods and
  `with_filter`.
- `proc`: distinct `proc.STDOUT`/`proc.STDERR` aliases, `:stdin`, `:envs`,
  `:env_remove`, `child:try_wait`, append mode for `proc.file`, `proc.pid`.
- `ssh`: `conn:close`, `cmd:interactive`, `child:try_wait`; commands are
  shell-escaped; `ssh.connect(host, { tofu = true })` for trust-on-first-use.
- `time`: `minutes`/`hours`/`days` builders (count → seconds),
  `truncate(secs, unit)`/`count(secs, unit)` (floor a delta to whole units /
  return the unit count), host-only `till(t)`/`ago(t)`, and
  `format_civil_*`; months and years are accepted by `add_*`/`sub_*`.
- `task.status`, `pool:concurrency()`.
- `cli`: an options table (`default`, `required`, `choices`, `value_name`,
  `details`) and `cli.parse(list)`.
- `io.stdin` and reading from it (`io.read`/`io.lines`), `ito` only.
- `aws`: `aws.autoscaling` `:launch_template`, `describe_scaling_activities`
  and `describe_policies`; `aws.ec2.describe_instance_status`; the new
  `aws.cloudtrail` module (`lookup_events`); the new `aws.cloudwatch` module
  (`get_metric_statistics`/`list_metrics`); CloudTrail `envelope.truncated`,
  CloudWatch `:max_metrics`, and RFC 3339 strings accepted wherever a time
  argument is taken.
- `ui.prompt.multi_select_builder():all()`: pre-check every item, so
  selecting all by default needs no loop over `:checked(i)`.
- `yaml.edit`: one scalar quoting method per style, plus an auto mode —
  `cursor:unquote()`, `cursor:single_quote()`, `cursor:double_quote()` and
  `cursor:auto_quote()` — and `cursor:quote_style()` reads a scalar's
  current style. `cursor:exists()` reports whether a path resolves, and
  `try_kind`/`try_len`/`try_to_string`/`try_raw`/`try_read`/`try_write`/
  `try_remove` are non-raising siblings of the resolving calls. `"alias"`
  is a cursor kind of its own, with `cursor:is_alias()`.
- `hcl.edit`: `h:kind()` names the resolved node's kind (`"body"`,
  `"block"`, `"object"`, `"array"`, `"expr"`), reading the same value the
  `is_*()` predicates do, plus `try_kind()`.
- `hcl.edit`/`yaml.edit`: a read-modify-write structural surface —
  `add`/`add_at`/`update`/`update_at`/`update_all`/`upsert`/`upsert_at`/
  `remove`/`remove_at`, shared in shape by both cursors. `update`/`upsert`
  variants call a function with the current decoded value (`nil` if absent
  for `upsert`) and write back its return value, removing the entry on a
  `nil` return; `add_at`/`upsert_at` place a new entry at a given position
  instead of always appending. `yaml.edit`'s `remove()` gained a
  no-argument self-removal form to match `hcl.edit`'s.
- `yaml.edit`/`hcl.edit`: `cursor:raw()`/`h:raw()` (with `try_raw`) read a
  node's exact source text as an opaque handle, reserving the name for a
  future verbatim-write escape hatch.

### Changed

- **Breaking:** `print` is vanilla Luau: `tostring` of each argument (so
  `__tostring` is honoured), tab-joined, newline, to stdout. A plain table now
  prints `table: 0x…` instead of JSON (`log.*` still renders JSON). It is
  never rerouted to the log; `-q` (`ied` only) makes it a silent no-op. A
  closed pipe (`| head`) ends the run quietly with exit **141** (128 +
  SIGPIPE) so the consumer can tell it was cut short; this also applies to
  `io` writes and to `ied -d`/`-l` reports, and a `pcall` cannot hide it.
- **Breaking:** `io` (`ito` only) is now file-like handles — `io.stdout`,
  `io.stderr`, `io.stdin` with `:write` (chains; strings and numbers only),
  `:flush`, `:close` (raises), `:setvbuf` (no-op), and for stdin `:read(fmt)`
  and `:lines()` — plus `io.write`/`read`/`lines`/`flush` on the defaults.
  `io.read()` now reads one line (vanilla); `io.read("a")` reads the rest.
  All writes are flushed immediately. There is no `io.open`/`io.output`/
  `io.input`.
- `ied -q` silences the script's `print` and nothing else; `-d`/`-l` imply
  it, so a report is never mixed with script output.
- **Breaking:** the command line is now `<binary> [<flags>] <script>
  [<args>...]`. Everything after the script path reaches the script
  verbatim as `argv` (and `cli.parse`), flag-looking tokens included, so a
  `#!/usr/bin/env -S ito` script owns its whole command line. The `::`
  separator is gone.
- **Breaking:** `ito.pid` → `proc.pid`; `ito.require_root`/`require_dir` →
  `fs.require_root`/`fs.require_dir` (host-only). `ito` keeps only
  `version_check` and `slug`.
- **Breaking:** `ito.null`/`json.null`/`ito.is_null` → the global `null`.
- **Breaking:** the `*_pretty` variants → `json.to_string(v, { pretty, indent })`
  (likewise `toml`); object keys are written sorted; TOML datetimes parse to
  strings; `yaml` write works on any node and `remove_at` with an
  out-of-range index raises.
- **Breaking:** `ito.colors`/`ito.is_tty` → `fmt.colors`/`fmt.is_tty`;
  `tablefmt` → `fmt.table` (`t:print` dropped); string methods renamed:
  `padded` → `padded_start`/`padded_end`, `set` → `replaced_at`,
  `stripped` → `without_ansi`.
- **Breaking:** `fs.rmdir` → `fs.remove_all`; the rooted VFS rejects a
  leading `/`; `re` capture keys are integers.
- **Breaking:** `ssh` checks host keys strictly by default; exit codes `127`
  and `255` are returned rather than raised.
- **Breaking:** `time`'s parse/format functions are renamed to
  `parse_<format>[_<zone>]`/`format_<format>[_<zone>]`: `parse`/`format`
  are gone, replaced by `parse_rfc3339`/`format_rfc3339_utc`/
  `format_rfc3339_tz`/`format_rfc3339_local`, `parse_civil_utc`/`_tz`/
  `_local` (an offset-less date/date-time), and `parse_fmt_utc`/`_tz`/
  `_local`/`format_fmt_utc`/`_tz`/`_local` (a strftime pattern, replacing
  `parse_strftime`). `format_offset`, `format_tz`, `parse_tz`, and
  `zone_offset` are gone — use the `_tz` family (a fixed offset is now
  reached via `format_fmt_tz(t, "%:z", zone)` rather than a dedicated
  function). `time.delta` is renamed to `parse_delta`. `time.add`/`sub`
  are renamed to `add_utc`/`sub_utc`, with `add_tz`/`sub_tz` and
  `add_local`/`sub_local` siblings taking the anchor zone explicitly.
  `time.round`/`round_with_rest`/`diff` are gone — use `truncate`/`count`
  and `till`/`ago` (see Added).
- **Breaking:** one delta grammar (`time.parse_delta`); a day is a fixed 24h;
  `_local` uses the host zone with its DST rules.
- **Breaking:** `cli.assert` requires a boolean; help uses a `Usage:`
  heading. `ui.prompt.select` returns `index, item` and prompts raise
  without a tty; `ui.progress.suspend` is async and `clear` only erases.
- **Breaking:** `aws.ec2.delete_tags` with nothing to delete raises; empty id
  lists raise; `aws.s3.get_object` always returns a string (use `:parse`);
  boolean arguments are strict; `:profile()` beats the environment.
- Logging: `-v` is debug, `-vv` is trace.
- `require` canonicalizes the root and module paths, excludes a sibling of
  the root, and keeps the dot in dotted names.
- Documentation and `--help` text now treat `ito` as the default and call out
  only what is unavailable in `ied`. New "Reproducibility" section in the
  `ied` chapter: a run depends only on the work-directory state and the
  script, which is why the clock, environment, randomness, commands, network
  and paths outside the work directory are absent.
- **Breaking:** a `yaml.edit` cursor now carries a *path* and resolves it
  against the document on every call, instead of caching a syntax node —
  the model `hcl.edit` already used. Consequences:
  - Traversal (`m[key]`, `s[i]`, `:get()`) no longer resolves, so it no
    longer raises: a missing key or out-of-range index surfaces at the
    next read or write, naming the path it got through. Use `:exists()`
    to check up front.
  - `:kind()` and the `:is_*()` predicates resolve, so they raise on a
    path that is not there. `:try_kind()` returns `nil` instead.
  - The `"null"` kind and `:is_null()` are **gone**, along with the
    null-object cursor. A path that does not resolve raises on use
    rather than quietly answering `0`/`{}`/`""`.
  - `:values()` and `:entries()` hand back **key-addressed** cursors, so
    they survive edits to sibling entries. Sequence selectors remain
    position-addressed, and shift when an earlier element is removed.
- `yaml.edit` quoting conversions now decode the scalar and re-encode it
  in the requested style, so every pair of styles is reachable (`'it''s'`
  → `"it's"`) instead of only those whose raw text transferred.
- **Breaking:** `yaml.edit`'s scalar quoting methods are renamed to verb
  form — `make_unquoted`/`make_single_quoted`/`make_double_quoted`/
  `make_auto_quoted` are now `unquote`/`single_quote`/`double_quote`/
  `auto_quote` — and each now **returns a `bool`** for whether the
  conversion happened instead of raising when the style can't represent
  the value; there are no more `try_make_unquoted`/`try_make_single_quoted`
  twins, since the bool return already is the non-raising form. Each
  still raises on a non-scalar receiver.
- **Breaking:** `hcl.edit`'s `read_raw`/`try_read_raw` are renamed to
  `raw`/`try_raw`.
- **Breaking:** `hcl.edit`'s overloaded `add(...)` is replaced by
  `add`/`add_at`/`update`/`update_at`/`update_all`/`upsert`/`upsert_at`
  (see Added), and `remove(...)` gains a companion `remove_at(index)`.
- **Breaking:** `yaml.edit`'s `append`/`insert_at`/`append_or_update` are
  replaced by `add`/`add_at`/`update`/`update_at`/`update_all`/`upsert`/
  `upsert_at` (see Added); `append_or_update(key, value)`'s signature
  changes shape under the rename — `upsert(key, upsert_fn)` takes a
  function, not a plain value.

### Removed

- **Breaking:** the `iq` binary, with its "return value is the output"
  mode, `-f/--fs` and the `iq` chapter. Query with an `ito` script, or with
  `ied` without `-w` (which computes and drops).
- **Breaking:** `io` in `ied` (so its only stderr path is `log.*`), and
  `io.ewrite`/`io.ewriteln`/`io.writeln` everywhere; use `io.stderr:write(...)`
  / `io.stdout:write(..., "\n")` in `ito`, or `print`.
- **Breaking:** `ito -q/--quiet`. Scripts control their own output; `-v`/`-vv`
  only raise log verbosity.
- **Breaking:** the `irun` binary. Use `ito`, which no longer claims any
  argument after the script.
- **Breaking:** the `INPUT` positional and the `input` global.
- **Breaking:** reading the script from stdin (`-`), inline scripts
  (`-s`/`--script`), and `-r`/`--require`. The script is always a file.
- **Breaking:** the old `time` delta spellings (ISO 8601), `tablefmt` and
  `ansi`.

### Fixed

- `ssh` commands are shell-escaped rather than joined verbatim.
- `ui.prompt` and `ui.progress` pick their auto colour/theme by whether
  **stderr** (where they draw) is a terminal, not stdout, so
  `ito script > out` keeps coloured prompts and bars.
- `ui.prompt`'s builders (`confirm_builder`, `input_builder`,
  `password_builder`, `select_builder`, `multi_select_builder`) now
  actually chain — every setter returns the builder instead of `nil`,
  matching the module's own documented `:prompt(...):default(...)`
  usage and `ui.progress.style()`'s existing convention.
- `yaml.edit` cursors could **silently corrupt a document**. Because an
  edit rebuilds the syntax tree, a cursor taken beforehand pointed into
  the discarded one: after removing an earlier sibling, a held cursor
  read its own (stale) value but *wrote* to whichever node had taken its
  position. Paths are resolved against the current tree, so reads and
  writes can no longer disagree.
- `yaml.edit` could abort the process. A cursor whose position no longer
  existed panicked inside `yamlito` (`stale edit target: no node at
  child index …`), escaping `pcall` and exiting 101 rather than ito's
  documented script-error code. It is now an ordinary, catchable error,
  and `yamlito`'s own `node_at_path` no longer panics on a stale path.
- `yaml.edit`'s `:entries()`/`:values()` returned entries that a later
  removal had deleted, because the array captured nodes rather than
  addresses.
- `yaml.edit` reported an implicit null (`key:` with no value) as kind
  `"null"` and refused `:read()`; it is a scalar and reads as `null`.
- `yaml.edit` reported an alias (`*anchor`) as kind `"null"`: the kind
  test matched the alias *token* rather than the node, so the intended
  branch was unreachable. It now reports `"alias"`.
- `yaml.edit`'s `cursor:write()` and `cursor:rename_key()` could emit a
  value that re-read as something else: they chose between plain and
  single-quoted style only, so a string with a newline or with leading /
  trailing whitespace (`" pad "` → `pad`) was silently altered. Both now
  use the shared quoting policy, which falls through to double quotes.
- `unquote` no longer reports success for a value containing a control
  character (it emitted a raw tab into the document), and
  `single_quote` no longer reports success for a value with
  leading/trailing whitespace that single-quoted style would drop.
- `aws.config():profile(p):build()` failed with "Missing Region" for
  assume-role profiles (`role_arn` + `source_profile`): the STS client behind
  the credentials provider got no region. It now receives the profile's
  region (or the `:region()` override).
- Further per-module behavioural fixes are described in the chapters under
  `docs/src/modules/`.

## [0.6.0] - 2026-09-09

The single `ito` binary was under the influence of different requirements
and has been split into separate binaries with well-matching command line interfaces.

- `iq` ito-query - used for read-only operations
- `ied` ito-edit - used for read-write (this is roughly was `ito` v0.5.0 was)
- `ito` ito - ito for automation (real filesystem access, spawn processes, network, time)
- `irun` ito-run - a shebang helper that allows passing arguments to scripts easily

The scripting language is now `luau`.

New scripting modules/globals:

- `cli` - declarative flag/option parsing with usage and help
- `hash` - hashing of strings, bytes and files
- `redirect` - stream redirect targets for `proc`/`ssh` commands
- `ssh` - remote command execution over SSH
- `tablefmt` - render ASCII/Unicode tables
- `task` - concurrency: spawn, pool, timeout, wait
- `time` - timestamps, durations, timezones, calendar math
- `aws.ec2` - inspect and manage EC2 instances, images, snapshots
- `aws.s3` - read, write, tag S3 objects and buckets
- `aws.autoscaling` - inspect and manage Auto Scaling groups
- `aws.config` - shared AWS credentials/region configuration
- `ui.prompt` - interactive terminal prompts (confirm, select, input)
- `ui.progress` - terminal progress bars, including multi-bar display

Also substantially reworked: `fs` (host filesystem, finder, grep), `path`,
`proc` (async child processes), `io`.


## [0.5.0] - 2026-06-29

### Added

- **ito:** Script flags — pass `key=value` and `+flag` on the command line;
  read them with `ito::opt`, `ito::try_opt`, and `ito::flag`
- First-class invocation of well-known script names — `ito <name>`
  - looks for one of
    - `<name>.ito.rhai`
    - `.<name>.ito.rhai`
    - `ito/<name>.rhai`
    - `.ito/<name>.rhai`

### Deprecated

- `-S`/`-N`/`-B` (`--string`/`--numeric`/`--bool`) injected globals — use `key=value` and `+flag` with `ito::opt`/`ito::flag` instead

### Changed

- **Breaking:** `run` now requires an explicit path argument
  - to get the old default run `ito main`

### Fixed

- Writing an empty string to a new file now correctly reports as a change
- **yamlito:** Block scalar decoding fixes (literal and folded styles)
  - test cases taken from [yaml-test-suite](https://github.com/yaml/yaml-test-suite)
  - explicit indent indicator (`|2`, `>1`) now relative to parent block indent per spec
  - folded scalar line-folding and blank-line handling corrected
  - double-quoted scalar trailing whitespace trimmed correctly before line folds

## [0.4.0] - 2026-06-11

### Added

- **yaml:** Structural mapping edits; rename seq edits to append/insert_at/remove_at

## [0.3.4] - 2026-06-10

### Fixed

- **yamlito:** Reindent block-seq item continuations at green level
- **yamlito:** Fixes and refactorings

## [0.3.2] - 2026-06-10

### Added

- **yaml:** Add yaml::edit for format-preserving in-place edits
- **yamlito:** Add sequence cursor insert/push/remove edit API
- **yamlito:** Continue multiline flow sequence layout on insert/remove
- **yamlito:** Return live node from sequence item edits
- **yaml:** Add sequence push/insert/remove to yaml::edit cursor

### Documentation

- **yaml:** Document yaml::edit .doc entry point and scalar quote-style API

## [0.3.0] - 2026-06-08

### Added

- **scripting:** Prefix builder methods with with_*
- **fs:** Exclude hidden files by default in glob and finder
- **scripting:** Make_cleared returns nothing
- **scripting:** Make_* string methods return nothing instead of the result
- **scripting:** Set_* string methods accept char or string

### Fixed

- **docs:** Make SUMMARY.md mdbook-parseable

## [0.2.0] - 2026-06-02

### Added

- **string:** Add trim_start, trim_end, and array join methods
- **string:** Replace built-in overrides with to_*/make_* variants

### Documentation

- Split strings-arrays-maps into string/array/map topics, add ito-vs-rhai

### Fixed

- **string:** Join takes array by value, not mut ref

## [0.1.2] - 2026-06-02

### Added

- Add cargo-deb packaging for linux x86_64 and aarch64

### Fixed

- **deb:** Fix tarball glob and deb arch copy path
- **deb:** Install aarch64 cross libs and skip strip for cross-arch deb
- **deb:** Copy deb from target/<triple>/debian/ not target/debian/
- **deb:** Use --output to write deb directly to target path
- **deb:** Write debs to target/deb/ to avoid self-copy by dist

## [0.1.1] - 2026-06-01

### Added

- **re:** Add regular expression module

## [0.1.0] - 2026-06-01

### Added

- Initial release of ito

[0.7.0]: https://github.com/denfren/ito/compare/v0.6.0...v0.7.0
[0.6.0]: https://github.com/denfren/ito/compare/v0.5.0...v0.6.0
[0.5.0]: https://github.com/denfren/ito/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/denfren/ito/compare/v0.3.4...v0.4.0
[0.3.4]: https://github.com/denfren/ito/compare/v0.3.2...v0.3.4
[0.3.2]: https://github.com/denfren/ito/compare/v0.3.0...v0.3.2
[0.3.0]: https://github.com/denfren/ito/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/denfren/ito/compare/v0.1.2...v0.2.0
[0.1.2]: https://github.com/denfren/ito/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/denfren/ito/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/denfren/ito/releases/tag/v0.1.0

