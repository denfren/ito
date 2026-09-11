# `ied` — reproducible file edits

`ied` is `ito` with the world taken away: the same Luau runner and
standard library, restricted to the files in one directory. Give it a
[Luau](https://luau.org) script and a directory, and it proposes changes
to the files it finds there. By default it proposes only: nothing is
written unless you ask for it.

Everything this page does not list as unavailable works as it does in
`ito`.

```sh
ied fix.luau                  # compute the changes, write nothing
ied -d fix.luau               # show them as a diff
ied -l fix.luau               # list the files that would change
ied -e fix.luau               # exit 1 if anything would change
ied -w fix.luau               # apply them
ied -w fix.luau src lib       # ... with arguments for the script itself
```

`ied`'s own flags go before the script path. Everything after it belongs
to the script and reaches it verbatim as the global `argv` — so in
`ied fix.luau -w`, the `-w` is the script's, and nothing is written.

That default makes it usable as a linter or a config-propagation check:
run it in CI with `-e` and a non-zero exit means the tree has drifted
from what the script says it should be.

## Reproducibility

An `ied` run is a function of two things: **the state of the work
directory and the script**. The same pair always produces the same change
set, on any machine, at any time. That is what makes a bare run a
trustworthy preview, `-e` a usable CI check, and `-d` output safe to
review and then apply with `-w`: the changes you inspected are the changes
that get written.

Anything that would let a third input in is therefore absent. These
globals are not stubbed or policy-checked — they do not exist, so
`if proc then` or `if os then` is a usable feature test:

| Input | What is absent |
| --- | --- |
| time | `time.now`/`till`/`ago`, the `_local` family (host time zone), `os.clock`/`date`/`time`/`difftime` |
| randomness | `math.random`/`randomseed`, `ansible.vault.encrypt` (fresh salt per call) |
| environment | `os.getenv`, `fs.cwd`, `fs.script`, `fs.require_root`/`require_dir`, `proc.pid`, `ito.slug` |
| files outside the work directory | `fs` is rooted at the work directory: `..` and symlink escapes are errors, and `hash.*_file` does not exist |
| other processes and machines | `proc`, `ssh`, `aws`, `task` |
| the terminal | `io`, `ui`, `fmt.is_tty`; colour auto-detection resolves to off (`fmt.colors(true)` opts in) |
| effects with no preview | `fs.append`/`mkdir`/`remove_all`/`rename`/`copy`/`chmod`/`symlink`/`temp_*`/`write_new`, `fs.finder():grep()` |

Arguments (`argv`) are part of the invocation, so a script may take data
through them; the files they name are read from inside the tree. The only
output besides `print` is `log.*`, which goes to stderr.

## The operating model

**Writes are buffered, and writing is opt-in.** `fs.write` and
`fs.remove` queue a change rather than performing one. After the script
returns, the queue is applied only if `--write` was given — otherwise it
is reported on and dropped.

Because nothing lands until the end, the whole change set is one unit:
it can be diffed, listed, counted, or abandoned.

**A script cannot read its own buffered writes.** `fs.read` always
reflects what is on disk right now, never a pending write:

```lua
local before = fs.read("config.toml")
fs.write("config.toml", "updated = true")
fs.read("config.toml")     -- still the original text
```

Keep the value you computed rather than reading it back.

**Last call to a path wins.** Each path has a single pending slot, so a
later `fs.write` or `fs.remove` replaces an earlier one:

```lua
fs.write("a", "x") fs.remove("a") fs.write("a", "y")   -- writes "y"
```

**Reports own stdout.** `--diff` and `--list-changed` write their report
to stdout, so they imply `-q`: the script's `print` is silenced and cannot
land inside the diff you are about to pipe into `patch`. Use `log.*` for
diagnostics: it goes to stderr.

**`-q` silences `print`.** `-q`/`--quiet` turns the script's `print`
(`tostring` of each argument, tab-joined, newline-terminated) into a
silent no-op. It affects nothing else: not the report, not logs, not exit
codes or `-w`. Combined with `-d` or `-l` it is redundant.

**Data is read through `fs`.** A file the script should take its values
from is named as one of its arguments and read like any other, from
inside the tree:

```sh
ied -d fix.luau versions.json
```

```lua
local wanted = json.parse(fs.read(argv[1]))
for name, version in pairs(wanted) do
    fs.write(name .. "/VERSION", version)
end
```

What `fs.read` returns is **raw text**, never parsed for you: a file's
extension is a hint rather than a fact, so the script says which parser it
wants.

## Modules

| Module | For |
| --- | --- |
| [`fs`](../modules/fs.md) | read, write and remove files under the working directory |
| [`path`](../modules/path.md) | join, split and inspect paths as strings — no disk access |
| [`time`](../modules/time.md) | convert between timestamp strings, Unix seconds and duration deltas |
| [`json`](../modules/json.md) | parse and emit JSON |
| [`toml`](../modules/toml.md) | parse and emit TOML |
| [`yaml`](../modules/yaml.md) | parse and emit YAML, including format-preserving edits |
| [`hcl`](../modules/hcl.md) | parse and emit HCL, including format-preserving edits |
| [`j2`](../modules/j2.md) | render Jinja2 templates |
| [`re`](../modules/re.md) | regular expressions |
| [`fmt`](../modules/fmt.md) | colours, text helpers, status symbols, aligned tables |
| [`string`](../modules/string.md) | character-aware string helpers (sugar over `fmt`) |
| [`table`](../modules/table.md) | arrays and objects, and the `table.array` marker (plus the global `null`) |
| [`ito`](../modules/ito.md) | the version guard (no `ito.slug`) |
| [`hash`](../modules/hash.md) | MD5/SHA-1/SHA-224/SHA-256/SHA-384/SHA-512 of a string or buffer (no `_file` forms) |
| [`encoding`](../modules/encoding.md) | base64, with an engine builder for other alphabets |
| [`ansible`](../modules/ansible.md) | decrypt Ansible Vault text, `is_vault` (no `encrypt`) |
| [`log`](../modules/log.md) | logging to stderr, the only route to it |
| [`cli`](../modules/cli.md) | declare the script's own options and arguments, with a generated `--help` |

The modules `proc`, `ssh`, `aws`, `task`, `io` and `ui` are absent. Print
any topic with `--doc <topic>`, e.g. `ied --doc fs`.

## Gotchas

**`--diff` and `--list-changed` are mutually exclusive.** Each is a
different rendering of the same change set; asking for both is a usage
error.

**`-w` goes before the script.** After the script path it is one of the
script's arguments, so `ied fix.luau -w` computes the change set and drops
it, like a bare run. The same holds for `-d`, `-l`, `-e`, `-q` and `-C`.

**A "change" means differing from disk.** Writing a file the exact bytes
it already contains is not a change: it does not appear in `--diff` or
`--list-changed`, and does not trigger `--exit-code`.

**Prefer the format-preserving editors for config you care about.**
`yaml.parse` + `yaml.to_string` is a lossy round-trip — comments and
layout are rebuilt, not preserved. Use `yaml.edit` / `hcl.edit` to make
surgical changes that leave the rest of the file byte-identical.

**Exit codes.** `0` success, `1` success *with* changes (only under
`-e`/`--exit-code`, like `git diff --exit-code`), `2` a failure that is
not the script's fault, `3` a failure in the script itself, `141` the
output pipe went away (`| head`; the run was cut short, and `-w` does not
apply its buffered edits).
