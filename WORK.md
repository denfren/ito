# WORK — scripting surface review backlog

Source: a read-only review, one agent per module (2026-09-30), plus the
regrouping proposal. Findings come from reading the code and were not run;
`(?)` marks a claim still to check. Breaking changes are OK.

Item IDs are stable (`<area>-<n>`), so commits and discussion can refer to
them. `[ ]` means open and `[x]` means done.

---

## 0. Decisions

| # | Topic | Decision |
|---|---|---|
| D1 | Empty the `ito` grab-bag | **Do.** See §1 (REG-1..5). |
| D2 | Codec interface | **Only** reconcile null, and replace `to_string_pretty` with `to_string(v, {pretty=true})`. No other codec unification yet. |
| D3 | Unify `yaml.edit`/`hcl.edit` vocabulary | **Skip for now.** |
| D4 | `time` delta helpers | Remove the `truncate_*`/`count_*` duplicates. Leave the delta surface otherwise as is (no `time.delta.*` namespace). |
| D5 | ansible naming | **Do** (`ansible.vault.encrypt`/`decrypt`/`load_password_file`). |
| D6 | `tablefmt` → `ui.table` | **No.** `ui.table` stays free for something else. Instead consider a formatting module grouping tables, colours/styles, symbol/emoji helpers and ANSI stripping (§1, REG-6). |
| D7 | `encoding` module | **Add**, base64 only for now: a default function pair plus a builder mirroring the Rust `base64` crate (engines/alphabets/padding). |
| D8 | Breakage | OK. |

### Resolved

- **Q1** → `ito.colors`/`ito.is_tty` move to `fmt` (is_tty stays host-only).
- **Q2** → both: the `string` methods stay as sugar; `fmt` owns the implementation and adds `fmt.strip(s)`/`fmt.width(s)`.
- **Q3** → a global **`null`** (the light-userdata sentinel, same value as today). Drop `ito.null`, `json.null` **and `ito.is_null`**: `x == null` replaces the latter, with no `is_null` global. Every codec produces/accepts it, and so does `yaml.edit` `:read()`/`:write()`. Registered before `sandbox(true)`, in all three binaries.
- **Q4** → leave key order alone for now (D2 scope).
- **Q5** → the module is named **`fmt`**.

### Item-level decisions

| Item | Decision |
|---|---|
| X-1 / TIME-3 | A day is a **fixed 24h** everywhere, `add_tz`/`add_local` included: `"1d"` == `86400` == `time.days(1)`. Only months/years are calendar units. |
| TIME-2 | `add_*`/`sub_*` accept **`parse_delta`'s grammar plus months/years** only. Drop the jiff Span spellings (`PT1H`, `-1h`, `2h ago`). |
| ITO-1 | **Keep** `version_check`'s semver-compatibility (caret) semantics. Pre-1.0 we follow the Rust convention: a change in `x` of `0.x.y` is incompatible. Fix only the stale examples (use the current version), and state the 0.x rule in the doc. |
| REDIR-2 / REDIR-14 | Give `proc.STDOUT`/`proc.STDERR` distinct values, and raise on a self-alias (`:stdout(proc.STDOUT)`). **No rename.** |
| IO-2 | ~~Under `-q`, `print` logs at **debug** (visible with `-vv`).~~ **Superseded** by the `iq`-removal redesign: `print` is vanilla and never goes to the log; `ito` has no `-q`; `ied -q` (implied by `-d`/`-l`) makes `print` a silent no-op. |
| FS-7 | The rooted VFS **rejects a leading `/`**, so a path means the same file under ied and ito. |
| FS-13 | Rename `fs.rmdir` → **`fs.remove_all`**. |
| ANS-7 | `ansible.vault.encrypt` becomes **host-only** (it's random); `decrypt` stays everywhere. |
| SSH-12 | **Strict host-key checking by default**; trust-on-first-use is opt-in (e.g. `ssh.connect(host, {tofu = true})`). |
| S3-10 | `get_object` **always returns a string**; parsing is on request (`:parse("json")` or `json.parse`). |

---

## 1. Regrouping / redistribution

- [ ] **REG-1** `ito.pid` → `proc.pid`.
- [ ] **REG-2** `ito.is_tty`, `ito.colors` → `fmt.is_tty`, `fmt.colors`. `colors` must stay available in all three binaries, and `is_tty` stays host-only.
- [ ] **REG-3** `ito.require_root`, `ito.require_dir` → `fs` (next to `fs.cwd`), host-only. Consider adding `fs.script()` (the entry script's own path; ITO-13).
- [ ] **REG-4** Null: add a global `null`; remove `ito.null`, `json.null` and `ito.is_null`; make yaml/hcl/json use it consistently. `yaml.edit` `:read()` must return the sentinel, not `nil` (YAML-2). Drop `toml` from the doc lists that claim a toml null (ITO-8, TOML-8).
- [ ] **REG-5** `ito` then keeps `version_check` and `slug`.
- [ ] **REG-6** New `fmt` module: absorbs `tablefmt`, ANSI styles/colours, an `fmt.strip`, `fmt.width`, `fmt.colors`/`fmt.is_tty`, (the string methods remain as sugar), and symbol/emoji helpers (e.g. ✔/✘ with ASCII fallbacks tied to the colour switch, the `if_color`/`if_nocolor` idea). Must be **pure** in ied: the width and colour auto-detection parts are host-only (see TFMT-2, TFMT-1).
- [ ] **REG-7** `json.to_string_pretty`, `toml.to_string_pretty` → `to_string(v, {pretty = true})`. Consider an `indent` option (JSON-10).
- [ ] **REG-8** `time`: remove `truncate_minutes/_hours/_days` and `count_minutes/_hours/_days`; keep `truncate(s, unit)`/`count(s, unit)`.
- [ ] **REG-9** `ansible.vault.encrypt_vault`/`decrypt_vault` → `ansible.vault.encrypt`/`decrypt`; `ansible.load_vault_password_file` → `ansible.vault.load_password_file`.
- [ ] **REG-10** New `encoding` module, base64 only: `encoding.base64.encode(s|buffer)`/`decode(s)` defaults (STANDARD, padded), plus a builder mirroring the `base64` crate (`encoding.base64.engine():alphabet("url_safe"|"standard"|"crypt"|"bcrypt"|"imap_mutf7"|"bin_hex"):padding(true|false):decode_padding("indifferent"|"required"|"none")` with `:encode`/`:decode`). Accepts string and buffer. Serves HASH-3, the S3 checksums and SRI.
- [ ] **REG-11** Hex encoding: deferred by D7, but hash/ansible/ito each hand-roll it internally; revisit.
- [ ] **REG-12** `hash`: optional reshape (4 algorithms × 3 forms). Not decided.

---

## 2. P0: data loss, security, crashes, hangs

- [ ] **P0-1** `aws.ec2.delete_tags(ids):tags({})` **deletes every tag**: the SDK skips the empty list, and EC2 reads that as "all". ec2.rs:1363-1372. Raise, as autoscaling does. (EC2-1)
- [ ] **P0-2** ssh: the program name is passed unescaped to the remote shell (`arc_raw_command`), so injection is possible. (SSH-3)
- [ ] **P0-3** require: the root's sibling `<root>.lua` is loadable via `./.`; symlinks escape the root. (REQ-1, REQ-2)
- [ ] **P0-4** ansible: a relative executable password file is looked up on `$PATH`. (ANS-3)
- [ ] **P0-5** hcl: `Ident::new` panics on a non-identifier object key. (HCL-1)
- [ ] **P0-6** hcl: keys are silently sanitized on write (`a.b` → `a_b`), and remove can hit the wrong key. (HCL-2)
- [ ] **P0-7** hcl: `remove(key)` removes every labelled block with that ident. (HCL-5)
- [ ] **P0-8** aws.config: env vars beat `:profile()`, so a script can act in the wrong account. (AWSC-2)
- [ ] **P0-9** task: an unawaited failure that settles before the end is lost, so the run exits 0. (TASK-1)
- [ ] **P0-10** task: `pool:cancel()` hangs a concurrent `pool:await()`. (TASK-2)
- [ ] **P0-11** ui.progress: `suspend(fn)` deadlocks if `fn` touches a bar. (PROG-1)
- [ ] **P0-12** ui.prompt: Ctrl-C kills the process and leaves the cursor hidden or echo off. (PROM-1, PROM-2)
- [ ] **P0-13** ied: a remove of an absent file shows as "create"; a write to a directory breaks every run. (FS-1, FS-2)
- [ ] **P0-14** j2: included `*.html`/`*.xml` are HTML-escaped automatically. (J2-4)
- [ ] **P0-15** time: `_local` uses the current offset, not the zone, so it is wrong across DST. (TIME-1)
- [ ] **P0-16** ssh: exit 127/255 raises and loses the output. (SSH-1)
- [ ] **P0-17** Panics from script input: `task.wait(1e20)`, `task.pool(huge)`, progress `tick_strings({"x"})`, `enable_steady_tick(-1)`. (TASK-5, PROG-9)

---

## 3. Cross-cutting

- [ ] **X-1** "One delta parser" is false: `task.rs` has its own humantime parser, and `time.add_*`/`sub_*` use the jiff `Span` grammar. In `add_tz`, `"1d"` ≠ `86400`. Route everything through `parse_delta_value`. (TIME-2, TIME-3, TIME-4, TASK-4)
- [ ] **X-2** Bool arguments coerced by truthiness (mlua `bool: FromLua`): cloudtrail `:read_only`, ui.prompt bools, `string:red(nil)`, `cli.assert`. Take `Value` and require `Boolean` or nil.
- [ ] **X-3** Empty tables encode as `{}`, never `[]` (json, toml, yaml, s3 listings). Expose one shared array marker. (JSON-1, TOML-7, S3-11)
- [ ] **X-4** Non-UTF-8 rejected across the board: `fs.read`, `re`, `string`, `proc` args/env, `hash` paths. Luau strings are bytes. (FS-12, HASH-1, RE-7, STR-6, PROC-14)
- [ ] **X-5** Generic mlua conversion errors with no `module.fn:` prefix, and a wrong userdata gives "userdata is not expected type". Almost every module has this. Add a shared helper.
- [ ] **X-6** Output plumbing: `-v` is a no-op; `-q` doesn't silence `print` or `io.*`; `print`/`log` garble progress bars; tracing colours ignore tty and `ito.colors`. (IO-1..7, PROG-3)
- [ ] **X-7** No stdin anywhere: no `proc:stdin()`, no ssh stdin/`:interactive()`, no `io.read`/`io.lines`. (PROC-2, SSH-7, IO-8, REDIR-8)
- [ ] **X-8** AWS shared:
  - an empty id/name list widens the query to the whole account;
  - time args are numeric only, and the `time_arg` helper is duplicated;
  - error prefixes are inconsistent;
  - tag parsing differs between ec2 and autoscaling;
  - the `mod.rs` docs list only ec2/s3.
  
  (ASG-3, CW-3, CT-6, CT-12, EC2-8, ASG-8, ASG-11)
- [ ] **X-9** Documented examples that fail: `version_check("0.4.0")`; cloudwatch `time.ago(time.hours(3))` and `:maximum():percentile()`; aws-config `:send()` binding; progress `pool:await_all()`; string `if_nocolor` reversed; proc `2>&1`; ansible `~/.vault_pass`; hash `fs.read` fallback.
- [ ] **X-10** CLAUDE.md is stale: the globals list lacks `hash`/`ansible`/`tablefmt`; the "deltas are one surface" claim is false (X-1); the `run.rs:344` comment says ssh has `:env()`. The ied chapters don't list `hash`/`ansible`.
- [ ] **X-11** Recurring doc nits: "Lua" where it should be "Luau" (ansible.md:65, tablefmt.md:52, tablefmt.rs:34); placeholder convention; chapters not in the required per-function shape (io, log, ui-progress, ui-prompt, toml).

---

## 4. Per module

### fs (vfs.rs, rfs.rs, finder.rs, pathmap.rs)
- [ ] FS-1 `ied --diff`: a remove of an absent file shows as creating an empty file; removing an existing empty file is skipped by the diff but counted by `-l`/`-e`. Handle `Action::Remove` explicitly.
- [ ] FS-2 A write or remove aimed at a directory is accepted at queue time, then `changed()` fails and every ied run exits 2. `flush` is not atomic. Check the target kind at queue time; validate the plan before the first write.
- [ ] FS-3 The rooted `fs.finder(dir)` ignores `dir` (the closure takes `()`).
- [ ] FS-4 Host `fs.glob` always walks the cwd: absolute and `../` patterns return `{}`, and it ignores `.gitignore` and follows outward links. Start from the pattern's literal prefix.
- [ ] FS-5 Glob metacharacters in the root/cwd path break `fs.glob`. Use `globset::escape` or match relative paths.
- [ ] FS-6 Host finder: `:exclude` and `:path` match different strings (with vs without the start prefix).
- [ ] FS-7 A leading `/`: root-relative in the VFS, absolute on the host. **Decided:** the VFS rejects it.
- [ ] FS-8 The rooted finder descends into outward symlink targets before dropping them (`link -> /` walks the whole disk). Prune in `filter_entry`.
- [ ] FS-9 Host `walk`/`glob`/`copy` always follow symlinks; `finder` doesn't unless `:follow()`.
- [ ] FS-10 `fs.walk`/finder return empty for a missing start directory, while `fs.list` raises.
- [ ] FS-11 Rooted `fs.list` returns out-of-root symlinks that everything else refuses.
- [ ] FS-12 Missing: `fs.stat`, `chmod`, `append`, binary-safe read/write (`read_bytes`), temp file/dir, `rename` EXDEV fallback; `fs.list(dir)` requires its argument while `walk`/`finder` default it.
- [ ] FS-13 `fs.rmdir` is actually `rm -rf`: **Decided:** rename it to `fs.remove_all`. `fs.remove` on a directory gives a raw OS error. `fs.copy(p, p)` may truncate (?).
- [ ] FS-14 Rooted errors leak the absolute host root; host errors mix spellings. Use the script's own spelling.
- [ ] FS-15 Doc/code mismatches:
  - grep results are path-sorted, not in walk order;
  - `:limit` does not apply to `:any()`;
  - the `fs.read` error paragraph is garbled;
  - a `:first()` terminal is mentioned but doesn't exist;
  - the first path is `""` in the VFS vs `"."` on the host;
  - symlink aliases get separate ied write slots.

### proc (proc.rs)
- [ ] PROC-1 The doc's `2>&1` example captures nothing (eager alias). Swap the order or use an explicit shared handle.
- [ ] PROC-2 No `:stdin()`. Accept NULL/INHERIT/a file/a string or buffer.
- [ ] PROC-3 `:interactive()` can't capture stdout (the fzf/gum case).
- [ ] PROC-4 Ctrl-C during `:interactive()` kills ito. Ignore SIGINT/SIGQUIT while the child runs.
- [ ] PROC-5 `proc.trap` reports success for untrappable signals (ILL/FPE/SEGV/999); the registration failure is only logged. Register synchronously.
- [ ] PROC-6 `:exec()` failure leaves cwd/env mutated. It uses the parent's PATH, not `:env("PATH")`. `last_err` masks EACCES.
- [ ] PROC-7 No `Child:try_wait()`, so the grace-period pattern always sleeps the full time.
- [ ] PROC-8 An abandoned `:run()` (under `task.timeout`) orphans the child. Set `kill_on_drop`.
- [ ] PROC-9 The watchdog example `kill -9 pid` is open to pid reuse. Use `child:signal`.
- [ ] PROC-10 `Child:kill()` docs overstate the result when the child has already exited.
- [ ] PROC-11 `proc.STDOUT == proc.STDERR` (see REDIR-2).
- [ ] PROC-12 `proc.file` handle semantics are undocumented (shared offset across commands); no append mode; the same-file check compares strings (see REDIR-5..7).
- [ ] PROC-13 Missing `:envs(table)` and `:env_remove(name)`.
- [ ] PROC-14 Args/env/cwd must be UTF-8 (X-4).
- [ ] PROC-15 Minor:
  - `Trap::count` is never exposed;
  - the `signal_number` i64→i32 cast doesn't check range;
  - the Conventions section omits `:exec()`/`:interactive()`;
  - `:exec()`/`:interactive()` reject an explicit `INHERIT`;
  - `:wait()` hangs when a grandchild holds the pipe (doc note);
  - `SIGWINCH`/`SIGCHLD`/`SIGTTIN`/`SIGTTOU` are missing.

### redirect (redirect.rs)
- [ ] REDIR-1 `bind_alias` of `Inherit(Same)` goes to the wrong stream (`:stdout(INHERIT):stderr(STDOUT)` → our stderr). Resolve to a concrete target.
- [ ] REDIR-2 `proc.STDOUT`/`proc.STDERR` are the same value. **Decided:** distinct variants plus a raise on self-alias; no rename.
- [ ] REDIR-3 `:exec()`/`:interactive()` check "not NULL", but NULL is the default: an explicit NULL is silently ignored and an explicit INHERIT is rejected. Use `Option<Redirect>`.
- [ ] REDIR-4 ssh merged capture is concatenated, not interleaved; the doc claims parity. Document it, or pass one pipe fd for both streams.
- [ ] REDIR-5 The same-file check compares path strings. Compare dev/ino.
- [ ] REDIR-6 A reused `proc.file()` handle appends across commands and keeps the fd until GC. Document it or reopen per spawn.
- [ ] REDIR-7 No append mode (`proc.file(p, {append=true})`).
- [ ] REDIR-8 No stdin redirect (X-7).
- [ ] REDIR-9 ssh `:spawn()` doesn't drain until `:wait()`, so the remote side stalls on a full pipe.
- [ ] REDIR-10 A wrong userdata gives a vague error.
- [ ] REDIR-11 A relative `proc.file()` ignores `:current_dir()`. Document or resolve.
- [ ] REDIR-12 The stdout file is truncated before the spawn can fail. Open all targets first.
- [ ] REDIR-13 Doc gaps: the alias also sets `r.stderr`; the result-shape rule needs its own section.
- [ ] REDIR-14 `OUT`/`ERR` vs `STDOUT`/`STDERR` naming is confusing. **Decided:** no rename (REDIR-2 covers it).
- [ ] REDIR-15 Check whether our buffered stdout is flushed before spawn/exec with INHERIT (?).

### ssh (ssh.rs)
- [ ] SSH-1 Exit 127/255 raises and loses the captured output; the doc says non-zero is not an error.
- [ ] SSH-2 openssh errors lose their `source()` chain ("failed to connect" with no reason).
- [ ] SSH-3 The program name is unescaped in the remote shell; the doc claims "no shell". Use `arc_command` and rewrite the doc.
- [ ] SSH-4 An abandoned `:wait()` bricks the handle. Copy proc's `WaitGuard` pattern.
- [ ] SSH-5 Merged capture is concatenated (REDIR-4).
- [ ] SSH-6 No `:close()`; the session Drop runs a blocking `ssh -O exit` whenever GC happens.
- [ ] SSH-7 No stdin, no `:interactive()` (`-t`).
- [ ] SSH-8 Missing-method stubs are inconsistent (`env_clear`/`exec`/`interactive`/`signal`/`id` are simply nil), and the typed stub signatures produce conversion errors.
- [ ] SSH-9 The `:kill()` "use a pty" advice can't be followed; `disconnect()` isn't exposed.
- [ ] SSH-10 The redirect list in the doc omits `proc.OUT`/`ERR`.
- [ ] SSH-11 Errors surface at `:wait()`, not at `:spawn()`; the docs are wrong.
- [ ] SSH-12 `KnownHosts::Add` overrides the user's `StrictHostKeyChecking yes`. **Decided:** Strict by default, with a TOFU opt-in.
- [ ] SSH-13 The `run.rs:344` comment claims ssh has `:env()`.
- [ ] SSH-14 No tests at all.
- [ ] SSH-15 Undocumented: the `user@host` / `ssh://` forms, that `:spawn()` yields, and connect abandoned mid-handshake leaking the master (?).

### task (task.rs)
- [ ] TASK-1 The drain misses already-failed unawaited tasks. Sweep for `Failed{reported:false}`; add a test that yields.
- [ ] TASK-2 `pool:cancel()` doesn't wake `pool.waiters`, so `await` hangs. `Ready` must also finish when outstanding == 0.
- [ ] TASK-3 `pool:cancel()` doesn't claim queued failures (hidden today by TASK-1); the count semantics are unclear.
- [ ] TASK-4 Durations bypass `parse_delta` (X-1).
- [ ] TASK-5 `Duration::from_secs_f64` panics on overflow; `Semaphore::new` panics above `MAX_PERMITS`.
- [ ] TASK-6 The reentrancy guard can be bypassed through a nested `task.spawn`/`task.timeout` inside a pooled body, which deadlocks.
- [ ] TASK-7 Cancellation is only a string error. Add a `task.CANCELLED` sentinel or `task.status(h)`.
- [ ] TASK-8 A wrong userdata gives a generic error.
- [ ] TASK-9 `task.pool(2.5)` is probably truncated to 2 (?). Validate.
- [ ] TASK-10 Batch errors don't say which handle failed; the drain's `task#N` is meaningless for pooled tasks.
- [ ] TASK-11 Handle lists stop at the first nil; `task.await({})` returns nothing silently.
- [ ] TASK-12 The shape of `task.await`'s result depends on the number of handles (doc note).
- [ ] TASK-13 The registry never releases tasks; drain/collect are O(n²).
- [ ] TASK-14 A pool has no `__tostring`; its concurrency can't be read; there's no first-of for plain handles.
- [ ] TASK-15 The `pool:cancel` counting wording.

### time (time.rs)
- [ ] TIME-1 `_local` uses a fixed current offset. Use `TimeZone::system()`.
- [ ] TIME-2 `add_*`/`sub_*` parse with the jiff Span grammar (`-1h`, `PT1H`, `2h ago` (?), no `1M`). **Decided:** only `parse_delta` grammar plus months/years; drop the Span spellings.
- [ ] TIME-3 In a zone, `"1d"` is a calendar day but `86400` is 24h. **Decided:** a fixed 24h.
- [ ] TIME-4 `task.wait` parser (X-1).
- [ ] TIME-5 `parse_fmt_*` lets `%z` in the text override the zone argument, contrary to its comment. Reject it, or add a zone-less `parse_fmt`.
- [ ] TIME-6 `format_rfc3339_*` truncates sub-seconds. Use `%.f`.
- [ ] TIME-7 `format_fmt_*` passes a pattern with no `%` through unchanged; `parse_fmt_*` has no pattern check at all.
- [ ] TIME-8 No `format_civil_{utc,tz,local}` (the inverse of `parse_civil_*`).
- [ ] TIME-9 The docs say rfc3339 has "no suffix", but only `format_rfc3339_*` exists. Alias `format_rfc3339` to `_utc`?
- [ ] TIME-10 Stale module doc: "signed" delta, `P1D` spellings, `until`/`diff` mentions, the "1mo-free" wording, `os.time()` "fractional".
- [ ] TIME-11 Delta error messages: `"1.5"` gets a hint that fails; `"-90s"` gets a generic error; the month/year paragraph is always appended.
- [ ] TIME-12 Some errors lack the `time.` prefix.
- [ ] TIME-13 `minutes`/`hours`/`days` don't check finiteness; `hours("2h")` gives a generic error.
- [ ] TIME-14 `secs_to_timestamp` nanosecond rounding overflows to 1e9, so formatting raises.
- [ ] TIME-15 A string `t` input accepts RFC 3339 only; document it or fall back to rfc2822.
- [ ] TIME-16 (REG-8) Remove the `truncate_*`/`count_*` duplicates.

### cli / argv (cli.rs, argv.rs)
- [ ] CLI-1 A value option swallows the next flag (`--env --help`). Refuse a following token that looks like a flag.
- [ ] CLI-2 Negative numbers can't be positionals. Add an allow-negative-numbers rule.
- [ ] CLI-3 `-e=` vs `--env=` and `-f=` vs `--file=` behave differently.
- [ ] CLI-4 A repeated single-value option is silently last-wins and `:count()` undercounts. Make it a usage error or count occurrences.
- [ ] CLI-5 `pcall` catches the CliExit sentinel; a second `cli.parse` then misbehaves. Mark the registry as terminated.
- [ ] CLI-6 `argv` is probably read-only after the sandbox (?), and `cli.parse` can't take a list, so there are no subcommands. Add `cli.parse([list])`.
- [ ] CLI-7 Missing: defaults, required options, choices, value names, env fallback, stop at the first positional. Accept an options table; rename "required value" to "takes a value".
- [ ] CLI-8 Doc/code: the usage error prints the summary (the doc says it's omitted).
- [ ] CLI-9 The `cli.about` last-wins rationale is backwards. Only honour the entry chunk?
- [ ] CLI-10 Confusing "declare inside main" advice for required modules.
- [ ] CLI-11 Help quality: the heading reads `Arguments:` (should be `Usage:`); every value is `<value>`; descriptions aren't wrapped.
- [ ] CLI-12 An unknown option beats `--help`.
- [ ] CLI-13 No "did you mean" suggestion.
- [ ] CLI-14 Inconsistent quoting in error messages; no `--help` hint.
- [ ] CLI-15 Doc nits: the `condition` placeholder; `cli.assert` truthiness; the "do not return a value / return a handle" contradiction.

### yaml (yaml/*)
- [ ] YAML-1 `:write()` on an implicit null (`key:`) raises; update/upsert too. Replace the `NULL_SCALAR` node.
- [ ] YAML-2 `:read()` returns `nil`, not `ito.null` (parse and edit disagree); `:write(ito.null)` raises. (REG-4)
- [ ] YAML-3 `write`/`update`/`upsert` are scalar-only; they can't replace a mapping/list. Splice a subtree.
- [ ] YAML-4 `rename_key` can create duplicates; `key_node():write` is unchecked.
- [ ] YAML-5 `write` formats floats with Rust's `to_string` (`inf`, `NaN`). Reuse the builder's formatter.
- [ ] YAML-6 `write` discards the scalar's existing quote/block style.
- [ ] YAML-7 `remove()` out of range returns true; `remove_at` out of range is a silent no-op.
- [ ] YAML-8 No arity checks on `add`/`add_at`.
- [ ] YAML-9 `update_all` visits sequences last-to-first.
- [ ] YAML-10 Empty documents: `len`, `docs`, `doc()` and `stream[i]` disagree; `yaml.edit("")` can't create a first document.
- [ ] YAML-11 The `quote_style` docs are wrong (`"none"` means an implicit null, not an alias).
- [ ] YAML-12 The documented error messages don't match `render_path`.
- [ ] YAML-13 `upsert`'s delete-on-nil is undocumented.
- [ ] YAML-14 An integer argument means a position even on mappings with numeric keys; improve the error.
- [ ] YAML-15 Minor:
  - `try_*` coverage is inconsistent;
  - there is no `:values()` on sequences;
  - `set_trailing_comment` on a document root is undocumented;
  - the doc names internal files;
  - the `parse_multi` alias rejection is undocumented.

### hcl (hcl/*)
- [ ] HCL-1 `Ident::new` panics on non-identifier keys; quoted keys are never found. Add a shared `find_object_key`.
- [ ] HCL-2 `new_sanitized` rewrites keys on write; lookups use the rewritten key. Emit quoted keys instead.
- [ ] HCL-3 `h:blocks()` with no arguments returns only unlabelled blocks. Use `any_labels()`.
- [ ] HCL-4 A `write(builder)` block replace drops the leading comments and blank lines; the builder's own `with_comment` is lost.
- [ ] HCL-5 `remove`/`remove(key)` delete labelled blocks too; an ambiguous key silently removes the attribute.
- [ ] HCL-6 `update(index, fn)` on a body raises; the doc says positional.
- [ ] HCL-7 Error messages: hardcoded segment names, 0- vs 1-based indices, `NotFound` drops the path, internal jargon.
- [ ] HCL-8 `try_update`/`try_upsert` swallow callback errors; `update_all` is not atomic.
- [ ] HCL-9 `update_all`/`update` fail on any computed value. Skip them or pass `HclExpr`.
- [ ] HCL-10 `attributes()`/`attribute_keys()` reject objects; the doc example fails.
- [ ] HCL-11 Gaps vs yaml: comment editing, `rename_key`/block-label rename, `contains`, `read()` of a block. (D3: vocabulary unification skipped.)
- [ ] HCL-12 `blocks()` handles shift when entries are removed; a literal `*` in a label acts as a wildcard.
- [ ] HCL-13 `add(block)` adds no blank line before the block.
- [ ] HCL-14 The builder:
  - `with_label` uses Rust `{:?}` escaping, so it doesn't escape `${`;
  - attribute validation timing contradicts the doc;
  - duplicate keys are not rejected;
  - `HclBlock` has no `to_string`.
- [ ] HCL-15 Minor:
  - `add_at` clamping is inconsistent;
  - method names shadow `h["len"]`;
  - a stale `h[0]` comment;
  - snapshot metadata still shows the old API;
  - test gaps.

### json (json.rs)
- [ ] JSON-1 No way to write an empty array (X-3).
- [ ] JSON-2 Output keys are sorted (BTreeMap), so a parse→to_string round trip reorders them. (deferred, Q4)
- [ ] JSON-3 Serialize errors come from `from_value` ("deserialize error"), with no prefix and no key path; the doc's prefix is wrong.
- [ ] JSON-4 Mixed tables silently lose their keyed part. Enable `detect_mixed_tables` or raise.
- [ ] JSON-5 Integer-keyed/sparse tables give a confusing error or unstable output.
- [ ] JSON-6 NaN/inf silently become `null`. Raise.
- [ ] JSON-7 Integers above 2^53 lose precision (doc).
- [ ] JSON-8 An invalid-UTF-8 string encodes as a byte array. Raise.
- [ ] JSON-9 Pretty output has no trailing newline.
- [ ] JSON-10 No JSON Lines; no indent option (REG-7).
- [ ] JSON-11 Stale doc comment: "parse produces nil".
- [ ] JSON-12 "Value mapping" omits the lossy cases.
- [ ] JSON-13 Thin tests.
- [ ] JSON-14 Parse errors don't name the file. Add an optional `name` argument (json/toml/yaml).

### toml (toml.rs)
- [ ] TOML-1 Datetimes come back as `{["$__toml_private_datetime"]=…}` tables, not strings. Walk the value manually; decide on the write direction (`toml.datetime(s)`?).
- [ ] TOML-2 No format-preserving `toml.edit` (`toml_edit`), so Cargo.toml edits drop comments.
- [ ] TOML-3 Key order is the Luau hash order. (deferred, Q4)
- [ ] TOML-4 Serialize error prefix/path (as JSON-3).
- [ ] TOML-5 Integral floats come back as integers (`1.0` → `1`).
- [ ] TOML-6 Integers above 2^53 lose precision.
- [ ] TOML-7 Empty array vs table (X-3).
- [ ] TOML-8 `ito.md` lists toml as a null source (REG-4).
- [ ] TOML-9 The doc lacks the value-mapping section.
- [ ] TOML-10 "toml parse error: TOML parse error" is doubled.
- [ ] TOML-11 Mixed/sparse tables give a confusing error.
- [ ] TOML-12 `parse` returns `-> table`, not `any`.
- [ ] TOML-13 No tests.
- [ ] TOML-14 Check the `to_string_pretty` example against toml 1.1's output (becomes REG-7).

### j2 (j2.rs)
- [ ] J2-1 No tests at all (include containment, the no-fs raise, strict undefined, custom syntax, missing include).
- [ ] J2-2 `tojson` is missing (the minijinja `json` feature); `urlencode` too. Fix together with J2-4.
- [ ] J2-3 No Jinja2 method calls (`d.items()`, `s.upper()`). Use minijinja-contrib pycompat.
- [ ] J2-4 Includes are HTML-autoescaped by name. Set `AutoEscape::None`, and optionally add `:with_autoescape`.
- [ ] J2-5 The `with_syntax` error surfaces at `:template()`, not at the setter as documented. Validate in the setter.
- [ ] J2-6 No render-from-file (`engine:render(name, data)`).
- [ ] J2-7 No `with_filter`/`with_test`/`with_global`; the Ansible filters (to_yaml, b64encode, regex_replace, basename, combine, mandatory, …) are missing. Clone the options before callbacks (RefCell).
- [ ] J2-8 Undefined is lenient by default and can't be changed on `j2.template`; `semi_strict` is missing.
- [ ] J2-9 `{% break %}`/`{% continue %}` are off (the `loop_controls` feature).
- [ ] J2-10 Include containment differs between Rooted and Host.
- [ ] J2-11 The Host canonicalize check fails open on any error. Only NotFound should fall through.
- [ ] J2-12 The doc is wrong about missing includes (they raise unless `ignore missing`).
- [ ] J2-13 `data` is not validated: a non-table is accepted; function values give an unprefixed error; an empty table becomes a mapping.
- [ ] J2-14 Error messages drop the source context and the cause chain.
- [ ] J2-15 A new Environment is built per call. Cache it per engine.

### re (re.rs)
- [ ] RE-1 Capture keys are strings (`m["1"]`), so `m[1]`, `ipairs` and `unpack` all fail. Use integer keys.
- [ ] RE-2 No `re.escape`.
- [ ] RE-3 No compiled pattern, so every call recompiles. Add `re.compile(p)` (or an LRU cache).
- [ ] RE-4 No match positions. `find` → `s, start, end` (1-based, like `string.find`).
- [ ] RE-5 No function replacement and no `n` limit on replace/split.
- [ ] RE-6 An unknown group reference in `rep` expands to `""` silently.
- [ ] RE-7 Non-UTF-8 text gives an opaque error; numbers are coerced to strings.
- [ ] RE-8 The invalid-pattern error doesn't name the function or pattern.
- [ ] RE-9 The capture shape differs from `fs.finder():grep()` (group 0). Share one helper.
- [ ] RE-10 The doc says "superset of POSIX"; it is really RE2/Perl-style.
- [ ] RE-11 Document the inline flags and that `\d` is Unicode-aware.
- [ ] RE-12 The examples teach double escaping. Use `[[…]]`.
- [ ] RE-13 `split` edge cases are undocumented and untested.
- [ ] RE-14 No `re.count` and no anchored `re.match`.
- [ ] RE-15 Doc noise: "(but still a new string)".

### path (path.rs)
- [ ] PATH-1 The batch `relative_to`/`capture` results have `nil` holes; the doc says "same length". Use a sentinel or document it.
- [ ] PATH-2 `path.product` silently drops a third argument. Make it variadic.
- [ ] PATH-3 `join`/`product`: an absolute segment replaces everything before it; an empty segment adds a trailing slash. Document it, or raise/skip.
- [ ] PATH-4 An empty capture name gets through (`{*}`, `{?}`).
- [ ] PATH-5 Duplicate capture names are not rejected.
- [ ] PATH-6 The ambiguity rule is too strict (`{dirs*}/{file}` is rejected) and inconsistent with `{a?}/{b?}`.
- [ ] PATH-7 Absent optional captures: `{x?}` omits the key, `{x*?}` sets `""`.
- [ ] PATH-8 `capture` drops the leading `/`, while `relative_to` refuses to mix absolute and relative.
- [ ] PATH-9 `relative_to` doesn't handle `.` segments (`relative_to("a/b", ".")` → nil).
- [ ] PATH-10 Extension/stem edge cases are undocumented (dotfiles, `a.tar.gz`, `foo.`, `..`).
- [ ] PATH-11 `parent("/")` is `""`, and other root/`.` cases.
- [ ] PATH-12 Missing: `split`, `with_extension`, `with_file_name`, `is_absolute`, `normalize`, `..` output from `relative_to`.
- [ ] PATH-13 Error messages don't name the function/argument; pattern errors lack a prefix; base/pattern are coerced.
- [ ] PATH-14 Doc nits: the `relative_to` Errors line; nested arrays; the duplicated example.

### string / ansi (string.rs, ansi.rs) (see REG-6 / Q2)
- [ ] STR-1 The doc example has `if_nocolor` reversed.
- [ ] STR-2 A nil condition enables the style (`green(is_live)` with nil → both colours); non-booleans are truthy (X-2).
- [ ] STR-3 `padded`/`truncated`/`cropped`/`set` count escape characters in styled strings and can cut a sequence in half.
- [ ] STR-4 Only right padding exists. Add `padded_start`/`padded_end`, default the pad to `" "`.
- [ ] STR-5 `removed`/`replaced` are literal, and the doc doesn't say so; add an optional `n`.
- [ ] STR-6 Invalid UTF-8 raises (X-4).
- [ ] STR-7 Missing: `starts_with`, `ends_with`, `contains`, `without_prefix`, `without_suffix`, `lines`.
- [ ] STR-8 "`cropped` matches `string.sub`" is misleading (length vs end index, negative `len`).
- [ ] STR-9 "Nested styles survive" is overstated (a same-category inner reset clears the outer style).
- [ ] STR-10 Out-of-range handling differs between functions; `set` fails silently.
- [ ] STR-11 Naming: `set` breaks the past-participle convention; `stripped` reads as whitespace stripping.
- [ ] STR-12 `trimmed` uses Unicode whitespace (≠ `%s`); no custom trim chars.
- [ ] STR-13 `width` of multi-line/tab text (doc).
- [ ] STR-14 `strip` edge cases: OSC `\27`, two-byte escape + non-ASCII, C1 CSI; the fn doc omits OSC.
- [ ] STR-15 `truncated` overlaps `cropped`; add an ellipsis argument.

### ito (ito.rs) (see REG-1..5)
- [ ] ITO-1 `version_check("0.4.0")` examples fail against 0.7.0. **Decided:** keep the caret semantics (0.x minor = incompatible, the Rust convention); fix the examples and document the rule.
- [ ] ITO-2 A number argument is misread (`0.10` → `"0.1"`). Require a string.
- [ ] ITO-3 The `ito.colors(ito.is_tty("stderr"))` idiom overrides `NO_COLOR`. Add a per-stream auto mode.
- [ ] ITO-4 `require_dir()` in an `init.luau` returns the parent directory.
- [ ] ITO-5 `require_dir()` answers for the chunk that contains the call; pcall/C frames give nil. Document it.
- [ ] ITO-6 The anchors are symlink-resolved, with a silent fallback to `-C`. Document it.
- [ ] ITO-7 The error prefix reads `ito::version_check:`; it should be `ito.version_check:`.
- [ ] ITO-8 The null story contradicts itself (REG-4).
- [ ] ITO-9 The host-only rationale covers only half the functions.
- [ ] ITO-10 Stale lists (`os.getenv` siblings, "Two more … lists four").
- [ ] ITO-11 The slug doc mentions smic; the collision odds aren't stated.
- [ ] ITO-12 The pid "lock" example doesn't lock anything.
- [ ] ITO-13 No way to get the entry script's own path (REG-3).
- [ ] ITO-14 `NO_COLOR=""` disables colour, against the spec. Follow the spec or document the deviation.
- [ ] ITO-15 Extra arguments to `ito.colors` are ignored.

### io / log (io.rs, log.rs, run.rs)
- [ ] IO-1 `-v` does nothing (0 and 1 both map to info). Map 0→info, 1→debug, 2+→trace.
- [x] IO-2 `-q` doesn't silence `print`. ~~Decided: log it at debug~~ — **superseded**: no `print` rerouting at all; `ito` loses `-q`; `ied -q` (implied by `-d`/`-l`) silences `print` only.
- [x] IO-3 `-q` doesn't cover `io.write`. Moot: no `-q` on `ito`.
- [x] IO-4 `io.write` without a newline isn't flushed. Flush, or add `io.flush()`.
- [ ] IO-5 The "ANSI stripped in logs" claim is likely wrong (escaped instead) (?).
- [ ] IO-6 Log level labels are coloured even when stderr isn't a tty; this ignores `ito.colors`/`NO_COLOR`.
- [x] IO-7 `print` swallows write errors while `io.write` raises "Broken pipe" with exit 3. Treat EPIPE as a quiet exit. **Done:** a broken pipe ends the run with exit 141 (128 + SIGPIPE), quietly, via `print::emit`; `pcall` cannot hide it.
- [ ] IO-8 No read side (`io.read`/`io.lines`), so there's no piped stdin (X-7).
- [ ] IO-9 `iq` has no way to write to stderr. Register `ewrite`/`ewriteln` everywhere.
- [ ] IO-10 An invalid `ITO_LOG` is silently ignored, and when set it replaces `-v` (undocumented).
- [ ] IO-11 `log.md` omits the default level and an `ITO_LOG` example.
- [ ] IO-12 `log.error` doesn't affect the exit code (doc).
- [ ] IO-13 No format args/structured fields; tables print as `table: 0x…`; no caller location.
- [ ] IO-14 A failing `__tostring` renders `<error: …>` instead of raising.
- [ ] IO-15 `display_value` is copied three times; stale `debug_print` comment.
- [ ] IO-16 io/log chapters are not in the per-function shape.

### hash (hash.rs)
- [ ] HASH-1 The `iq`/`ied` fallback `hash.sha256(fs.read(p))` fails on binary files (`read_to_string`) (X-4, FS-12).
- [ ] HASH-2 No rooted `_file` forms, though the VFS resolver would confine them. Add `register_rooted_files`.
- [ ] HASH-3 A digest can only be encoded by hand → REG-10 (base64). Hex is still open (REG-11).
- [ ] HASH-4 No `_file_bytes`.
- [ ] HASH-5 The ied chapters don't list `hash`/`ansible`.
- [ ] HASH-6 The CLAUDE.md globals list is stale (X-10).
- [ ] HASH-7 No sha384/sha224.
- [ ] HASH-8 Case-sensitive comparison (uppercase published digests). Suggest `:lower()` or add `hash.equal`.
- [ ] HASH-9 The error wording differs from `fs`.
- [ ] HASH-10 The fallback snippet crashes in `iq` without `-f`.
- [ ] HASH-11 Every string input is copied before hashing.
- [ ] HASH-12 Paths must be UTF-8.
- [ ] HASH-13 No checksum-file helper (doc example?).
- [ ] HASH-14 Tests: pin a non-UTF-8 vector; check that `_file` is absent in ied.

### tablefmt (tablefmt.rs) (moves into the formatting module, REG-6)
- [ ] TFMT-1 `t:print()` uses `println!`: it gets around ied stdout and panics on EPIPE. Route it through print, or drop it.
- [ ] TFMT-2 The no-width output depends on the tty (Dynamic + tty) in every binary, so it isn't pure. Use the natural width; detect only on the host.
- [ ] TFMT-3 `:rows` takes only a list; a non-table element gives a generic error; map-shaped input is dropped.
- [ ] TFMT-4 Header vs cell conversion differs (numbers).
- [ ] TFMT-5 Bad rows are accepted silently (misspelled keys, extra values, an empty row, a table among the varargs).
- [ ] TFMT-6 `cell_text` shows `<error: …>` instead of raising.
- [ ] TFMT-7 `:align` overloads by arity and depends on call order; the header can't be aligned. Split into `align`/`align_cell`/`align_header`.
- [ ] TFMT-8 `t:headers()` with no arguments silently removes the header.
- [ ] TFMT-9 Width edge cases (negative, fractional, 0).
- [ ] TFMT-10 Constant casing differs; the preset list is duplicated; the modifiers aren't exposed.
- [ ] TFMT-11 The `NOTHING` preset doc is wrong about padding.
- [ ] TFMT-12 Missing: column width/nowrap/padding, footer, header style, `__tostring`.
- [ ] TFMT-13 Width counting differs from `string.width` (unicode-width vs chars).
- [ ] TFMT-14 "Lua" wording; the summary table omits the varargs forms.
- [ ] TFMT-15 No tests.

### ansible (ansible.rs) (see REG-9)
- [ ] ANS-1 The docs contradict themselves on `--vault-password-file` handling.
- [ ] ANS-2 The doc example `"~/.vault_pass"` fails (no `~` expansion).
- [ ] ANS-3 A relative executable is looked up on `$PATH` (P0-4). Make the path absolute or prefix `./`.
- [ ] ANS-4 The password script gets no stdin and its stderr is discarded.
- [ ] ANS-5 Empty passwords are accepted. Raise.
- [ ] ANS-6 Vault 1.2 (`--vault-id` label) files are rejected as "not a vault".
- [ ] ANS-7 `encrypt` is random, so in ied re-encrypting always shows a diff. **Decided:** `encrypt` becomes host-only.
- [ ] ANS-8 Naming (REG-9).
- [ ] ANS-9 No `ansible.vault.is_vault(text)`.
- [ ] ANS-10 No `!vault |` inline YAML helper or docs.
- [ ] ANS-11 Vault only; there's no inventory parsing. Say so, or add it later.
- [ ] ANS-12 The error docs are incomplete (header trimming, empty input, non-UTF-8, trailing lines).
- [ ] ANS-13 Style: "Lua type", placeholders, `key` → `password`.
- [ ] ANS-14 Stale comment (says std::fs; the code uses tokio::fs).
- [ ] ANS-15 Test gaps (indented body, tampered ciphertext, malformed payload, relative executable).

### require (require.rs)
- [ ] REQ-1 The root's sibling `<root>.lua` is loadable via `./.`; an ambiguous `proj.lua` + `proj/init.lua` fails. At the root, only consider `init.*`; add a backstop containment check.
- [ ] REQ-2 Confinement is textual only: symlinks escape, and macOS `/tmp` aliases are rejected. Canonicalize.
- [ ] REQ-3 Dotted names: `with_extension` replaces the extension (`config.prod` → `config.luau`). Append instead.
- [ ] REQ-4 Escape errors are vague ("could not get parent"). Wrap `require` and report the root.
- [ ] REQ-5 An entry `init.lua` resolves `./` differently than a required one.
- [ ] REQ-6 A symlinked module resolves beside the link; cache keys aren't canonical, so a module can load twice.
- [ ] REQ-7 An error on an intermediate segment is misleading; ambiguity is raised too early.
- [ ] REQ-8 Chunk names are absolute paths with no extension, so tracebacks are hard to use.
- [ ] REQ-9 `.config.luau` is not supported; alias lowercasing and chaining are undocumented.
- [ ] REQ-10 Explicit extensions: the doc says "exact file", but the code falls back; it's also a portability difference.
- [ ] REQ-11 The entry script can be required and runs again. Pre-seed the cache.
- [ ] REQ-12 A module that yields during load while another task requires it (?).
- [ ] REQ-13 Tests for items 1–5 and 7.

### ui.progress (ui/progress.rs)
- [ ] PROG-1 `suspend` holds the multi lock, so touching a bar inside deadlocks; "safe to nest" is wrong.
- [ ] PROG-2 `suspend` is sync, so the documented `ui.prompt` example fails. Make it async (without holding the lock across the await).
- [ ] PROG-3 `print`/`log` garble live bars on a tty. Route them through `MULTI.println`/`suspend`.
- [ ] PROG-4 `h:println`/finish messages are dropped when not a tty. Fall back to eprintln.
- [ ] PROG-5 No teardown on script error. Add `progress::teardown()` in `Engine::eval`.
- [ ] PROG-6 `finish`/`abandon` don't leave the multi (the docs say they do); zombies pile up.
- [ ] PROG-7 `ui.progress.clear()` erases nothing and splits the display across two multis.
- [ ] PROG-8 `style()` starts from `default_bar`, so it drops `{msg}` and turns a spinner into a bar.
- [ ] PROG-9 A bad style or tick interval panics; `enable_steady_tick` should take `parse_delta`.
- [ ] PROG-10 Colour ignores `ito.colors()`.
- [ ] PROG-11 The doc example uses `pool:await_all()`, which doesn't exist.
- [ ] PROG-12 Bars have no steady tick; `h:inc()` with no argument raises.
- [ ] PROG-13 The chapter is tables only (required shape).
- [ ] PROG-14 `MULTI` isn't reset per engine build (tests).
- [ ] PROG-15 Wording: "with_*-chained", "(bar only)".

### ui.prompt (ui/prompt.rs)
- [ ] PROM-1 Ctrl-C kills the process (console raises SIGINT). Map it to a Luau error, or document it.
- [ ] PROM-2 The terminal is left broken (hidden cursor, echo off) on Ctrl-C/error. Add a restore guard.
- [ ] PROM-3 Escape is ignored by `:interact()` (the doc says it raises); `q` quits under `interact_opt` (undocumented).
- [ ] PROM-4 confirm has `interact_opt` too; the docs say only select/multi_select.
- [ ] PROM-5 The non-tty error is vague; the check is on stderr; there is no fallback to the default. Add a knob.
- [ ] PROM-6 The auto theme reads stdout, but prompts draw on stderr.
- [ ] PROM-7 Builder index validation is call-order dependent; `:items()` leaves defaults stale or resets checks. Validate at `:interact()`.
- [ ] PROM-8 Bool arguments are truthy (X-2).
- [ ] PROM-9 Nothing serialises concurrent prompts; progress bars redraw over a prompt. Add an async mutex and suspend the multi.
- [ ] PROM-10 An abandoned prompt keeps reading keys and blocks exit.
- [ ] PROM-11 select returns only the index. Return `index, item`.
- [ ] PROM-12 `:report(true)` is a no-op without `:prompt()`; the input shorthand can't accept an empty answer; the password strings are hardcoded.
- [ ] PROM-13 Doc mismatches (the shorthand count, the signature, confirm's Enter behaviour); required shape.
- [ ] PROM-14 Duplicated `interact`/`interact_opt`/`run_blocking` code.
- [ ] PROM-15 Test gaps.

### aws (shared: mod.rs) (see X-8)
- [ ] AWS-1 An empty id/name list widens to the whole account. Raise on an explicitly empty list.
- [ ] AWS-2 Shared `time_arg` accepting RFC 3339 strings (via `time`'s `TimeArg`).
- [ ] AWS-3 Shared error-prefix helper (`<fn>: …`), and apply it in `value_to_string_list`.
- [ ] AWS-4 Shared tag parsing for ec2/autoscaling (value coercion, empty set).
- [ ] AWS-5 `mod.rs` docs: all five services, the ambient list.
- [ ] AWS-6 Result-order conventions per module (newest-first vs oldest-first) in one place.

### aws.ec2 (aws/ec2.rs)
- [ ] EC2-1 `delete_tags(ids):tags({})` wipes all tags (P0-1).
- [ ] EC2-2 `:ids` takes `Vec<String>` on three builders but `Value` elsewhere.
- [ ] EC2-3 An unknown id raises `*.NotFound` although "never raises on no matches" is promised. Map it to empty.
- [ ] EC2-4 Repeated filter calls AND together. Merge same-name filters or document it.
- [ ] EC2-5 No `:wait()` on start/stop/terminate.
- [ ] EC2-6 Doc mismatches ("three describes", missing status rows, the `:name` row, `deprecation_time`).
- [ ] EC2-7 `:name` is `tag:Name` on instances but the AMI `name` on images. Rename it to `:image_name`?
- [ ] EC2-8 Validation errors don't name the call (AWS-3).
- [ ] EC2-9 Sloppy input accepted (`{123}`, hash part ignored, `""` id, empty filter values).
- [ ] EC2-10 A dry run returns `nil` data. Return an empty table, or set `envelope.dry_run`.
- [ ] EC2-11 Stopped instances under `include_all_instances` get "not-applicable" status, not absent (?).
- [ ] EC2-12 Missing fields: key_name, dns names, security_groups, iam profile, platform, lifecycle, block devices; image block devices; snapshot kms_key_id.
- [ ] EC2-13 Missing operations: describe_volumes/SGs/VPCs/addresses, create/deregister image, create/delete snapshot.
- [ ] EC2-14 The aws-config.md example binds the envelope; its service list is incomplete.
- [ ] EC2-15 Builders are shaped differently internally. Add a shared paginate helper.

### aws.s3 (aws/s3.rs)
- [ ] S3-1 `head_object` on a missing bucket gives `ok=false`; the docs say it raises.
- [ ] S3-2 No `head_bucket`.
- [ ] S3-3 `max_keys` + delimiter drops `common_prefixes`.
- [ ] S3-4 `max_keys(n ≤ 0)` returns one entry. Validate, and use `min(n, 1000)` as the page size.
- [ ] S3-5 There's no truncation flag or resume cursor.
- [ ] S3-6 `list_object_versions` + `max_keys` is not a key-ordered prefix.
- [ ] S3-7 A tagging `:send(target)` is silently ignored.
- [ ] S3-8 The empty-name check is on tagging only.
- [ ] S3-9 `get_object` and `head_object` return metadata in different places with different fields; no `tag_count`.
- [ ] S3-10 `get_object` parses by content type, so the return type depends on the uploader. **Decided:** always return a string; parse on request.
- [ ] S3-11 Empty lists encode as `{}` (X-3).
- [ ] S3-12 A 403 without ListBucket looks like an error, not "not found" (doc).
- [ ] S3-13 `get_object_tagging` raises on a missing key while head/get return ok=false.
- [ ] S3-14 The version examples use a prefix as if it were an exact key.
- [ ] S3-15 Nits:
  - the tag-skip comment;
  - `respond` shadows `aws::respond`;
  - no put/delete/copy/presign (consistent with the scope).

### aws.autoscaling (aws/autoscaling.rs)
- [ ] ASG-1 No `:launch_template` on `update_group`/`start_instance_refresh` (DesiredConfiguration), so a new-version rollout is impossible.
- [ ] ASG-2 Missing: terminate_instance, set_instance_health/protection, standby, suspend/resume, attach/detach, lifecycle hooks.
- [ ] ASG-3 An empty list widens the query to the whole account (AWS-1).
- [ ] ASG-4 Groups with a mixed-instances policy show no launch config; many group fields are dropped.
- [ ] ASG-5 Instance rows lack the launch template/version.
- [ ] ASG-6 Target-tracking/step policies come back nearly empty.
- [ ] ASG-7 `:wait()` exists only on start (not rollback/cancel/describe); one transient error aborts the wait; the first poll comes only after 15s.
- [ ] ASG-8 Error prefixes are inconsistent (AWS-3).
- [ ] ASG-9 The empty group name is checked only by the tag writers.
- [ ] ASG-10 Boolean builder methods take arguments inconsistently.
- [ ] ASG-11 Tag coercion differs from ec2 (AWS-4).
- [ ] ASG-12 How ids are passed is inconsistent; `:name` means the group name here but `tag:Name` in ec2.
- [ ] ASG-13 The id-list API limits are neither chunked nor documented.
- [ ] ASG-14 The refresh builder accepts invalid values; empty preferences are sent; refresh details aren't returned; many preference options are missing.
- [ ] ASG-15 Doc mismatches (`:filter` scope, the `instance_warmup` fallback, pagination lists, the stale mod.rs text).

### aws.cloudtrail (aws/cloudtrail.rs)
- [ ] CT-1 No pacing, so throttling is possible (2 TPS); a mid-walk failure loses everything.
- [ ] CT-2 `:read_only()` is truthy (`"false"` → read events) (X-2).
- [ ] CT-3 Attributes silently replace each other. Raise on a second one.
- [ ] CT-4 JSON null inside `raw` becomes the truthy sentinel (doc/convert).
- [ ] CT-5 `raw` keys are camelCase (undocumented); promote error_code/aws_region/source_ip.
- [ ] CT-6 Time is a number going in and a string coming out (AWS-2).
- [ ] CT-7 `:filter` keys aren't validated.
- [ ] CT-8 The Insights attribute limit isn't enforced; `:insights()` can't be undone.
- [ ] CT-9 Hitting `max_events` gives no truncation flag.
- [ ] CT-10 No start ≤ end check.
- [ ] CT-11 Stale mod.rs docs (AWS-5).
- [ ] CT-12 `time_arg` duplicated (AWS-2).
- [ ] CT-13 Result order differs between siblings (AWS-6).
- [ ] CT-14 Test gaps.

### aws.cloudwatch (aws/cloudwatch.rs)
- [ ] CW-1 Statistics and percentiles can't be combined (AWS rejects it), yet the docs, the code and a test say they can.
- [ ] CW-2 The `time.ago(time.hours(3))` example is used as a timestamp. Use `time.now() - time.hours(3)`.
- [ ] CW-3 Numeric time only (AWS-2).
- [ ] CW-4 Three positional arguments break mod.rs's rule. Take a table, or amend the rule.
- [ ] CW-5 The 1440-datapoint cap isn't checked locally.
- [ ] CW-6 A bad period resolves credentials before it raises.
- [ ] CW-7 `:unit()` accepts any string.
- [ ] CW-8 Dimensions aren't deduplicated.
- [ ] CW-9 `list_metrics` has no bound. Add `:max_metrics(n)`.
- [ ] CW-10 `list_metrics` rejects nil positionals.
- [ ] CW-11 Data retention vs period is undocumented.
- [ ] CW-12 Period rounding gives a confusing error.
- [ ] CW-13 The rate-limit figure is likely wrong (?).
- [ ] CW-14 Error timing wording; no start < end check.
- [ ] CW-15 No GetMetricData.

### aws.config (aws/config.rs)
- [ ] AWSC-1 `:build()` never resolves credentials, so it can't raise on a bad profile. Force resolution or fix the docs.
- [ ] AWSC-2 Env vars beat `:profile()` (P0-8). Build the providers from the profile alone.
- [ ] AWSC-3 The doc examples bind the envelope; the spanning-regions example prints 0.
- [ ] AWSC-4 The doc lists only 3 of the 5 services.
- [ ] AWSC-5 `endpoint_url` without `force_path_style` breaks MinIO/LocalStack bucket operations. Add `:path_style`.
- [ ] AWSC-6 Chained setters mutate a shared builder. Copy on set, or add `:clone()`.
- [ ] AWSC-7 `Arc<Mutex>` breaks the `Rc` rule (and adds poison unwraps).
- [ ] AWSC-8 Bad `retries` values give an inconsistent error.
- [ ] AWSC-9 Empty strings are accepted and fail far from the call.
- [ ] AWSC-10 There's no attempt/connect timeout; `RetryConfig::standard()` overrides the profile's retry mode (doc).
- [ ] AWSC-11 No static credentials/assume-role/sso_session.
- [ ] AWSC-12 Explicit clients are rebuilt per call. Cache them.
- [ ] AWSC-13 A client can't report its region/profile.
- [ ] AWSC-14 Test gaps.
