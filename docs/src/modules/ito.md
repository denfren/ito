# `ito` — runtime utilities

| Function | Availability | Effect |
| --- | --- | --- |
| `ito.version_check(req)` | everywhere | Abort if the running `ito` is not semver-compatible with `req`. |
| `ito.slug()` | not in `ied` | A random `"adjective-noun"` name, e.g. `"rusty-nail"`. |

The null sentinel is the global [`null`](table.md#the-global-null); the
colour controls are [`fmt.colors`](fmt.md#fmtcolors) and
[`fmt.is_tty`](fmt.md#fmtis_tty). The import anchors and `proc.pid` live on
their own modules ([`fs`](fs.md) and [`proc`](proc.md) respectively).
`os.getenv` is registered onto the standard **`os`** table, not `ito`
(see [below](#osgetenv)); it is absent under `ied`.

## Example

```lua
ito.version_check("0.7.0")            -- fail fast on an incompatible ito

local settings = json.parse(fs.read("settings.json"))
```

## Conventions

- **Availability.** An `ied` run must be reproducible
  ([why](../binaries/ied.md#reproducibility)), so functions that depend on
  ambient state are **absent** under `ied` rather than stubbed, and
  `if ito.slug then` or `if os.getenv then` is a usable feature test. That
  covers host locations (the import anchors on [`fs`](fs.md), a terminal
  stream) and ambient or random state (`proc.pid`, `ito.slug`, `os.getenv`,
  the clock). `ito.version_check` is pure and present in both binaries.
- **Related globals.** The null sentinel is the global
  [`null`](table.md#the-global-null); the colour switch is
  [`fmt.colors`](fmt.md#fmtcolors), with [`fmt.is_tty`](fmt.md#fmtis_tty)
  as its host-only detection surface.
- **Version compatibility.** `ito.version_check` is the only version
  surface: the running version is not readable as a value, so a script
  asserts what it needs (a semver requirement) rather than branching on
  what it got. Call it near the top of a script so an incompatible `ito`
  fails fast (exit `3`) with a descriptive error instead of misbehaving
  later. Compatibility is **caret** (Cargo's rule): `"1.2"` accepts any
  `1.x` from 1.2; **before 1.0 a change in the minor is incompatible**, so
  `"0.7.0"` accepts `0.7.x` (>= 0.7.0) but not a later minor. The
  requirement must be a *string* — a number such as `0.10` would be read as
  `"0.1"`, so it raises.
- **Script arguments are not part of this module.** Everything after
  the script path on the command line arrives as the global `argv`, not through
  `ito`. See [args](../args.md).

## `ito.version_check`

Abort the script if the running `ito` does not satisfy a semver
requirement.

**Signature:** `ito.version_check(req: string)`

**Parameters:**

- `req` — a version requirement. A bare literal (`<major>.<minor>.<patch>`,
  or a prefix of it) is read with **caret (`^`) semantics**: the leftmost
  non-zero component is the breaking axis, and newer versions within it
  are accepted. `"0.7.0"` accepts any `0.7.x` from 0.7.0 on; `"1.2.3"`
  accepts `>=1.2.3, <2.0.0`. Any string a semver `VersionReq` accepts also works
  verbatim (e.g. `">=0.2, <0.4"`, `"^0.2.1"`). **Must be a string**: a number
  raises, since `0.10` would silently become `"0.1"`. Before 1.0 a minor
  bump is incompatible, so `"0.7.0"` accepts `0.7.x` (>= 0.7.0), not a later
  minor.

**Returns:** none.

**Errors:** raises if `req` is not a valid version requirement,
or if the running version does not satisfy it. The error message names
both the requirement and the running version.

```lua
ito.version_check("0.7.0")   -- this ito, or a compatible 0.7.x
```

## `ito.slug`

A random `"adjective-noun"` name — useful for a scratch
directory, resource, or run name that reads better than a random hex
string.

**Signature:** `ito.slug() -> string`

**Parameters:** none.

**Returns:** a string of the form `"adjective-noun"`, e.g.
`"rusty-nail"`, drawn from the `names` crate's default word lists and
generator. No numbered variant and no uniqueness guarantee: the word lists
give on the order of tens of thousands of combinations, so two calls in one
run rarely but *can* collide — never treat a slug as a lock or an ID.

**Errors:** none.

Not available in `ied`, for the reason `math.random`
is absent there: it's random, so a reproducible run cannot see it. Absent
rather than stubbed, so `if ito.slug then` is a usable feature test.

```lua
local dir = path.join("/tmp", "deploy-" .. ito.slug())
fs.mkdir(dir)
```

## `os.getenv`

Not part of the `ito` table: registered onto
Luau's built-in `os` table instead — restoring the Lua 5.1 name Luau's
sandboxed `os` drops (which otherwise keeps only `clock`/`date`/
`difftime`/`time`).

**Signature:** `os.getenv(name: string) -> string?`

**Parameters:** `name` — the environment variable to read.

**Returns:** the variable's value, `""` if it is set to the empty
string (kept distinguishable from unset), or `nil` if unset or not valid
UTF-8.

**Errors:** none.

Not available in `ied` (the environment is ambient state). Read-only on purpose — there
is no setter, because mutating process-wide environment state would be
visible to every later task; a per-command `proc`/`ssh` `:env()` covers
passing a variable to a child instead.

```lua
local level = os.getenv("LOG_LEVEL") or "info"
```
