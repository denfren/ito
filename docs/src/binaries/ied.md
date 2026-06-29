# `ied` — edit files in a sandbox

Give `ied` a [Luau](https://luau.org) script and a directory, and it
proposes changes to the files it finds there. By default it proposes
only: nothing is written unless you ask for it.

```sh
ied fix.luau                  # compute the changes, write nothing
ied fix.luau -d               # show them as a diff
ied fix.luau -l               # list the files that would change
ied fix.luau -e               # exit 1 if anything would change
ied fix.luau -w               # apply them
ied fix.luau data.json -w     # ... with a file handed to it as `input`
ied fix.luau -w :: src lib    # ... with arguments for the script itself
```

That default makes it usable as a linter or a config-propagation check:
run it in CI with `-e` and a non-zero exit means the tree has drifted
from what the script says it should be.

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

**Files are all it can reach.** There is no way to run a command, open a
network connection, or read the environment. The script sees a virtual
filesystem rooted at the working directory (`-C`), and paths that escape
that root with `..` are rejected.

**Reports own stdout.** `--diff` and `--list-changed` write to stdout and
imply `--quiet`, so a script's `print` output can never interleave with a
diff you are about to pipe into `patch`.

**Data can arrive on the command line.** An optional `INPUT` positional —
a file path, or `-` for stdin — reaches the script as the global `input`:

```sh
ied fix.luau versions.json -d       # input = the file's text
curl -s "$URL" | ied fix.luau - -d  # input = stdin
```

`input` is **raw text**, never parsed for you: a file's extension is a
hint rather than a fact, and stdin carries no indication of its format, so
the script says which parser it wants.

```lua
local wanted = json.parse(input)
for name, version in pairs(wanted) do
    fs.write(name .. "/VERSION", version)
end
```

Omit the positional and `input` is nil, which is distinct from an empty
string — a script can tell "nothing was passed" from "something empty was
passed". The files the script *edits* still come from `fs`; `input` is the
one value handed in from outside the tree.

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
| [`string`](../modules/string.md) | character-aware string helpers, ANSI styling |
| [`table`](../modules/table.md) | arrays and objects |
| [`ito`](../modules/ito.md) | the version guard, the `null` sentinel, and the colour switch |
| [`log`](../modules/log.md) | logging to stderr |
| [`cli`](../modules/cli.md) | declare the script's own options and arguments, with a generated `--help` |

Print any of them with `--doc <topic>`, e.g. `ied --doc fs`.

## Gotchas

**`--diff` and `--list-changed` are mutually exclusive.** Each is a
different rendering of the same change set; asking for both is a usage
error.

**`SCRIPT` and `INPUT` cannot both be `-`.** Stdin can only be read once.
Use `-s` for an inline script, or name a file for one of the two.

**`INPUT` is not a `cli.arg()`.** The input file is `ied`'s own
positional, matched before `::`, so a script's
[`cli`](../modules/cli.md) declarations never see it — `cli.arg("file")`
reads whatever came *after* `::`, which is a different thing. The data
arrives as `input`, as always.

**A "change" means differing from disk.** Writing a file the exact bytes
it already contains is not a change: it does not appear in `--diff` or
`--list-changed`, and does not trigger `--exit-code`.

**Prefer the format-preserving editors for config you care about.**
`yaml.parse` + `yaml.to_string` is a lossy round-trip — comments and
layout are rebuilt, not preserved. Use `yaml.edit` / `hcl.edit` to make
surgical changes that leave the rest of the file byte-identical.

**Exit codes.** `0` success, `1` success *with* changes (only under
`-e`/`--exit-code`, like `git diff --exit-code`), `2` a failure that is
not the script's fault, `3` a failure in the script itself.
