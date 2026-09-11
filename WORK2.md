# WORK2 — open points after the WORK.md pass

Two parts: decisions still open from the `io`/`print`/`-q`/`iq` redesign
sketch, then the WORK.md items the agents skipped. `[ ]` open, `[x]` done.

---

## 1. Open decisions: `io` / `print` / `-q` / `iq`

Context: `iq` is being removed and `io`/`print`/`-q` cleaned up (in progress,
separately). The sketch it follows:

- `ied` and `ito` both get a vanilla `print` (`tostring` of each argument, tab
  joined, newline, always to stdout; a closed pipe exits quietly).
- `ito` gets a host-only `io` with `io.stdout`/`io.stderr`/`io.stdin` handles
  (`:write` chainable and strict like vanilla, `:flush`, `:close`, `:setvbuf`;
  `stdin` also `:read`/`:lines`) and `io.write`/`io.read`/`io.lines`/`io.flush`
  as the same calls on the default handles. No `io.open`, no
  `io.output`/`io.input` setters.
- One internal `emit(stream, bytes)` sink serves `print` and the handles: it
  suspends progress bars, flushes, and handles the closed pipe.
- `ito` loses `-q`; `-v`/`-vv` stay (logs only).

Resolved as implemented (differs from the sketch in one point): `ied -q` stays but silences **only `print`**; `-d`/`-l` imply `-q`; logs and report text are unaffected. `print` is vanilla, `:write` is strict, there is no `writeln`/`ewrite`/`eprint`, and `ito` has no `-q`. Docs, CHANGELOG and clap help are updated.

- [x] **OPEN-1** `ied -q`: keep it, covering the report text and logs only
  (the sketch's pick), or drop it too? Related: `-d`/`-l` would no longer
  imply `-q` for `print`, so a script that prints under `-d`/`-l` pollutes the
  diff or list. The sketch's documented rule is "a script must not print under
  `-d`/`-l`".
- [x] **OPEN-2** `print` of a plain table: vanilla `table: 0x…` (the sketch's
  pick), or keep the JSON rendering ioui added?
- [x] **OPEN-3** `ied` loses `io.ewrite`/`ewriteln`, so its only stderr path is
  `log.*`. Accept that, or keep `io.stderr` in `ied`? An `eprint(...)`
  global (`print` for stderr) is a possible convenience.
- [x] **OPEN-4** `io.write` and `:write` strictness: vanilla raises on
  non-string/number arguments. Keep that, or keep today's `tostring`-style
  rendering?
- [x] **OPEN-5** `io.writeln`: dropped in the sketch (`print` and
  `io.stderr:write(s, "\n")` cover it). Confirm, or add
  `io.stdout:writeln`/`io.stderr:writeln`.
- [x] **OPEN-6** WORK.md IO-2 ("under `-q`, `print` logs at debug") is
  superseded by this redesign. Record the new decision there.
- [x] **OPEN-7** Docs to rewrite for the redesign: `io.md`, `log.md`,
  `ito.md`, `ied.md`, `intro.md`, README, CLAUDE.md (capability table, the
  `iq` paragraph, "three front doors"), CHANGELOG (breaking), clap `-q`/`-v`
  help text, and the `--doc` program-name logic.

---

## 2. Skipped in the WORK.md pass

Items the agents did not implement, by area. Check against WORK.md for the
original wording.

### Decided against or deferred on purpose
- [ ] **TOML-2** format-preserving `toml.edit` (needs a whole cursor API).
- [ ] **TOML-1** write direction: a `toml.datetime(s)` constructor.
- [ ] **HCL-11** gaps vs yaml (comment editing, `rename_key`, `contains`,
  `read()` of a block). D3 skipped the vocabulary unification.
- [ ] **TIME-9** `format_rfc3339` alias; the docs were corrected instead.
- [ ] **CLI-7** stop-at-first-positional, and an `env=` fallback (`cli` exists
  where there is no ambient state).

### Not done
- [ ] **HASH-2** rooted `_file` forms for the VFS binaries.
- [ ] **REQ-11** the entry script can be required and runs again (pre-seed the
  cache). **REQ-12** a module that yields while loading. REQ-4 only partly:
  `require` errors do not name the root.
- [ ] **X-4** non-UTF-8 input still raises in `path`, `j2` template text/data
  and `string` (STR-6). Done for `fs`, `re`, `proc`, `hash` paths.
- [ ] **TFMT-12** table footer, column `nowrap`, header style. Host terminal
  width detection is not implemented: tables never consult the terminal.
- [ ] **TASK-14** first-of for plain handles.
- [ ] **CLI-11** wrapping of help descriptions. `cli.parse(list)` is one-shot,
  so subcommands cannot redeclare options in a second parse.
- [ ] **YAML-15** `try_*` coverage; YAML-6 block scalars.
- [x] **PROM-6** `ui.prompt` auto theme reads stdout while prompts draw on
  stderr (needs `fmt.colors` per-stream detection). **PROM-10** an abandoned
  prompt keeps reading keys (documented, not fixed). **PROG-6** lingering
  finished bars (documented, indicatif reaps them). **IO-13** format args,
  structured fields, caller location.
- [ ] **REDIR-12** a stdout file target is still truncated if the spawn then
  fails (documented). **SSH-9** `disconnect()` not exposed.
- [ ] **ASG-2** new autoscaling operations (terminate/health/protection,
  standby, suspend/resume, attach/detach, lifecycle hooks). Also skipped:
  chunking for group names, activity ids and policy names, and many instance
  refresh preference options.
- [ ] **CW-15** `GetMetricData`. **CW-4** three positional arguments rule left
  as is. **CT-1** partial result on a mid-walk failure. **CT-8** the Insights
  attribute limit is not enforced.
- [ ] **AWS-4** shared tag parsing for ec2/autoscaling. **AWS-6** result-order
  conventions in one place. **CT-13** result order across siblings.
- [ ] **EC2-2/5/7/11/12/13/15**, **S3-2/5/9**, **AWSC-3/10/11/12/13**.
  EC2-4: repeated filters still AND (documented, not merged).

### To verify
- [ ] `ito/tests/proc_trap.rs` and the full suite pass on the merged tree
  (1154 tests), but no ssh test ran against a real server, no `ui.prompt` ran
  on a real terminal, and the AWS changes are unit-tested only.
- [ ] CW-13: the cloudwatch rate-limit wording was written from memory and is
  unverified.
- [ ] Unrequested behaviour changes to confirm: JSON/YAML/TOML output keys are
  always sorted; yaml `remove_at` out of range raises; ssh 127/255 return a
  result instead of raising.
