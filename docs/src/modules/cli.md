# cli — command-line interfaces

`argv` hands a script its arguments as a plain list of strings and stops
there. `cli` is the other half: declare what the script accepts, parse it
once, and get `--help` written for you.

| | |
| --- | --- |
| `cli.about(<summary>, [<details>])` | Describe the script, printed above the usage line. Returns nothing. |
| `cli.opt(<spec>, [<description>], [<details>])` | Declare an option. Returns a handle. |
| `cli.arg(<name>, [<description>], [<details>])` | Declare a positional. Returns a handle. |
| `cli.parse()` | Parse the script's `argv` against every declaration made so far. Returns nothing. |
| `cli.assert(condition, [<message>])` | Report a usage error (usage on stderr, exit 2) if `condition` is falsy. Returns nothing. |
| `<handle>()` | Read the parsed value. Raises before `cli.parse()`. |
| `<handle>:get()` | The same read, spelled as a method. |
| `<handle>:count()` | How many times the option/argument appeared. Raises before `cli.parse()`. |

## Example

```lua
cli.about("Copy files to a deployment target.", [[
Reads a manifest, resolves each entry against the work directory,
and uploads what changed. Nothing is written without --commit.]])

local env   = cli.opt("e/env=",    "deployment target")
local dry   = cli.opt("n/dry-run", "report without writing")
local host  = cli.arg("host",      "target machine")
local files = cli.arg("files...",  "files to copy")

function main()
    cli.parse()
    cli.assert(env() ~= "foo", "foo is not a valid deployment target")

    log.info("deploying to " .. host())
    if dry() then
        log.info("dry run — nothing will be written")
    end
    for _, f in ipairs(files()) do
        print(f, env())
    end
end
```

```sh
ito deploy.luau :: --env prod -n web1 a.conf b.conf
irun deploy.luau --env prod -n web1 a.conf b.conf   # same, no `::`
```

## General rules

**Declare, parse, read — strictly in that order.** `cli.opt`/`cli.arg`
register into one process-wide registry that `cli.parse` later consumes;
they do not return a value, because none exists yet. They return a
*handle*: an opaque reference into the registry, callable (`env()`) or
readable via `:get()` — both spellings are the same read, behind a
`__call` metamethod and a method respectively. The short form is the one
to reach for; `:get()` stays for when a handle has travelled far enough
from its declaration that a bare call would not read as one. Reading a
handle (either spelling, and `:count()` too) before `cli.parse()` has run
**raises**, rather than returning a plausible-looking default — a script
that reads that early has a bug, and a zero value would hide it.

Because the registry is global state, not a parser object, there is
exactly one interface per run. That is what lets the handles be
file-level locals while the parse happens inside `main`, as in the
example above. It also means **declarations belong in the entry
script**: a `require`d module's body runs once, so a `cli.opt` at its top
level joins the caller's command line and shows up in the caller's
`--help`. Put such calls inside the module's own `main` (which `require`
never runs) if they must not leak.

**Spec grammar is fish `argparse`'s.** For `cli.opt`, an optional suffix
says how many values the option takes:

| spec | meaning | reading it (`get`/call) | `:count()` |
| --- | --- | --- | --- |
| `v/verbose` | flag | `true`/`false` | occurrences (`-vv` → `2`) |
| `e/env=` | required value | string, `nil` when absent | `0` or `1` |
| `f/file=?` | optional value | string, or `true` if bare, `nil` when absent | `0` or `1` |
| `I/include=+` | repeatable value | list of strings, `{}` when absent | number of elements |

A `/` names the halves explicitly — whatever sits on a side *is* that
half, an empty side is that half omitted (`f/` is short-only, `/force`
is long-only). With **no `/`, length decides**: a one-character name is
short (`f` → `-f` — nobody writes `--f`), anything longer is long
(`force` → `--force`). So `/f` is the one spelling the length default
cannot reach — a one-character *long* option, reachable only by naming it
explicitly. A spec with nothing on either side (`""`, `"/"`) is rejected.
Short flags cluster (`-nvv`); a clustered short option with a value takes
the rest of the cluster as that value (`-eprod`).

**A flag reads as a boolean, on purpose.** Presence is the question
`if force() then` asks, and `0` is truthy in Luau — a flag that returned
its occurrence count would make that `if` always taken. The count is not
lost, just moved: `:count()` is the only way to see `-vv` as `2`. This is
one instance of a uniform rule — `:count()` always means "how many times
did this appear", across every handle kind (occurrences for a flag,
elements for `=+`/`...`, `1`-or-`0` for a single value) — so
`files:count() == 0` and `#files() == 0` can never disagree.

**`cli.parse()` is terminal when it prints.** `--help` (usage to stdout,
exit `0`) and a malformed command line (usage + complaint to stderr, exit
`2`) both leave `cli.parse` through a `CliExit` sentinel that the runner
recognizes and turns into a process exit before returning control to the
script. This is why every binary — including `iq` (whose return value
*is* stdout) and `ied` (which owns its own diff/report output) — can
register `cli.parse` without conflict: the run unwinds out of `eval`
entirely, so it never reaches `emit_returned`/`emit_reports`/
`flush_writes` and cannot interleave with or precede anything they would
have emitted. Nothing is committed either — an `ied` script that calls
`cli.parse()` and then hits a usage error has buffered nothing yet.

**Two kinds of mistake, two audiences.** A malformed spec — a bad
`cli.opt`/`cli.arg` string, a duplicate short/long name, an argument
ordering violation — is the *script's* bug: it raises immediately, as an
ordinary Luau error, from the declaring call, before any parsing happens.
A malformed *command line* — unknown option, missing required value,
missing required argument, leftover positional — is the *user's*: it is
reported through the usage block on stderr and exits `2`, never as a Lua
error.

**Help is answered before positionals are matched.** `cli.parse` scans
options first (settling whether `--help`/`-h` was given) and only after
that matches positionals against what is left — so a script with
*required* arguments can still explain itself when invoked with no
arguments at all, which is exactly the situation someone reaching for
`--help` is in.

**No program name appears in the usage line.** Under a shebang the
binary's own `argv[0]` is the interpreter, not the script, so a name that
would be wrong half the time is omitted entirely; whoever ran the script
knows what they typed.

**`-h` and `--help` render the declarations differently.** `-h` (and a
usage error) print the compact two-column table: label on the left,
one-line `description` on the right. `--help` prints one section per
option/positional instead — the label as a heading, `description` under
it, then a blank line and `details` (if the declaration gave one),
indented below. So `details` is prose that only ever appears on the fuller
page, the same relationship `cli.about`'s `summary`/`details` has, just
per-declaration instead of once for the whole script.

## `cli.about`

Sets the descriptive prose shown above the usage line, in two parts: a
summary that appears on every help surface, and optional details that
only `--help` prints.

**Signature**

```lua
cli.about(summary: string, details: string?)
```

**Parameters**

- `summary` (`string`) — a line or two saying what the script does.
  Shown by `-h`, by `--help`, and above the usage block of a **usage
  error**. Trailing whitespace is trimmed.
- `details` (`string`, optional) — the rest of the prose. Appended
  *below* `summary`, separated by a blank line, and shown by `--help`
  only. Trailing whitespace is trimmed.

`details` never replaces `summary` — `--help` is the summary plus more,
never something else — so the two spellings of help cannot describe the
script differently. A usage error deliberately stops at the summary:
someone who mistyped a flag wants the complaint next to the shape that
corrects it, not a paragraph wedged between them.

| | `summary` | `details` |
| --- | --- | --- |
| `-h` | ✓ | — |
| `--help` | ✓ | ✓ |
| usage error (exit 2) | ✓ | — |

`-h` and `--help` are separate rows internally but print as one
`-h, --help` line. When `details` is set, that line reads "show help (-h
for a summary)"; with no details both spellings print the same page and
it stays "show this help".

The two spellings are auto-registered **independently**, each one unless
the script claimed that exact spelling. So `cli.opt("/help")` takes
`--help` for the script and leaves `-h` printing the help, and
`cli.opt("h/")` does the reverse. Whichever remains automatic reaches the
whole of the prose: with `--help` claimed, `-h` prints the details too
rather than withholding them for a spelling that will never show them,
and its row names only the spelling it owns.

**Return value**

None.

**Error conditions**

- Raises if called after `cli.parse()` ("the usage block is already
  rendered").
- Calling it more than once does **not** raise: the last call wins,
  which is deliberate so a `require`d module setting its own `about`
  cannot defeat the entry script's. Both halves move together — a
  one-argument call clears any `details` an earlier call set, so a
  surviving summary never heads prose written for a different one.

**Example**

```lua
cli.about("Copy files to a deployment target.", [[
Reads a manifest, resolves each entry against the work directory,
and uploads what changed. Nothing is written without --commit.]])
```

## `cli.opt`

Declares one option (short and/or long flag) and returns a handle to read
its parsed value after `cli.parse()`.

**Signature**

```lua
cli.opt(spec: string, description: string?, details: string?) -> handle
```

**Parameters**

- `spec` (`string`) — the option's name(s) and arity, fish-`argparse`
  grammar:
  - Name half: `x/y` names short `x` and long `y` explicitly (either side
    may be empty to omit that half); with no `/`, a one-character name is
    short and a longer one is long.
  - Arity suffix (default: flag, no suffix):
    - *(none)* — flag, no value, repeats counted.
    - `=` — required value.
    - `=?` — optional value (bare presence reads as `true`).
    - `=+` — repeatable, each occurrence supplying one value.
  - Allowed characters: short name must be an ASCII letter or digit; long
    name must be letters, digits, `-` or `_`.
- `description` (`string?`) — one-line help text. Shown on **both** `-h`
  (the `Options:` table column) and `--help` (under this option's own
  heading). Omit for no description.
- `details` (`string?`) — longer prose, shown **only** by `--help`,
  indented below `description` in this option's section. `-h` and a usage
  error never show it — the same summary/details split `cli.about` makes
  for the whole script, here made per option.

**Return value**

A handle (see general rules) — callable / `:get()` / `:count()`.

**Error conditions**

- Raises immediately (Lua error, not exit 2) for:
  - A malformed spec (empty, both halves empty, `=` not at the end, bad
    characters, short half longer than one character where required).
  - A short or long name already declared by a previous `cli.opt`.
  - Called after `cli.parse()` ("declare every option first").

**Example**

```lua
local verbose = cli.opt("v/verbose", "repeat for more detail")
local force   = cli.opt("f")             -- -f            -> true
local dry     = cli.opt("dry-run")       -- --dry-run     -> true
local env     = cli.opt("e/env=", "deployment target")
local file    = cli.opt("f/file=?")      -- --file / --file=x
local include = cli.opt("I/include=+")   -- -I a -I b -> { "a", "b" }
local n = cli.opt("n/not-real", "Checks if the option is real", [[
This is just an example but we could have more text here.]])
```

On `--help`, that last declaration renders as:

```
-n, --not-real
  Checks if the option is real

  This is just an example but we could have more text here.
```

## `cli.arg`

Declares one positional argument and returns a handle to read its parsed
value after `cli.parse()`.

**Signature**

```lua
cli.arg(name: string, description: string?, details: string?) -> handle
```

**Parameters**

- `name` (`string`) — the positional's name, with an optional arity
  suffix:
  - *(none)* — required.
  - `?` — optional (`nil` when absent).
  - `...` — takes every remaining positional (`{}` when none are left);
    must be the last declared positional.
  - Bare name: letters, digits, `-`, `_` only.
- `description` (`string?`) — one-line help text. Shown on **both** `-h`
  (the `Positionals:` table column) and `--help` (under this argument's
  own heading).
- `details` (`string?`) — longer prose, shown **only** by `--help`,
  indented below `description` — see `cli.opt`'s `details`.

**Return value**

A handle (see general rules) — callable / `:get()` / `:count()`.

**Error conditions**

- Raises immediately at the declaring call for:
  - A malformed name (empty, or containing characters other than
    letters/digits/`-`/`_`).
  - A name already declared by a previous `cli.arg`.
  - Declaring anything after a `...` positional ("already takes every
    remaining argument").
  - Declaring a required positional after an optional one ("optional
    arguments must come last") — order is registration order, and
    `<a> [b] <c>` has no unambiguous reading.
  - Called after `cli.parse()` ("declare every argument first").

**Example**

```lua
local host  = cli.arg("host", "target machine")     -- required
local port  = cli.arg("port?")                       -- optional
local files = cli.arg("files...", "files to copy")   -- rest, must be last
```

## `cli.parse`

Consumes every declaration made so far, matches the script's `argv`
against it, and populates every handle's value. Must be called exactly
once, and after every `cli.opt`/`cli.arg` call the script wants included.

**Signature**

```lua
cli.parse()
```

**Parameters**

None.

**Return value**

None, on a successful parse — control returns to the script and every
handle now reads its value.

**Error conditions**

- Raises an ordinary Lua error if called twice ("cli.parse() called
  twice").
- Auto-registers `-h`/`--help` unless the script already declared `-h` or
  `--help` itself (in which case the script owns that spelling as a plain
  flag, with no auto-exit behavior).
- On `--help`/`-h` (when auto-registered): writes the usage block to
  **stdout** and exits with code **0**. This check happens *before*
  positionals are matched, so a script with required arguments still
  answers `--help`.
- On a malformed command line — unknown option, an option requiring a
  value that got none, a missing required positional, or leftover
  positionals — writes `error: <message>` followed by the usage block (the
  `about` text omitted) to **stderr** and exits with code **2**.
- Both exits above are *not* Lua errors the script can catch meaningfully:
  they leave through the `CliExit` sentinel and unwind straight out of
  `eval` (see general rules). Neither returns to the script.

**Example**

```lua
local env = cli.opt("e/env=")
local host = cli.arg("host")

function main()
    cli.parse()   -- unknown flag or missing <host> exits 2 with usage on stderr
                  -- --help exits 0 with usage on stdout
    print(host(), env())
end
```

## `cli.assert`

Reports an invariant that `cli.opt`/`cli.arg` cannot express by themselves —
a value out of range, two options that conflict — the same way a malformed
command line is reported.

**Signature**

```lua
cli.assert(condition: boolean, message: string?)
```

**Parameters**

- `condition` (`boolean`) — already evaluated by the script; `cli.assert`
  does not interpret truthiness itself.
- `message` (`string?`) — shown as `error: <message>` above the usage
  block. Defaults to `"assertion failed"` when omitted.

Meant to run *after* `cli.parse()`, since the usual reason to call it is
checking a parsed value — but nothing enforces that ordering.

**Return value**

None, when `condition` is truthy.

**Error conditions**

- When `condition` is falsy: writes `error: <message>` followed by the
  usage block's summary (the same `Detail::Summary` rendering a malformed
  command line gets — no `about` details, no per-option `details`) to
  **stderr** and exits with code **2**. Like `cli.parse`'s own usage
  errors, this leaves through the `CliExit` sentinel and unwinds straight
  out of `eval`, not as a catchable Lua error.

**Example**

```lua
local env = cli.opt("e/env=")

function main()
    cli.parse()
    cli.assert(env() ~= "foo", "foo is not a valid option value")
end
```

## Handle: call / `:get()`

Reads a declared option's or argument's parsed value.

**Signature**

```lua
handle()       -- via the __call metamethod
handle:get()   -- identical read, method spelling
```

**Parameters**

None.

**Return value** — depends on how the handle was declared:

| declared as | return value |
| --- | --- |
| flag (`v/verbose`) | `true` if it appeared at least once, else `false` |
| required (`e/env=`) | string, or `nil` if absent |
| optional (`f/file=?`) | string if given a value, `true` if given bare, `nil` if absent |
| repeatable (`I/include=+`) | list (table) of strings, `{}` if never given |
| positional, required | string |
| positional, optional (`host?`) | string, or `nil` if absent |
| positional, rest (`files...`) | list (table) of strings, `{}` if none left |

**Error conditions**

- Raises if called before `cli.parse()` has run: `"<name> read before
  cli.parse() -- declare with cli.opt/cli.arg, then call cli.parse()
  before reading"`.

**Example**

```lua
local env = cli.opt("e/env=")
cli.parse()
print(env())      -- e.g. "prod"
print(env:get())  -- same value, method spelling
```

## Handle: `:count()`

Answers "how many times did this appear on the command line" — the
question a boolean flag read cannot answer on its own.

**Signature**

```lua
handle:count() -> number
```

**Parameters**

None.

**Return value** — uniform across every handle kind:

| declared as | `:count()` |
| --- | --- |
| flag | number of occurrences (`-vvv` → `3`) |
| required / optional value | `1` if present, `0` if absent |
| repeatable (`=+`) / rest (`...`) | number of elements collected |

**Error conditions**

- Raises before `cli.parse()` has run, same as the value read.

**Example**

```lua
local verbose = cli.opt("v/verbose", "repeat for more detail")

function main()
    cli.parse()
    if verbose() then log.info("verbose") end            -- -v or more
    if verbose:count() >= 2 then log.debug("very verbose") end  -- -vv or more
end
```

## `--` and other parsing conventions

A bare `--` ends option parsing; every token after it is positional,
however it is spelled — including something that looks exactly like a
declared option:

```sh
irun grep.luau -- --env      # host is not set; the positional is "--env"
```

The `--` token itself is consumed, not passed through. A required value
may be attached inline (`--env=prod`, `-eprod`) or given as the next
token (`--env prod`, `-e prod`); an optional value (`=?`) is only ever
read when attached — bare `--file` never reaches forward for a value.

## `argv` is still there

`cli` is opt-in and takes nothing away: `argv` remains the raw list, and
a script wanting a grammar this module does not cover can ignore `cli`
entirely and read `argv` directly. See [args](../args.md).
