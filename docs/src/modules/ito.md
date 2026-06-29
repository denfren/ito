# `ito` — runtime utilities

| Function | Everywhere / host-only | Effect |
| --- | --- | --- |
| `ito.version_check(req)` | everywhere | Abort if the running `ito` is not semver-compatible with `req`. |
| `ito.null` | everywhere | The sentinel value for an explicit null from `json`/`toml`/`yaml`/`hcl`. |
| `ito.is_null(value)` | everywhere | `true` iff `value` is that sentinel. |
| `ito.colors([on])` | everywhere | Set/read whether ANSI styling emits codes; returns the resolved state. |
| `ito.is_tty([stream])` | host-only (`ito`/`irun`) | Whether `"stdout"` (default), `"stderr"` or `"stdin"` is a terminal. |
| `ito.require_root()` | host-only (`ito`/`irun`) | The directory imports are confined to, absolute. |
| `ito.require_dir()` | host-only (`ito`/`irun`) | The directory the *calling* chunk's relative imports resolve against, absolute. |
| `ito.pid()` | host-only (`ito`/`irun`) | The running process's own pid, as a number. |
| `ito.slug()` | host-only (`ito`/`irun`) | A random `"adjective-noun"` name, e.g. `"rusty-nail"`. |

Also under `ito`/`irun`, but registered onto **`os`**, not `ito`: `os.getenv`
(see [below](#osgetenv-host-only)).

## Example

```lua
ito.version_check("0.4.0")            -- fail fast on an incompatible ito

local v = json.parse('{"a":null,"b":1}')
if ito.is_null(v.a) then
    v.a = 0
end

ito.colors(ito.is_tty("stderr"))      -- follow the log stream
local shared = path.join(ito.require_root(), "lib/shared.yaml")
```

## Conventions

- **Host-only vs everywhere.** `iq`/`ied` script against a rooted virtual
  filesystem; naming a host location (an import directory, a terminal
  stream) would leak something outside what that `fs` can act on. Such
  functions are simply **absent** on those binaries' `ito` table rather
  than stubbed, so `if ito.require_dir then` / `if ito.is_tty then` are
  usable feature tests. `ito.colors`, `ito.null`/`ito.is_null` and
  `ito.version_check` are pure and present on all four binaries.
- **Colour auto-detection.** `ito.colors(nil)` (including the default,
  unset state) resolves to *auto*. Under `ito`/`irun`, auto detects via
  the CLICOLORS convention (<https://bixense.com/clicolors/>):
  `CLICOLOR_FORCE` (unless `"0"`) forces on; else `NO_COLOR` set forces
  off; else `CLICOLOR=0` forces off; else it follows whether stdout is a
  terminal. Under `iq`/`ied`, auto always resolves to **off** — those
  binaries must not depend on ambient state (`iq`'s output is its return
  value; an `ied --diff` preview must not disagree with the `--write`
  that follows it) — so a script there opts in explicitly with
  `ito.colors(true)`.
- **Version compatibility.** `ito.version_check` is the only version
  surface: the running version is not readable as a value, so a script
  asserts what it needs (a semver requirement) rather than branching on
  what it got. Call it near the top of a script so an incompatible `ito`
  fails fast (exit `3`) with a descriptive error instead of misbehaving
  later.
- **Script arguments are not part of this module.** Everything after
  `::` on the command line arrives as the global `argv`, not through
  `ito`. See [args](../args.md).

## `ito.version_check`

Abort the script if the running `ito` does not satisfy a semver
requirement.

**Signature:** `ito.version_check(req: string)`

**Parameters:**

- `req` — a version requirement. A bare literal (`<major>.<minor>.<patch>`,
  or a prefix of it) is read with **caret (`^`) semantics**: the leftmost
  non-zero component is the breaking axis, and newer versions within it
  are accepted. `"0.2.1"` accepts `>=0.2.1, <0.3.0`; `"1.2.3"` accepts
  `>=1.2.3, <2.0.0`. Any string a semver `VersionReq` accepts also works
  verbatim (e.g. `">=0.2, <0.4"`, `"^0.2.1"`).

**Return value:** none.

**Error conditions:** raises if `req` is not a valid version requirement,
or if the running version does not satisfy it. The error message names
both the requirement and the running version.

```lua
ito.version_check("0.4.0")
```

## `ito.null` / `ito.is_null`

The shared null sentinel and its test, used because a Luau table cannot
store `nil` as a *value* — assigning `t.k = nil` deletes the key, so a
parsed null from JSON/YAML/HCL (TOML has no null in the format) must be
something other than plain `nil` to keep the key present. `json.parse`/
`yaml.parse`/`hcl.parse` produce `ito.null` for it; `json.null` is the
identical value. See [`json`](json.md#null) for the full picture.

**Signature:** `ito.null` (value), `ito.is_null(value: any) -> boolean`

**Parameters (`is_null`):** `value` — anything.

**Return value:** `ito.is_null` returns `true` iff `value` is `ito.null`.

**Error conditions:** none.

```lua
local v = json.parse('{"a":null,"b":1}')
ito.is_null(v.a)              -- true
v.a == nil                    -- false: it's a value, not an absent key

json.to_string({a = ito.null, b = 1})   -- '{"a":null,"b":1}'
```

## `ito.colors`

Set or read the process-wide switch that decides whether the
[`string`](string.md#terminal-styling-ansi) styling methods emit ANSI
codes.

**Signature:** `ito.colors(on: boolean|nil?) -> boolean`

**Parameters:**

- `on` — `true` forces styling on, `false` forces it off, `nil` resets to
  *auto* (see [Conventions](#conventions)). Omitted (no argument at all)
  leaves the switch untouched and only reads it — distinct from passing
  `nil`, which is why this is a variadic function rather than one taking
  `Option<bool>`.

**Return value:** the resolved boolean — "will my styles show up" —
never the three-state setting itself.

**Error conditions:** raises if given a non-boolean, non-nil argument.

```lua
ito.colors(true)                       -- force on regardless of the pipe
ito.colors(ito.is_tty("stderr"))       -- follow the log stream instead
if ito.colors() then ... end           -- read the resolved answer, unchanged
ito.colors(nil)                        -- back to auto
```

## `ito.is_tty`

Host-only. Report whether a given standard stream is a terminal —
the input to colour auto-detection, and useful on its own for deciding
whether to draw progress output.

**Signature:** `ito.is_tty(stream: string?) -> boolean`

**Parameters:**

- `stream` — one of `"stdout"` (default), `"stderr"` or `"stdin"`.

**Return value:** `true` if that stream is a terminal.

**Error conditions:** raises if `stream` is not one of the three
accepted names.

**Availability:** only under `ito`/`irun` — it reports ambient state,
alongside `os.getenv` and `time.now`. Absent under `iq`/`ied`, so
`if ito.is_tty then` is a usable feature test.

```lua
if ito.is_tty() then
    print("interactive")
end
ito.colors(ito.is_tty("stderr"))
```

## `ito.require_root` / `ito.require_dir`

The two anchors [`require`](../luau/require.md) itself resolves against,
handed to the script so it can resolve its own paths the same way: one
script wants a path relative to the whole import tree, another wants one
relative to the file asking.

**Signatures:**

- `ito.require_root() -> string?`
- `ito.require_dir() -> string?`

**Parameters:** none.

**Return value:**

- `require_root()` — the absolute directory imports are confined to (the
  `-r`/entry-script root).
- `require_dir()` — the absolute directory the *calling* chunk's own
  relative imports resolve from. Answers for **whichever chunk called
  it**: the same call gives a different answer in `main.luau` than in a
  module it `require`d, because Luau names a filesystem-backed chunk
  `@<abs path minus extension>` and this is that path's parent. That is
  why both are functions, not constants — only `require_root` could have
  been one.
- Both return `nil` where there is no directory to name: a `-s` or stdin
  script run without `-r` has no import root and no imports; a call from
  a host-spawned context with no Luau frame above it also yields `nil`
  for `require_dir`.

**Error conditions:** none (absence is `nil`, not an error).

**Availability:** only under `ito`/`irun` — the same line
[`fs.cwd()`](fs.md) draws: they report a location on the host
filesystem, which is a script's business only where its `fs` is the host
filesystem too. Under the rooted `fs` of `iq`/`ied` the import root is a
boundary rather than a location, so these are absent rather than
stubbed, and `if ito.require_dir then` is a usable feature test.

```lua
-- in <root>/sub/lib.luau, required by <root>/main.luau:
ito.require_root()   -- /proj      -- the tree imports are confined to
ito.require_dir()    -- /proj/sub  -- where `require("./x")` resolves from

local shared = path.join(ito.require_root(), "lib/shared.yaml")
local mine   = path.join(ito.require_dir(), "fixtures/a.json")
```

## `ito.pid`

Host-only. The process id of the running `ito`/`irun` process itself —
useful for naming a temp or lock file uniquely per run, or for logging a
number an operator can then signal or trace.

**Signature:** `ito.pid() -> number`

**Parameters:** none.

**Return value:** the process's own pid, as a number
(`std::process::id()`).

**Error conditions:** none.

**Availability:** only under `ito`/`irun`, for the reason `os.getenv` is:
a pid is ambient state the caller never passed, and it differs between
two runs of the same script — exactly what an `ied --diff` preview must
not read, since a report embedding one could not match the `--write`
after it. Absent rather than stubbed under `iq`/`ied`, so `if ito.pid
then` is a usable feature test. A function rather than a constant, like
`require_root`/`require_dir`, so the whole host-only half of the table
reads as one thing.

```lua
local lock = path.join("/tmp", "deploy." .. ito.pid() .. ".lock")
fs.write(lock, "")

log.info("running as pid " .. ito.pid())
```

## `ito.slug`

Host-only. A random `"adjective-noun"` name — useful for a scratch
directory, resource, or run name that reads better than a random hex
string.

**Signature:** `ito.slug() -> string`

**Parameters:** none.

**Return value:** a string of the form `"adjective-noun"`, e.g.
`"rusty-nail"`, drawn from the `names` crate's default word lists and
generator (the same one `smic`'s `gen_random_tmp` uses). No numbered
variant and no uniqueness guarantee — two calls may collide.

**Error conditions:** none.

**Availability:** only under `ito`/`irun`, for the reason `math.random`
is dropped from the sandbox everywhere else: it's random, so a script
whose output must be reproducible (`iq`, or an `ied` preview) cannot see
it. Absent rather than stubbed under `iq`/`ied`, so `if ito.slug then` is
a usable feature test.

```lua
local dir = path.join("/tmp", "deploy-" .. ito.slug())
fs.mkdir(dir)
```

## `os.getenv` (host-only)

Not part of the `ito` table, but registered alongside `ito.require_root`/
`ito.require_dir`/`ito.is_tty`/`ito.pid` under the same host-only umbrella, onto
Luau's built-in `os` table instead — restoring the Lua 5.1 name Luau's
sandboxed `os` drops (which otherwise keeps only `clock`/`date`/
`difftime`/`time`).

**Signature:** `os.getenv(name: string) -> string?`

**Parameters:** `name` — the environment variable to read.

**Return value:** the variable's value, `""` if it is set to the empty
string (kept distinguishable from unset), or `nil` if unset or not valid
UTF-8.

**Error conditions:** none.

**Availability:** only under `ito`/`irun`. Read-only on purpose — there
is no setter, because mutating process-wide environment state would be
visible to every later task; a per-command `proc`/`ssh` `:env()` covers
passing a variable to a child instead.

```lua
local level = os.getenv("LOG_LEVEL") or "info"
```
