# require

Split a script across files with `require`. A module is just another
`.lua`/`.luau` file; call it and use whatever it returns.

```lua
local m = require("./lib")   -- loads lib.lua next to this script
print(m.hello())
```

A module is a normal Luau chunk: whatever it `return`s is what `require`
hands back (a table of functions, in the usual case). A module without a
`return` yields `nil`.

A path must start with `./`, `../` or `@` — a bare `require("lib")` is
rejected; write `require("./lib")`.

## Aliases

An `@name` prefix looks `name` up in the `aliases` table of the nearest
`.luaurc`, so a module deep in a tree can name a shared directory without
counting `../` hops:

```json
{ "aliases": { "lib": "./lib", "helper": "./lib/helper" } }
```

```lua
local h = require("@lib/helper")   -- ./lib/helper
local h = require("@helper")       -- same file, alias names it directly
```

An alias target is relative to the `.luaurc` that defines it, and may name
either a directory (with a path after it) or a module file (used on its
own).

Two limits keep the confinement guarantee below intact:

- **Aliases cannot escape.** An alias target outside the root is an
  error, exactly like a `../` that climbs too far — writing a `.luaurc` is
  not a way to widen the sandbox.
- **Only configs inside the root are read.** The search for `.luaurc`
  stops at the root instead of continuing to the filesystem root, so a
  config in a parent directory (or `$HOME`, or `/tmp`) cannot redirect a
  confined script's imports. Upstream Luau inherits alias maps all the way
  up; `ito` deliberately does not.

## `@self`

Inside a `pkg/init.lua`, a relative path is resolved from `pkg`'s
*parent* — the module stands in for the directory itself, so `./foo`
means the `foo` next to `pkg`, not inside it. `@self` is the way to name
the directory the module stands in for:

```lua
-- in pkg/init.lua
local outer = require("./foo")       -- foo.lua beside pkg/
local inner = require("@self/foo")   -- pkg/foo.lua
```

This only makes sense for an `init` module. From a plain `sub/a.lua`,
`@self` names `sub/a` — the module's own path — so `@self/helper` looks
for `sub/a/helper.lua` and fails.

## Confined to the script's directory

`require` in `ito` is **confined to the directory tree of the entry
script** — its *root*. A `../`-relative path may walk *within* that tree,
but never above the root: an escaping `require` is an error, not a read.

| Source | Root (import boundary) |
| --- | --- |
| `ito <path>` | the directory containing `<path>`, symlinks resolved |
| `ito -s`/`--script` (inline) | none — `require` is disabled unless `-r <dir>` |
| `ito -` (stdin) | none — `require` is disabled unless `-r <dir>` |

The path is resolved through symlinks first, so the root is the directory
holding the *real* file. A script installed as `~/.local/bin/<name>`
pointing at `~/scripts/<name>.luau` imports from `~/scripts/`, where its
modules are — not from the bin directory, where nothing it needs is.

A script read from stdin or `-s` has no file, so there is no directory to
confine it to. Rather than fall back to wherever you happen to be
standing, imports are refused outright until `-r <dir>` names a root
explicitly. `-r` is rejected alongside a `SCRIPT` file, which already has
one.

This boundary is **independent of the `fs`/`-C` root**, which constrains
file *operations* (`fs.read`/`fs.write`), not imports. So

```console
$ ito /tmp/foo.lua -C /media/one
```

confines `require` to `/tmp` (where `foo.lua` lives) while `fs` reads and
writes go to `/media/one`.

## Where paths resolve

A `require` path is resolved **relative to the directory of the script
that calls `require`** — not the working directory, and not the entry
script's directory.

The `.lua`/`.luau` extension is implied: `require("./lib")` loads
`lib.lua` or `lib.luau`. Relative paths (`require("./sub/lib")`,
`require("../shared")`) are resolved against the calling file's directory,
so long as they stay within the root.

So a nested script requires the module next to *itself*, even if a
same-named module also sits at the root:

```text
proj/
  lib.lua         -- return { hello = function() return "root" end }
  sub/
    lib.lua       -- return { hello = function() return "sub" end }
    s.lua         -- local m = require("./lib"); m.hello() == "sub"
```

```console
$ ito -C proj sub/s.lua
sub
```

Under `ito`/`irun`, both anchors are readable as values:
`ito.require_root()` is the root, `ito.require_dir()` the directory the
calling chunk resolves against — so a module can locate a data file the
way it locates a module. See [`ito`](../modules/ito.md#import-anchors).

## Nested requires

When a required module *itself* calls `require`, the inner path resolves
relative to **that module's** directory — the requiring file — not the
entry script's directory. Each hop is independent (but still bounded by
the entry script's root).

```text
proj/
  main.lua        -- require("./sub/a")
  b.lua           -- val() = "root-b"   (decoy, never picked)
  sub/
    a.lua         -- require("./b")     ->  resolves to proj/sub/b.lua
    b.lua         -- val() = "sub-b"
```

```console
$ ito -C proj main.lua
sub-b
```

`sub/a.lua` lives in `proj/sub/`, so its `require("./b")` finds
`proj/sub/b.lua`, never the `proj/b.lua` at the entry script's level.

## Resolution order

For a path segment `foo`, candidates are tried in this order:

1. if `foo` already ends in `.lua` or `.luau`, only that exact file;
2. otherwise `foo.luau`, then `foo.lua`;
3. fallback: a directory module `foo/init.luau`, then `foo/init.lua`.

A directory with an `init.lua`/`init.luau` file is itself a requireable
module — `require("./pkg")` runs `pkg/init.lua`.

**All** candidates are checked. If more than one matches, `require`
errors and names the conflicting files (e.g. both `foo.luau` and
`foo/init.lua` exist). If none match, `require` errors and lists the
paths it checked.

## Module caching

A module is compiled and run **once** per `ito` invocation; requiring the
same path again (from the same or a different requirer) returns the same
cached result rather than re-running the file.
