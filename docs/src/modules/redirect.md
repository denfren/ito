# redirects — where command output goes

Values accepted by [`proc`](proc.md)'s `:stdout()`/`:stderr()` and, via the
same values, [`ssh`](ssh.md)'s remote command builder. There is no
`redirect` global — these live on the `proc` table because a redirect
target is only ever an argument to those two methods:

| Value | Signature | Meaning |
| --- | --- | --- |
| `proc.NULL` | — | discard the output (the default for both streams) |
| `proc.INHERIT` | — | pass through to this program's stream matching the slot |
| `proc.OUT` | — | pass through to *this* program's stdout, whichever slot |
| `proc.ERR` | — | pass through to *this* program's stderr, whichever slot |
| `proc.STDOUT` | — | bind to whatever `:stdout()` currently points at (shell's `2>&1`) |
| `proc.STDERR` | — | bind to whatever `:stderr()` currently points at (shell's `1>&2`) |
| `proc.file` | `(<path>)` | write to a local file (truncated on first open) |
| `proc.buffer` | `()` | capture, returned as a `buffer` |
| `proc.string` | `()` | capture, returned as a string |

Any other value raises `"expected a redirect target (proc.INHERIT,
proc.OUT, proc.ERR, proc.NULL, proc.STDOUT, proc.STDERR, proc.file(path),
proc.buffer() or proc.string()), got <type>"`.

```lua
proc.command({ "ls" }):stdout(proc.string()):run()
ssh.connect("host"):command({ "ls" }):stdout(proc.string()):run()
```

## Rules

**One spelling, local or remote.** `ssh` accepts the exact same values as
`proc`, through the same conversion — a command built with `:stdout(proc.string())`
behaves identically whether it runs locally or over `ssh.connect(...)`.

**Handles, not strings.** Each value except the six constants is a distinct
handle object, not a plain string, so that passing the *same* handle to both
`:stdout()` and `:stderr()` is detectable and merges the two streams. A
result field is present only for a stream configured with a capture; a
stream left at `proc.NULL` is absent from the result rather than an empty
string.

**Both streams default to `proc.NULL`.** Nothing reaches the terminal
unless asked for, so a script's own output is never mixed with a command's.
Use `proc.INHERIT` to let a command write straight through to the host
program's stdout/stderr.

**Our streams versus the child's.** Two pairs of constants look alike and
answer different questions. `proc.OUT`/`proc.ERR` name one of *this*
program's streams: `:stdout(proc.ERR)` is the shell's `1>&2`, and
`:stderr(proc.OUT)` is `2>&1` onto our stdout. `proc.STDOUT`/`proc.STDERR`
cross-link the *child's* two streams to each other, wherever those point —
`:stderr(proc.STDOUT)` follows stdout into a capture or a file.

**Merging two streams.** `proc.string()`, `proc.buffer()` and `proc.file()`
each yield a *new* handle per call. Passing the same handle to both
`:stdout()` and `:stderr()` merges them so they interleave into one value,
the way they would on a terminal:

```lua
local both = proc.string()
local r = proc.command({ "sh", "-c", "echo out; echo err >&2" })
    :stdout(both)
    :stderr(both)
    :run()
-- r.stdout and r.stderr are the same merged value
```

Two *separate* `proc.file()` handles pointed at the same path is an error
at run time, not silent corruption — each would carry its own write offset
and clobber the file. Reuse one handle instead.

**Aliases bind on assignment, not at run time.** `proc.STDOUT`/`proc.STDERR`
substitute the *other* stream's current target when assigned — matching
the shell, where `cmd >f 2>&1 >/dev/null` still sends stderr to the file:
`:stderr(proc.STDOUT)` captures whatever stdout pointed at *then*, and a
later `:stdout(...)` does not drag stderr along.

**Files bypass `fs`.** Files opened via `proc.file()` are opened directly
against the filesystem, not through the [`fs`](fs.md) VFS — they are never
buffered, previewed or reported as changes.

## `proc.NULL`

Discards the stream. The default for both `:stdout()` and `:stderr()` when
neither is called.

- **Shape:** a constant value, not a function call.
- **Example:**

```lua
-- explicit, though this is already the default
proc.command({ "make" }):stderr(proc.NULL):run()
```

## `proc.INHERIT`

Passes the stream straight through to this program's own *corresponding*
stream — the slot's counterpart, so a child's stdout reaches our stdout
and its stderr our stderr. To cross them, name the stream with
`proc.OUT`/`proc.ERR` below.

- **Shape:** a constant value, not a function call.
- **Example:**

```lua
proc.command({ "make", "install" })
    :stdout(proc.INHERIT)
    :stderr(proc.INHERIT)
    :run()
```

## `proc.OUT` / `proc.ERR`

Passes the stream through to *this* program's stdout (`proc.OUT`) or
stderr (`proc.ERR`), named outright rather than matched to the slot.

- **Shape:** constant values, not function calls.
- **Why both these and `proc.INHERIT`:** inheriting hands the child
  whichever of our descriptors matches the slot it is assigned to, so a
  child's stdout going to *our stderr* had no spelling at all. These name
  the descriptor instead, which is what makes the shell's `1>&2` (and the
  reverse) expressible here. `proc.INHERIT` is unchanged and still means
  "the slot's own counterpart".
- **Error conditions:** raises at spawn time if the named descriptor
  cannot be duplicated.
- **Example:**

```lua
-- the child's stdout onto ito's stderr (the shell's `1>&2`)
proc.command({ "sh", "-c", "echo x" }):stdout(proc.ERR):run()

-- the child's stderr onto ito's stdout (`2>&1`, to our stdout)
proc.command({ "sh", "-c", "echo x >&2" }):stderr(proc.OUT):run()
```

## `proc.STDOUT` / `proc.STDERR`

Aliases the *other* stream's current target — `proc.STDOUT` assigned to
`:stderr()` is the shell's `2>&1`; `proc.STDERR` assigned to `:stdout()` is
`1>&2`. Resolved once, at the point of assignment. These link the
*child's* two streams to each other; to point one at a stream of *ours*,
use `proc.OUT`/`proc.ERR` above.

- **Shape:** constant values, not function calls.
- **Error conditions:** none — assigning either always succeeds; it simply
  captures whatever the aliased stream currently holds (`proc.NULL` if that
  stream hasn't been set yet).
- **Example:**

```lua
-- merge stderr into wherever stdout is currently headed
local r = proc.command({ "sh", "-c", "echo out; echo err >&2" })
    :stdout(proc.string())
    :stderr(proc.STDOUT)
    :run()
print(r.stdout) -- "out\nerr\n"
```

## `proc.file(path)`

Writes the stream to a file on the machine running the script.

- **Signature:** `proc.file(path: string) -> Redirect`
- **Parameters:** `path` — a local filesystem path. Always resolved on the
  machine running the script, even for a remote command over `ssh`: the
  target is a file descriptor here, so a remote command's output travels
  back over the connection to be written locally. To write on the far side
  instead, redirect inside the remote command itself.
- **Returns:** a new handle. The underlying file is created (truncating any
  existing content) on first use, not when `proc.file()` is called; reusing
  one handle across `:stdout()` and `:stderr()` shares the same file
  descriptor, so interleaved writes land through one offset instead of two
  independent ones.
- **Error conditions:** raises at run time if the file cannot be created
  (permissions, missing directory, etc.), or if two *different* `proc.file()`
  handles target the same path on one command.
- **Example:**

```lua
proc.command({ "ls", "-la" })
    :stdout(proc.file("listing.txt"))
    :run()
```

```lua
-- write on the far side instead of streaming back over the connection
ssh.connect("host"):command({ "sh", "-c", "cmd > /remote/path" }):run()
```

## `proc.buffer()`

Captures the stream, returned from `:run()` as a Luau `buffer`.

- **Signature:** `proc.buffer() -> Redirect`
- **Parameters:** none.
- **Returns:** a new capture handle. The result field is a `buffer`
  containing the raw bytes, with no UTF-8 validation — the honest choice
  when output may be binary.
- **Error conditions:** none.
- **Example:**

```lua
local r = proc.command({ "curl", "-s", "https://example.com/logo.png" })
    :stdout(proc.buffer())
    :run()
-- r.stdout is a `buffer`
```

## `proc.string()`

Captures the stream, returned from `:run()` as a Luau string.

- **Signature:** `proc.string() -> Redirect`
- **Parameters:** none.
- **Returns:** a new capture handle. The result field is a string; still
  8-bit clean, so binary output survives intact even though it prints as a
  string.
- **Error conditions:** none.
- **Example:**

```lua
local r = proc.command({ "git", "rev-parse", "HEAD" })
    :stdout(proc.string())
    :run()
print(r.stdout)
```
