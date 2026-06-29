# `ui.progress` — progress bars and spinners

**Host-only (`ito`/`irun`).** `ui` names ambient terminal state — a
progress bar draws to stderr — that a query's return value or an edit
preview must not depend on, so it is absent rather than stubbed under
`iq`/`ied`: `if ui then` is a usable feature test.

`ui.progress.bar(len[, msg])` and `ui.progress.spinner([msg])` return a
**handle**, not a one-shot call — a progress indicator is inherently
updated over time, across loop iterations or from several `task`s sharing
one bar. A handle can safely be shared across tasks. Both draw to
**stderr** by default, which keeps a bar from interleaving with a script's
own `print` (stdout) or a piped `iq`-style return value.

Every bar and spinner transparently joins one shared multi-bar display,
created on first use — several concurrent indicators (e.g. one per
`task.pool` worker) stack as separate terminal lines instead of
overwriting each other. A single bar draws exactly as it always did; the
difference only shows once a second one is live at the same time. A
finisher (`finish`/`finish_and_clear`/`abandon`) leaves that shared state;
`ui.progress.clear()` drops it outright for a clean slate between
unrelated phases, and `ui.progress.suspend(fn)` hides every live bar, runs
`fn`, then redraws them — for output (a prompt, a printed block) that must
not interleave with the bars.

```lua
local bar = ui.progress.bar(#hosts, "deploying")
for _, h in ipairs(hosts) do
    deploy(h)
    bar:inc(1)
end
bar:finish("done")
```

| Call | Effect |
| --- | --- |
| `ui.progress.bar(len[, msg])` | a bar handle with a default template |
| `ui.progress.spinner([msg])` | a spinner handle, self-animating |
| `ui.progress.style()` | a style builder, see [below](#uiprogressstyle) |
| `ui.progress.clear()` | drop the shared multi-bar state, for a fresh start |
| `ui.progress.suspend(fn)` | hide every bar, run `fn`, redraw |

## Handle methods

| Method | Effect |
| --- | --- |
| `h:style(style)` | apply a [`ui.progress.style()`](#uiprogressstyle) builder |
| `h:inc(n)` / `h:dec(n)` | advance / move back the position by `n` |
| `h:set(n)` | set the position to `n` |
| `h:set_length(n)` | change the total |
| `h:inc_length(n)` / `h:dec_length(n)` | grow / shrink the total in place |
| `h:set_message(s)` | replace the trailing status text |
| `h:set_prefix(s)` | replace the leading `{prefix}` text |
| `h:tick()` | advance a spinner's animation frame by hand |
| `h:enable_steady_tick(secs)` | animate automatically every `secs` |
| `h:disable_steady_tick()` | stop that animation |
| `h:position()` / `h:length()` | current position / total (`length` may be `nil`) |
| `h:elapsed()` / `h:eta()` | seconds elapsed / estimated seconds remaining |
| `h:per_sec()` | current rate, items per second |
| `h:is_finished()` | whether a terminal state (`finish`/`abandon`) was reached |
| `h:println(s)` | print a line above the bar without disturbing it |
| `h:reset()` | clear position, elapsed time and ETA |
| `h:finish([msg])` | stop, keep the bar on screen, optionally set `msg` first |
| `h:finish_and_clear()` | stop and erase the line |
| `h:abandon([msg])` | stop, keep on screen, marked incomplete rather than done |

```lua
local bar = ui.progress.bar(#hosts, "checking hosts")
for _, h in ipairs(hosts) do
    probe(h)
    bar:inc(1)
end
bar:finish("done")

local spin = ui.progress.spinner("waiting for lock")
while not acquired() do
    task.wait("200ms")
end
spin:finish_and_clear()
```

**A spinner keeps animating on its own** even while the script is doing
something else — a `task.wait` loop between manual `:tick()` calls would
otherwise freeze its frame for the whole wait; `enable_steady_tick` (on by
default for `ui.progress.spinner`) avoids that.

## Multiple bars and suspending the display

Concurrent bars stack automatically:

```lua
local pool = task.pool(4)
for _, h in ipairs(hosts) do
    pool:spawn(function()
        local bar = ui.progress.bar(100, h)
        -- ... advance bar as work on h progresses ...
        bar:finish_and_clear()
    end)
end
pool:await_all()
```

To show a prompt or print output without it interleaving with active bars,
wrap it in `ui.progress.suspend`:

```lua
ui.progress.suspend(function()
    print("pausing for input")
    ui.prompt.confirm("Continue?")
end)
```

`suspend` clears every live bar's lines, runs the function, then redraws
them — safe to nest, and the redraw still happens even if the function
raises. If no bar/spinner exists yet, the function just runs directly.

## `ui.progress.style`

A builder for a bar or spinner's layout, applied to a handle with
`h:style(...)`.

| Method | Effect |
| --- | --- |
| `s:template(str)` | the layout — placeholders like `{bar}`, `{wide_bar}`, `{msg}`, `{prefix}`, `{pos}`, `{len}`, `{percent}`, `{elapsed}`, `{eta}`, `{spinner}`, … |
| `s:tick_chars(str)` | one string, one character per spinner frame plus a final frame |
| `s:tick_strings(list)` | the same, as separate strings — for multi-byte glyphs |
| `s:progress_chars(str)` | three characters: filled, current, unfilled |

A **malformed** template (a stray `{`/`}`, an unterminated brace) raises,
naming the syntax problem. An **unrecognized placeholder** does not error —
it renders empty rather than being checked against the known set, so a
typo'd `{elapsd}` silently vanishes rather than failing loudly; check the
rendered output.

```lua
local style = ui.progress.style()
    :template("{spinner} {msg} [{wide_bar}] {percent}%")
    :tick_strings({ "◐", "◓", "◑", "◒", "" })
    :progress_chars("=> ")

local bar = ui.progress.bar(100)
bar:style(style)
```
