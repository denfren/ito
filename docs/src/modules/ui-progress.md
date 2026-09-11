# `ui.progress` — progress bars and spinners

| Function | Effect |
| --- | --- |
| `ui.progress.bar(len[, msg])` | a bar handle |
| `ui.progress.spinner([msg])` | a self-animating spinner handle |
| `ui.progress.style()` | a style builder |
| `ui.progress.clear()` | erase every bar from the display |
| `ui.progress.suspend(fn)` | hide the bars while `fn` runs |

Not available in `ied`: a progress bar draws to the terminal, ambient state an
`ied` run must not depend on ([reproducibility](../binaries/ied.md#reproducibility)).
`ui` is absent rather than stubbed, so `if ui then` is a usable feature test.

## Example

```lua
local bar = ui.progress.bar(#hosts, "deploying")
for _, h in ipairs(hosts) do
    deploy(h)
    bar:inc()
end
bar:finish("done")
```

## Conventions

- **Handles.** `bar`/`spinner` return a handle, updated over time; it can be
  shared across `task`s. Bars and spinners redraw on their own every 100 ms
  (see `enable_steady_tick`).
- **Stderr, and no terminal.** Bars draw to stderr, so they never mix with
  `print` output. Without a terminal nothing is drawn;
  `h:println(s)` and the message of `h:finish(msg)`/`h:abandon(msg)` are
  then written to stderr as plain lines so they are not lost.
- **Shared display.** Every bar joins one multi-bar display, so concurrent
  bars (one per `task.pool` worker) stack on separate lines. A finished bar
  keeps its last frame on screen; it is dropped from the display once every
  bar above it has finished as well.
- **Other output stays tidy.** `print`, `io.*`, log records and prompts pause
  the bars while they write, so they do not garble them.
- **Colour.** Default layouts are coloured only while `fmt.colors()` is on (auto detects against stderr, where bars draw).
- **Errors.** When the script ends — or fails — still-running bars are
  abandoned, so nothing keeps animating after an error.
- **Panics become errors.** Invalid styles and intervals raise.

## `ui.progress.bar`

**Signature:** `ui.progress.bar(len: integer[, msg: string]) -> handle`

**Parameters:**
- `len` — the total (a non-negative integer).
- `msg` — text for the `{msg}` slot; default empty.

**Returns:** a handle with the default layout
`{msg} [{bar}] {pos}/{len} ({eta})`.

## `ui.progress.spinner`

**Signature:** `ui.progress.spinner([msg: string]) -> handle`

**Returns:** a handle with the layout `{spinner} {msg}`. It has no length.

```lua
local spin = ui.progress.spinner("waiting for lock")
while not acquired() do
    task.wait("200ms")
end
spin:finish_and_clear()
```

## `ui.progress.clear`

Erase every bar (finished ones too) from the display. Live bars draw
themselves again on their next update.

**Signature:** `ui.progress.clear()`

## `ui.progress.suspend`

Run `fn` with the display hidden, then redraw. For output that must not
interleave with the bars (a prompt, a printed block).

**Signature:** `ui.progress.suspend(fn: function)`

**Parameters:**
- `fn` — called with no arguments; it may `await` (`task.wait`, prompts, …),
  touch bars, and call `suspend` again. The bars are redrawn afterwards, also
  if `fn` raises. `fn`'s error propagates.

```lua
ui.progress.suspend(function()
    if not ui.prompt.confirm("Continue?") then
        error("aborted")
    end
end)
```

## Handle methods

| Method | Effect |
| --- | --- |
| `h:style(style)` | apply a [style](#uiprogressstyle) |
| `h:inc([n])` / `h:dec([n])` | advance / move back the position by `n` (default 1) |
| `h:set(n)` | set the position |
| `h:set_length(n)` | change the total |
| `h:inc_length(n)` / `h:dec_length(n)` | grow / shrink the total |
| `h:set_message(s)` | replace `{msg}` |
| `h:set_prefix(s)` | replace `{prefix}` |
| `h:tick()` | advance the animation one frame |
| `h:enable_steady_tick(every)` | redraw automatically every `every` (on by default, 100 ms) |
| `h:disable_steady_tick()` | stop that |
| `h:position()` / `h:length()` | current position / total (`nil` for a spinner) |
| `h:elapsed()` / `h:eta()` | seconds elapsed / estimated seconds left |
| `h:per_sec()` | current rate, items per second |
| `h:is_finished()` | whether `finish`/`abandon`/`finish_and_clear` ran |
| `h:println(s)` | print a line above the bar |
| `h:reset()` | clear position, elapsed time and ETA |
| `h:finish([msg])` | stop, keep the bar, optionally setting `msg` first |
| `h:finish_and_clear()` | stop and erase the line |
| `h:abandon([msg])` | stop, keep the bar, marked incomplete |

Counts (`n`, `len`) are non-negative integers. `every` takes the delta
grammar of [`time`](time.md): seconds as a number, or a string such as
`"250ms"` or `"2s"`; it must be greater than zero, otherwise the call raises.

```lua
local bar = ui.progress.bar(100, "copy")
bar:enable_steady_tick("50ms")
bar:set(40)
bar:println("halfway soon")
bar:abandon("stopped at 40")
```

## `ui.progress.style`

A builder for the layout of a bar or spinner; apply it with `h:style(...)`.
It is a set of overrides over the handle's own default layout, so
`:progress_chars()` alone keeps `{msg}` and a spinner stays a spinner. Each
method returns the builder.

| Method | Effect |
| --- | --- |
| `s:template(str)` | the layout: `{bar}`, `{wide_bar}`, `{msg}`, `{prefix}`, `{pos}`, `{len}`, `{percent}`, `{elapsed}`, `{eta}`, `{spinner}`, … |
| `s:tick_chars(str)` | spinner frames, one character each (at least 2) |
| `s:tick_strings(list)` | spinner frames as separate strings (at least 2) |
| `s:progress_chars(str)` | the filled, current and unfilled characters (at least 2, equal width) |

**Errors.** A malformed template (a stray or unterminated brace) raises when
`:template()` is called, as do too few tick frames or progress characters. An
unrecognised placeholder does not raise: it renders empty, so a typo such as
`{elapsd}` silently vanishes.

```lua
local style = ui.progress.style()
    :template("{spinner} {msg} [{wide_bar}] {percent}%")
    :tick_strings({ "◐", "◓", "◑", "◒", "" })
    :progress_chars("=> ")
local bar = ui.progress.bar(100)
bar:style(style)
```

Custom computed template keys (`with_key`) are not available: they would call
back into Luau from the drawing thread. Update text with `set_message`/
`set_prefix` instead.

## Concurrent bars

```lua
local pool = task.pool(4)
for _, h in ipairs(hosts) do
    pool:spawn(function()
        local bar = ui.progress.bar(100, h)
        -- ... advance bar as work on h progresses ...
        bar:finish_and_clear()
    end)
end
while not pool:done() do
    pool:await()
end
```
