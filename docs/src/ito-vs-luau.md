# ito vs vanilla Luau

ito runs scripts on [Luau](https://luau.org), Roblox's Lua dialect (a
superset of Lua 5.1 with, among other things, string interpolation,
`continue`, compound assignment operators, and native type-annotation
syntax that ito does not use). What follows is what's different from a
plain Luau environment you might already know from elsewhere.

## Modules provided by ito

These are not part of Luau itself — they are ito's stdlib, installed as
plain global tables before your script runs. Call their functions with
`.` (`fs.read(...)`), and call methods on values they return with `:`
(`h:read()`) — see
[Method naming conventions](conventions.md#module-functions-vs-methods).

| Module | What it provides |
| --- | --- |
| [`string`](modules/string.md) | A few extra pure string functions, added to Luau's own `string` table |
| [`table`](modules/table.md) | Luau's own `table` library, plus `table.array` and the global `null` |
| [`fs`](modules/fs.md) | Read, write, list, remove files |
| [`path`](modules/path.md) | Path manipulation |
| [`re`](modules/re.md) | Regular expressions (the `regex` crate's syntax, not Lua patterns) |
| [`json`](modules/json.md) | JSON parse / emit |
| [`toml`](modules/toml.md) | TOML parse / emit |
| [`yaml`](modules/yaml.md) | YAML parse / emit |
| [`hcl`](modules/hcl.md) | HCL parse / emit |
| [`j2`](modules/j2.md) | Jinja2 template rendering |
| [`ito`](modules/ito.md) | Version guard, `ito.slug`, and `os.getenv` |

## `require` looks like Luau's — and is confined

`require` keeps Luau's familiar shape: the same `.luaurc`/path-alias/`init.lua`
resolution, resolved relative to the requiring file. What differs is that
`ito` replaces the resolver with its own, confined to the directory tree of
the **entry script** — its *require root*. A `../`-relative path may walk
*within* that tree but never above it; an escaping `require` is an error,
not a read. Aliases are bounded the same way, and the search for `.luaurc`
stops at the root, so a config in a parent directory cannot redirect a
confined script's imports.

The require root is **independent of the `fs`/`-C` root**, which constrains
file *operations* rather than imports. See [require](luau/require.md) for exactly how paths resolve.

```lua
local m = require("./lib")   -- loads lib.lua next to this script
print(m.hello())
```

## What's sandboxed

A script cannot touch the filesystem, spawn processes, or read
environment/OS state except through ito's own modules:

- **No file I/O.** Luau's file-oriented `io` functions (`io.open` and
  friends) are not present; ito has its own stream-handle
  [`io`](modules/io.md). All file access goes through
  [`fs`](modules/fs.md).
- **Environment access is re-added.** Luau's sandboxed `os` has no
  `getenv` at all; ito puts it back, so `os.getenv(<name>)` reads a
  variable. There is no setter: pass a variable to a
  child with a command's own [`:env()`](modules/proc.md) instead of
  mutating state that every later task would observe.
- **`ied` removes ambient state.** An `ied` run must be reproducible — the
  same work-directory state and script, the same result — so it has no
  `os` table (`os.clock`/`os.date`/`os.time`/`os.difftime`, `os.getenv`) and
  no `math.random`/`math.randomseed`: both hand a script input the caller
  never gave it. They are *absent* rather than stubbed, so `if os then` is
  a usable feature test. The rest of `math` is untouched, and
  [`task.wait`](modules/task.md) is how you sleep where `task` exists. See
  [Reproducibility](binaries/ied.md#reproducibility).
- **Luau sandbox mode is enabled** (an actual Luau feature, not an ito
  invention) before your script runs: the standard library and every
  ito module table are read-only, and a top-level assignment like
  `json = nil` writes into a private per-script shadow of the global
  environment rather than corrupting the real global for anyone else (a
  nested `require`d module, or a later script in the same process).
  Attempting to mutate a module table directly — `fs.write = nil` — is
  a runtime error.

## Method naming: `with_*` builders

The one naming convention ito adds on top of ordinary Luau style is
`with_*` for fluent builders (`hcl.builder()`, `j2.engine()`, …) —
see [Method naming conventions](conventions.md).
