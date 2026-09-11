# `io` — standard streams

| Name | Effect |
| --- | --- |
| `io.stdout`, `io.stderr`, `io.stdin` | file-like handles for the standard streams |
| `handle:write(...)` | write strings/numbers to `io.stdout`/`io.stderr`; returns the handle |
| `handle:flush()` | accepted; every write is already flushed |
| `handle:close()` | always raises on a standard handle |
| `handle:setvbuf(mode)` | accepted, does nothing |
| `io.stdin:read([fmt])` | read from stdin |
| `io.stdin:lines()` | iterate stdin lines |
| `io.write(...)` / `io.read([fmt])` / `io.lines()` / `io.flush()` | the same calls on the default handles |

Not available in `ied`, which has no `io` global (`if io then` is a feature
test; stdin and stderr are ambient state, see
[reproducibility](../binaries/ied.md#reproducibility)). `print` is its whole
output surface there.

## Example

```lua
io.stderr:write("warning: ", ("disk almost full"):yellow(), "\n")
io.stdout:write("a", 1, 2.5):write("\n")
for line in io.lines() do
    print(line:upper())
end
```

## Conventions

**Vanilla surface, narrowed.** The handles follow Luau's file-handle
conventions, but only for the standard streams. There is no `io.open` (use
[`fs`](fs.md)), no path form of `io.lines`, and no `io.output`/`io.input`
(use a `proc` redirect). There is no `io.ewrite`, `io.ewriteln` or
`io.writeln`: write to stderr with `io.stderr:write(...)`, and add the
`"\n"` yourself.

**Writes are unfiltered and unbuffered.** Writes go straight to the stream
with no level, target or `RUST_LOG`/`ITO_LOG` filtering, and ANSI codes are
left alone, so `io.stderr:write(("x"):red())` reaches the terminal styled.
That is the difference from [`log.*`](log.md). Every write is flushed
immediately and pauses live [`ui.progress`](ui-progress.md) bars so it does
not garble them. `io.stdout` and `print` share one sink and interleave in
call order.

**Closed pipes.** If the reader goes away (`ito s.luau | head`), the next
write ends the run quietly with exit code `141` (128 + SIGPIPE, as for any
process killed by it) instead of raising: the script did not run to the end.
Outstanding tasks still drain. The error cannot be caught with `pcall`; the
run ends with `141` regardless.

**Not affected by flags.** There is no `-q` for `ito`; nothing here is
rerouted to the log. (`-q` exists only in `ied`, where it silences `print`.)

`io.read()` with no argument reads one line (without its terminator), as
in vanilla. To read everything that is left, use `io.read("a")`.

## Handles

`io.stdout`, `io.stderr` and `io.stdin` are userdata that print as
`file (io.stdout)` and so on. Using a handle in the wrong direction raises
(`io.stdin:write(...)`, `io.stdout:read()`, `io.stdout:lines()`).

### `handle:write`

Write to a handle.

**Signature:** `handle:write(...) -> handle`

**Parameters:** any number of strings or numbers. Numbers are rendered with
`tostring`; strings are written byte for byte (need not be UTF-8). Nothing
is added between arguments and no newline is appended.

**Returns:** the handle itself, so calls chain.

**Errors:** raises on any other argument type (`nil`, booleans, tables;
use `print` or `tostring` for those), when called on `io.stdin`, and on a
write error other than a closed pipe.

```lua
io.stdout:write("progress: "):write(50, "%\n")
```

### `handle:flush`

**Signature:** `handle:flush() -> handle`

A no-op returning the handle, since every write is already flushed.

### `handle:close`

**Signature:** `handle:close()`

**Errors:** always raises (`cannot close standard file io.<name>`).

### `handle:setvbuf`

**Signature:** `handle:setvbuf(mode, [size]) -> true`

Accepted for compatibility; does nothing.

### `io.stdin:read`

Read from stdin.

**Signature:** `io.stdin:read([fmt]) -> string?`

**Parameters:**
- `fmt` — what to read; default `"l"`. A leading `*` is accepted
  (`"*a"`).
  - `"l"` — one line, without its `\n` (or `\r\n`).
  - `"L"` — one line, keeping its terminator.
  - `"a"` — everything that is left.
  - a non-negative number `n` — at most `n` bytes. `0` returns `""` if
    input remains and `nil` at end of input.

**Returns:** the data read, or `nil` at end of input. The exception is
`"a"`, which returns `""` at end of input. Data is not interpreted as text,
so it is safe for binary data.

**Errors:** raises on an invalid format or a read error; raises on
`io.stdout`/`io.stderr`.

```lua
local doc = json.parse(io.stdin:read("a"))
```

### `io.stdin:lines`

**Signature:** `io.stdin:lines() -> (() -> string?)`

**Returns:** an iterator yielding each remaining line without its
terminator, then `nil` at end of input.

**Errors:** raises on `io.stdout`/`io.stderr`.

## Default-handle functions

### `io.write`

**Signature:** `io.write(...) -> io.stdout`

Same as `io.stdout:write(...)`, including the argument rules and the
returned handle.

### `io.read`

**Signature:** `io.read([fmt]) -> string?`

Same as `io.stdin:read([fmt])`.

```lua
local name = io.read() or "world"
local rest = io.read("a")
```

### `io.lines`

**Signature:** `io.lines() -> (() -> string?)`

Same as `io.stdin:lines()`. Takes no path; use [`fs`](fs.md) to read files.

```lua
for line in io.lines() do
    log.info("got", line)
end
```

### `io.flush`

**Signature:** `io.flush()`

A no-op; every write is already flushed.
