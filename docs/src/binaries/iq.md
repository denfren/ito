# `iq` — query structured files

A filter: text in, text out. `iq` runs a [Luau](https://luau.org) script
over data you hand it and prints what the script returns.

```sh
iq q.luau data.json           # query a file
cat data.json | iq q.luau -   # query stdin
iq -s 'return input:upper()' -
iq q.luau data.json :: --key name   # ... with arguments for the script itself
```

## The operating model

**The return value is the output.** The script must return a string, and
that string is written to stdout verbatim. There is no `print` — a second
output channel would only interleave with the answer.

```sh
$ iq -s 'return json.parse(input).name' package.json
my-project
```

**Nothing can be changed.** `fs.write` and `fs.remove` do not exist, and
there is no way to run a command or open a network connection. A query
reads and computes; that is all it can do.

**Input is opt-in, and so is the filesystem.** Two independent sources,
neither on by default:

| | How | In the script |
| --- | --- | --- |
| One file, or stdin | `iq q.luau <path>` / `iq q.luau -` | `input` — the raw text |
| The whole directory | `iq q.luau -f` | `fs` — read-only, rooted at the working directory |

`input` is **raw text**, never parsed for you: stdin carries no
indication of its own format, so the script says which parser it wants.

```lua
local cfg = json.parse(input)   -- or yaml.parse, toml.parse, hcl.parse
return cfg.version
```

Omit both and `input` is nil and `fs` is undefined — fine for a query
that only needs its own arguments.

## Modules

| Module | For |
| --- | --- |
| [`fs`](../modules/fs.md) | read files under the working directory (needs `-f`; read-only) |
| [`path`](../modules/path.md) | join, split and inspect paths as strings — no disk access |
| [`time`](../modules/time.md) | convert between timestamp strings, Unix seconds and duration deltas |
| [`json`](../modules/json.md) | parse and emit JSON |
| [`toml`](../modules/toml.md) | parse and emit TOML |
| [`yaml`](../modules/yaml.md) | parse and emit YAML |
| [`hcl`](../modules/hcl.md) | parse HCL, and build it with a fluent builder |
| [`j2`](../modules/j2.md) | render Jinja2 templates |
| [`re`](../modules/re.md) | regular expressions |
| [`string`](../modules/string.md) | character-aware string helpers, ANSI styling |
| [`table`](../modules/table.md) | arrays and objects |
| [`ito`](../modules/ito.md) | the version guard, the `null` sentinel, and the colour switch |
| [`log`](../modules/log.md) | logging to stderr, which never pollutes the answer on stdout |
| [`cli`](../modules/cli.md) | declare the query's own options and arguments, with a generated `--help` |

Print any of them with `--doc <topic>`, e.g. `iq --doc fs`.

## Gotchas

**Returning a non-string is an error.** Convert explicitly — a table has
no single obvious rendering, so `iq` will not guess one:

```lua
return json.to_string(result)          -- not: return result
return tostring(count)                 -- not: return count
```

**`-C` requires `-f`.** `-C` sets the `fs` root, so without `-f` there is
no filesystem for it to point at; passing it alone is a usage error
rather than a silently ignored flag.

**`SCRIPT` and `INPUT` cannot both be `-`.** Stdin can only be read once.
Use `-s` for an inline script, or name a file for one of the two.

**`INPUT` is not a `cli.arg()`.** The input file is `iq`'s own positional,
matched before `::`, so a script's [`cli`](../modules/cli.md) declarations
never see it — `cli.arg("file")` reads whatever came *after* `::`, which is
a different thing. The data arrives as `input`, as always.

**Log, don't print.** Diagnostics belong on stderr via
[`log`](../modules/log.md) (`log.info`, `log.debug`, …). Raise verbosity
with `-v`, `-vv`, `-vvv`.

**Exit codes.** `0` success, `2` a failure that is not the script's fault
(usage, a missing input file), `3` a failure in the script itself (a
parse or runtime error).
