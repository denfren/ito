# redirects — where command output goes

Values accepted by [`proc`](proc.md)'s `:stdout()`/`:stderr()` and, via the
same values, [`ssh`](ssh.md)'s remote command builder. There is no
`redirect` global — these live on the `proc` table because a redirect
target is only ever an argument to those two methods:

| Value | Meaning |
| --- | --- |
| `proc.NULL` | discard the output (the default for both streams) |
| `proc.INHERIT` | pass through to this program's stream matching the slot |
| `proc.OUT` | pass through to *this* program's stdout, whichever slot |
| `proc.ERR` | pass through to *this* program's stderr, whichever slot |
| `proc.STDOUT` | on `:stderr()`: bind to whatever `:stdout()` currently points at (shell's `2>&1`) |
| `proc.STDERR` | on `:stdout()`: bind to whatever `:stderr()` currently points at (shell's `1>&2`) |
| `proc.file(<path>, [<options>])` | a local file: written (truncated on first open, or appended with `{append = true}`); read as a `:stdin()` source |
| `proc.buffer()` | capture, returned as a `buffer` |
| `proc.string()` | capture, returned as a string |

Any other value raises `"expected a redirect target (proc.INHERIT,
proc.OUT, proc.ERR, proc.NULL, proc.STDOUT, proc.STDERR, proc.file(path),
proc.buffer() or proc.string()), got <type>"` — including a userdata of
some other kind.

`:stdin()` accepts its own set: `proc.NULL`, `proc.INHERIT`,
`proc.file(path)` (read), or a Luau string or buffer fed to the command and
then closed. It raises `"expected a stdin source"` for any other redirect.

```lua
proc.command({ "ls" }):stdout(proc.string()):run()
ssh.connect("host"):command({ "ls" }):stdout(proc.string()):run()
```

## Conventions

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

Two *separate* `proc.file()` handles pointed at the same file is an error
at run time, not silent corruption — each would carry its own write offset
and clobber the file. "The same file" means the same path or, once both
exist, the same device and inode (a symlink, `a/../f`). Reuse one handle
instead.

**Aliases bind on assignment, not at run time.** `proc.STDOUT`/`proc.STDERR`
substitute the *other* stream's current target when assigned — matching
the shell, where `cmd >f 2>&1 >/dev/null` still sends stderr to the file:
`:stderr(proc.STDOUT)` captures whatever stdout pointed at *then*, and a
later `:stdout(...)` does not drag stderr along. Hence the order: stdout
first, then `:stderr(proc.STDOUT)`; the reverse aliases a not-yet-configured
stream (`proc.NULL`). An alias onto a capture makes it a shared handle,
so the result carries *both* `r.stdout` and `r.stderr`, holding the same
merged text. An alias onto a plain `proc.INHERIT` binds to
the concrete stream it means (`:stdout(proc.INHERIT):stderr(proc.STDOUT)`
sends stderr to *our stdout*).

**What the result contains.** A result has a `stdout`/`stderr` field only
for a stream whose target was `proc.buffer()`/`proc.string()`; both fields
hold the same value for a shared handle. Files, NULL, inherit targets and
aliases onto them leave the field absent.

**Merged captures interleave.** A shared capture handle funnels both
streams into one pipe for `proc` and for `ssh` alike, so the text arrives in
the order the command wrote it.

**Files bypass `fs`.** Files opened via `proc.file()` are opened directly
against the filesystem, not through [`fs`](fs.md) — they are never
buffered, previewed or reported as changes.

## `proc.NULL`

Discards the stream. The default for both `:stdout()` and `:stderr()` when
neither is called.

A constant value, not a function call.

```lua
-- explicit, though this is already the default
proc.command({ "make" }):stderr(proc.NULL):run()
```

## `proc.INHERIT`

Passes the stream straight through to this program's own *corresponding*
stream — the slot's counterpart, so a child's stdout reaches our stdout
and its stderr our stderr. To cross them, name the stream with
`proc.OUT`/`proc.ERR` below. As a `:stdin()` source it hands the child this
program's stdin. Anything this program has printed is flushed before the
child starts, so the two outputs do not reorder.

A constant value, not a function call.

```lua
proc.command({ "make", "install" })
    :stdout(proc.INHERIT)
    :stderr(proc.INHERIT)
    :run()
```

## `proc.OUT` / `proc.ERR`

Passes the stream through to *this* program's stdout (`proc.OUT`) or
stderr (`proc.ERR`), named outright rather than matched to the slot.

Constant values, not function calls.

**Why both these and `proc.INHERIT`:** inheriting hands the child
whichever of our descriptors matches the slot it is assigned to, so a
child's stdout going to *our stderr* had no spelling at all. These name
the descriptor instead, which is what makes the shell's `1>&2` (and the
reverse) expressible here. `proc.INHERIT` is unchanged and still means
"the slot's own counterpart".

**Errors:** raises at spawn time if the named descriptor cannot be
duplicated.

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
use `proc.OUT`/`proc.ERR` above. They are two distinct values so a mistake
can be caught.

Constant values, not function calls.

**Errors:** raises when a stream is aliased onto itself
  (`:stdout(proc.STDOUT)`, `:stderr(proc.STDERR)`). Otherwise assigning
  either succeeds; it captures whatever the other stream currently holds
  (`proc.NULL` if that stream hasn't been set yet).

```lua
-- merge stderr into wherever stdout is currently headed
local r = proc.command({ "sh", "-c", "echo out; echo err >&2" })
    :stdout(proc.string())
    :stderr(proc.STDOUT)
    :run()
print(r.stdout) -- "out\nerr\n"
```

## `proc.file`

Writes the stream to a file on the machine running the script, or (for
`:stdin()`) reads one.

**Signature:** `proc.file(path: string, options: { append: boolean? }?) -> Redirect`

**Parameters:**
- `path` — a local filesystem path. Always resolved on the machine running
  the script, even for a remote command over `ssh`: the target is a file
  descriptor here, so a remote command's output travels back over the
  connection to be written locally. To write on the far side instead,
  redirect inside the remote command itself. A relative path is relative to
  this process's working directory, **not** a command's `:current_dir()`.
- `options.append` — open with `O_APPEND` instead of truncating.

**Returns:** a new handle. The underlying file is created (truncating any
existing content, unless appending) on first use, not when `proc.file()` is
called, and the descriptor is then kept until the handle is collected. So
reusing one handle across `:stdout()` and `:stderr()` shares the same
descriptor and interleaved writes land through one offset instead of two
independent ones — and reusing it across several *commands* continues at
that offset (an append in effect, not a fresh truncate); make a new handle
to truncate again. As a `:stdin()` source the file is opened read-only at
spawn and never created.

**Errors:** raises at run time if the file cannot be opened (permissions,
missing directory, etc.), or if two *different* `proc.file()` handles target
the same file on one command. All of a command's targets are opened before
it is spawned, but a program that then fails to launch has already truncated
a file target.

```lua
proc.command({ "ls", "-la" })
    :stdout(proc.file("listing.txt"))
    :run()
```

```lua
-- write on the far side instead of streaming back over the connection
ssh.connect("host"):command({ "sh", "-c", "cmd > /remote/path" }):run()
```

## `proc.buffer`

Captures the stream, returned from `:run()` as a Luau `buffer`.

**Signature:** `proc.buffer() -> Redirect`

**Parameters:** none.

**Returns:** a new capture handle. The result field is a `buffer` containing
the raw bytes, with no UTF-8 validation — the honest choice when output may
be binary.

**Errors:** none.

```lua
local r = proc.command({ "curl", "-s", "https://example.com/logo.png" })
    :stdout(proc.buffer())
    :run()
-- r.stdout is a `buffer`
```

## `proc.string`

Captures the stream, returned from `:run()` as a Luau string.

**Signature:** `proc.string() -> Redirect`

**Parameters:** none.

**Returns:** a new capture handle. The result field is a string; still
8-bit clean, so binary output survives intact even though it prints as a
string.

**Errors:** none.

```lua
local r = proc.command({ "git", "rev-parse", "HEAD" })
    :stdout(proc.string())
    :run()
print(r.stdout)
```
