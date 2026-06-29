# `time` — timestamps and deltas

Two representations, both ordinary Luau numbers, so `<` compares and `+`
arithmetic works without unwrapping anything and a value survives a trip
through [`json`](json.md):

- a **timestamp** is Unix seconds — what `os.time()` returns, fractional.
- a **delta** is a number of seconds. As *output* it is signed (subtraction
  can go negative); as *input* the string grammar reads magnitudes only.

Every function that takes a timestamp or a delta accepts *both spellings* —
the number, or a string. That is what makes `time.add(t, "1h")` and
`time.add(t, 3600)` the same call, and lets `time.diff`'s two arguments come
from different sources.

## Summary

Pure value conversions — no clock, no host zone, available in `iq`/`ied`/`ito`/`irun`:

| Function | Effect |
| --- | --- |
| `time.parse_rfc3339(<str>)` | RFC 3339 only; a UTC offset is required. |
| `time.parse_rfc2822(<str>)` | RFC 2822 only — email `Date:` and HTTP `Date`/`Last-Modified`/`Expires`. |
| `time.parse_strftime(<str> <pattern>)` | Exactly `<pattern>`; UTC unless the pattern itself fixes an offset. |
| `time.format(<t>[ <pattern>])` | RFC 3339 in UTC, or a strftime pattern over UTC fields. |
| `time.format_offset(<t> <offset>[ <pattern>])` | As `time.format`, in a fixed offset (`"+05:30"`, `"Z"`) — DST-blind. |
| `time.format_tz(<t> <zone>[ <pattern>])` | As `time.format`, in a named IANA zone (`"Europe/Berlin"`) — DST-correct. |
| `time.parse_tz(<str> <zone>)` | A wall-clock reading *in* `<zone>` → Unix seconds. |
| `time.zone_offset(<t> <zone>)` | The offset `<zone>` is on at `<t>`, as `"+02:00"`. |
| `time.format_rfc2822(<t>)` | RFC 2822 in UTC — `"Sat, 13 Jul 2024 15:09:59 -0000"`. |
| `time.format_imf(<t>)` | IMF-fixdate for HTTP headers — `"Sat, 13 Jul 2024 15:09:59 GMT"`. |
| `time.delta(<value>)` | A duration string, or a number already in seconds, → seconds. |
| `time.format_delta(<value>)` | The inverse — a delta → `"4d 2h 5m 2s"`. |
| `time.round(<secs> <unit>)` | Truncate a delta toward zero at `"s"`/`"m"`/`"h"`/`"d"`/`"w"`; one return value. |
| `time.round_with_rest(<secs> <unit>)` | As `time.round`, plus the remainder below the unit; two return values. |
| `time.diff(<a> <b>)` | `b - a`, in seconds. |
| `time.add(<t> <delta>)` / `time.sub(<t> <delta>)` | Shift a timestamp; a calendar-unit delta (`"P1M"`) anchors at `<t>`. |

Ambient-state functions that may read the host's clock or local UTC offset —
`ito`/`irun` only:

| Function | Effect |
| --- | --- |
| `time.now()` | Unix seconds, fractional — the current instant. |
| `time.parse(<value>)` | Any shape the module understands → Unix seconds. Offset-less input is read as **local** time. |
| `time.format_local(<t>[ <pattern>])` | As `time.format`, in the host's current offset. |

`time.diff`/`time.add`/`time.sub` are also **rebound** under `ito`/`irun`
(same names, same behavior for offset-bearing input) so that an offset-less
string argument is completed with the local offset instead of being
rejected — see "Pure vs. host-only" below.

## Example

```lua
-- works everywhere: RFC 3339 requires an offset, so no host state is needed
local built = time.parse_rfc3339("2026-08-10T14:00:00+02:00")
print(time.format(built))                       --> "2026-08-10T12:00:00Z"
print(time.format_tz(built, "Europe/Berlin"))   --> "2026-08-10T14:00:00+02:00"

-- ito/irun only: guesses local time for offset-less input
if time.parse then
    local at = time.parse("2026-08-10 12:00:00")
    print(time.format_delta(time.diff(at, time.now())))
end
```

## Conventions

**Pure vs. host-only.** `time` is registered in two halves. The pure half
(`register`) is every value conversion where the answer follows from its
arguments alone — RFC 3339/2822 require a zone in the input itself, and a
named-zone function (`format_tz`/`parse_tz`/`zone_offset`) takes the zone as
an *argument* resolved against the tzdb **bundled into the binary**, not the
host's configured zone. All of that is identical on every machine and every
run, so it ships in `iq`/`ied`/`ito`/`irun` alike. The host-only half
(`register_host_time`) is only what may consult the running machine: the
clock (`time.now`) and the local UTC *offset* (`time.parse` on an
offset-less string, `time.format_local`). Because that offset also
completes an offset-less argument to `add`/`sub`/`diff`, those three names
are **rebound** under `ito`/`irun` to versions that accept such input — the
pure versions (still reachable as-is under `iq`/`ied`) reject it outright.
Feature-test with `if time.parse then … else … end`; there is no stub that
raises "host only" at call time, the function is simply absent.

**Offset vs. named zone.** A fixed offset (`"+05:30"`, `"Z"`) is DST-blind —
whoever hardcodes `"+02:00"` for Berlin is wrong half the year. A named IANA
zone (`"Europe/Berlin"`) carries DST rules, so the offset it renders depends
on the date. `format_offset`/`format_tz` and their sibling functions each
**reject** the other's argument shape (an offset string where a zone is
expected, or vice versa) rather than silently answering a different
question. Both also reject a value that looks like an offset or zone where
a strftime *pattern* argument belongs — a pattern with no `%` would
otherwise silently return itself as literal text.

**Deltas funnel through one coercion.** `time.delta` is the single
implementation every delta-taking function calls — `add`, `sub`, `round`,
`round_with_rest`, `format_delta`, and (outside this module) `task.wait`. A
*number* means seconds already; a *string* is read by humantime's grammar
(`"1h30m"`, `"4d2h5m2s"`, spaces optional, units largest-first). A bare
numeric string (`"1200"`) is rejected — the grammar demands a unit, and a
caller who means seconds can pass the number `1200` directly. Deltas are
**unsigned as input** (`"-90s"` does not parse); `format_delta` may still
print a leading `-` because subtraction is not required to stay
non-negative. `time.delta` rejects month/year units (`calendar_unit`) since
neither has a fixed length in seconds; `time.add`/`time.sub` accept them
because they anchor at a real timestamp instead.

**Rounding returns one value on purpose.** `time.round` truncates toward
zero and returns a single number so it composes directly into
`format_delta`. `time.round_with_rest` is the two-value sibling — kept
separate because a trailing multi-value call silently expands into the next
call's *second argument*, and `format_delta(round(d, "h"))` must not do
that by accident.

**No `since`/`until`, no ISO-8601 delta spelling.** An age is
`time.diff(t, time.now())`, written out at the call site rather than hidden
behind a name whose sign convention has to be recalled. There is no
`parse_delta`/ISO-8601 duration reader for deltas (`add`/`sub` do accept
ISO 8601 spans like `"P1M"` via jiff's `Span` grammar, since those anchor at
a timestamp) — don't add either.

## `time.parse_rfc3339`

Parses an RFC 3339 timestamp; the UTC offset is mandatory.

**Signature:** `time.parse_rfc3339(text: string) -> number`

**Parameters:**
- `text` — an RFC 3339 date-time, e.g. `"2026-08-10T12:00:00Z"` or
  `"2026-08-10T14:00:00+02:00"`.

**Returns:** Unix seconds (fractional if the input carried sub-second
precision).

**Errors:** raises if `text` has no full date-time, or has one but no
offset (`"2026-08-10T12:00:00"` is rejected — RFC 3339 requires a zone).

```lua
time.parse_rfc3339("2026-08-10T12:00:00Z")        -- 1786363200
time.parse_rfc3339("2026-08-10T14:00:00+02:00")   -- 1786363200, same instant
```

## `time.parse_rfc2822`

Parses the RFC 2822 date-time spelling used by email `Date:` headers and,
in its obsolete-zone form, by HTTP headers.

**Signature:** `time.parse_rfc2822(text: string) -> number`

**Parameters:**
- `text` — e.g. `"Thu, 29 Feb 2024 05:34:45 -0500"` or
  `"Sat, 13 Jul 2024 15:09:59 GMT"`. Accepts the obsolete named zones
  (`GMT`, `UT`, the single-letter military zones), which is what lets this
  same function read an HTTP `Date`/`Last-Modified`/`Expires` header as well
  as a mail header.

**Returns:** Unix seconds.

**Errors:** raises if `text` is not RFC 2822 — an RFC 3339 string
(`"2026-08-10T12:00:00Z"`) is rejected here, since `time.parse` deliberately
does not sniff between the two formats.

```lua
local at = time.parse_rfc2822("Sat, 13 Jul 2024 15:09:59 GMT")
time.format(at)   --> "2024-07-13T15:09:59Z"
```

## `time.parse_strftime`

Parses `text` against exactly the given strftime pattern.

**Signature:** `time.parse_strftime(text: string, pattern: string) -> number`

**Parameters:**
- `text` — the string to parse.
- `pattern` — a strftime pattern, e.g. `"%d.%m.%Y"`.

**Returns:** Unix seconds. An offset in the parsed result wins; otherwise
the reading is UTC; a pattern yielding a date with no time component reads
as midnight.

**Errors:** raises if `text` does not match `pattern`, or if the match
yields no date at all.

```lua
time.parse_strftime("10.08.2026", "%d.%m.%Y")   -- midnight UTC, 1786320000
```

## `time.format`

Renders a timestamp as RFC 3339 in UTC, or through a strftime pattern over
UTC fields.

**Signature:** `time.format(t: number|string, pattern?: string) -> string`

**Parameters:**
- `t` — a timestamp: Unix seconds, or any string `time.parse_rfc3339`
  and friends accept for a timestamp with an explicit offset.
- `pattern` — optional strftime pattern. Omitted: RFC 3339 (`"...Z"`).

**Returns:** the formatted string.

**Errors:** raises if `t` cannot be read as a timestamp; raises if
`pattern` looks like a fixed offset or a named zone (use `format_offset`/
`format_tz` instead) or if the pattern is otherwise invalid.

```lua
time.format(1786363200)                 --> "2026-08-10T12:00:00Z"
time.format(1786363200, "%Y-%m-%d")     --> "2026-08-10"
```

## `time.format_offset`

Renders a timestamp in a fixed UTC offset. DST-blind — the caller asserts
the offset.

**Signature:** `time.format_offset(t: number|string, offset: string, pattern?: string) -> string`

**Parameters:**
- `t` — a timestamp.
- `offset` — a fixed offset like `"+05:30"`, `"-08:00"`, or `"Z"`/`"UTC"`.
  Not a named zone — use `format_tz` for that.
- `pattern` — optional strftime pattern; defaults to RFC 3339 in that
  offset.

**Returns:** the formatted string.

**Errors:** raises if `offset` is not a fixed-offset spelling (e.g. passing
a zone name like `"Europe/Berlin"` here raises, pointing at `format_tz`);
raises if `pattern` looks like an offset/zone rather than a strftime
pattern.

```lua
time.format_offset(1786363200, "+05:30")   --> "2026-08-10T17:30:00+05:30"
time.format_offset(1786363200, "Z")        --> "2026-08-10T12:00:00Z"
```

## `time.format_tz`

Renders a timestamp in a named IANA zone, DST-correct for that instant's
date.

**Signature:** `time.format_tz(t: number|string, zone: string, pattern?: string) -> string`

**Parameters:**
- `t` — a timestamp.
- `zone` — an IANA zone name, e.g. `"Europe/Berlin"`. Resolved against the
  **bundled** tzdb, so the answer is identical on every machine.
- `pattern` — optional strftime pattern; defaults to RFC 3339 in that
  zone's offset at `t`.

**Returns:** the formatted string.

**Errors:** raises if `zone` is a fixed-offset spelling instead of a zone
name (points at `format_offset`); raises if `zone` is not a known IANA
name; raises if `pattern` looks like an offset/zone.

```lua
time.format_tz(summer_t, "Europe/Berlin")   --> "2026-08-10T14:00:00+02:00"
time.format_tz(winter_t, "Europe/Berlin")   --> "2026-01-10T13:00:00+01:00"
```

## `time.parse_tz`

Reads a wall-clock string as a reading *in* the given zone — the inverse of
`format_tz`.

**Signature:** `time.parse_tz(text: string, zone: string) -> number`

**Parameters:**
- `text` — a date-time or bare date, e.g. `"2026-08-10T14:00:00"` or
  `"2026-08-10"`. An input that already carries its own offset keeps it —
  the zone argument does not re-interpret an instant that is already named.
- `zone` — an IANA zone name.

**Returns:** Unix seconds.

**Errors:** raises if `zone` is unknown; raises if `text` is neither a
date nor a date-time.

**DST edges:** resolved rather than raised (jiff's "compatible" strategy).
On the repeated hour of a fall-back transition, the reading takes the
**first** occurrence; a time that never happened (a spring-forward gap)
shifts **forward** past the gap.

```lua
time.parse_tz("2026-08-10T14:00:00", "Europe/Berlin")   -- 1786363200

-- 02:30 happens twice on this date; this is the earlier one
time.parse_tz("2026-10-25T02:30:00", "Europe/Berlin")
-- formats back as 02:30+02:00

-- 02:30 does not exist on this date; shifts forward
time.parse_tz("2026-03-29T02:30:00", "Europe/Berlin")
-- formats back as 03:30+02:00
```

## `time.zone_offset`

Reports the UTC offset a named zone is on at a given instant.

**Signature:** `time.zone_offset(t: number|string, zone: string) -> string`

**Parameters:**
- `t` — a timestamp.
- `zone` — an IANA zone name.

**Returns:** the offset as `"+02:00"` (always `+HH:MM`/`-HH:MM`, not jiff's
shorter `+02`) — the same spelling `format_offset` accepts as its `offset`
argument.

**Errors:** raises if `zone` is unknown or is a fixed-offset spelling.

```lua
time.zone_offset(summer_t, "Europe/Berlin")   --> "+02:00"
time.zone_offset(winter_t, "Europe/Berlin")   --> "+01:00"

-- feeds straight back into format_offset
time.format_offset(t, time.zone_offset(t, "Europe/Berlin"))
```

## `time.format_rfc2822`

Renders a timestamp as RFC 2822, in UTC — the inverse of `parse_rfc2822`.

**Signature:** `time.format_rfc2822(t: number|string) -> string`

**Parameters:**
- `t` — a timestamp.

**Returns:** e.g. `"Sat, 13 Jul 2024 15:09:59 -0000"`. The offset is always
`-0000`, not `+0000` — RFC 2822 §3.3 gives `-0000` the meaning "this is UTC
and the sender's own zone is unknown", which is exactly true here: the
original offset was absorbed into the instant and is not recoverable.

**Errors:** raises if `t` cannot be read as a timestamp.

```lua
time.format_rfc2822(1786363200)   --> "Mon, 10 Aug 2026 12:00:00 -0000"
```

## `time.format_imf`

Renders a timestamp as HTTP's IMF-fixdate (RFC 9110) — always `GMT`,
always a two-digit day, always 29 characters.

**Signature:** `time.format_imf(t: number|string) -> string`

**Parameters:**
- `t` — a timestamp.

**Returns:** e.g. `"Sat, 13 Jul 2024 15:09:59 GMT"` — what an HTTP `Date`,
`Expires`, or `If-Modified-Since` header is required to contain. There is
no matching `parse_imf`: IMF-fixdate is a subset of what `parse_rfc2822`
already reads, so the narrowing only matters going out.

**Errors:** raises if `t` cannot be read as a timestamp.

```lua
time.format_imf(1786363200)   --> "Mon, 10 Aug 2026 12:00:00 GMT"
```

## `time.delta`

Reads a delta from a number (seconds) or a duration string.

**Signature:** `time.delta(value: number|string) -> number`

**Parameters:**
- `value` — a number of seconds, or a duration string like `"30s"`,
  `"1h 30min"`, `"4d2h5m"` (humantime grammar; spaces optional, units
  largest-first).

**Returns:** the delta in seconds.

**Errors:** raises if `value` is a bare numeric string with no unit
(`"1200"` — write `"1200s"` or pass the number `1200`); raises on a month
or year unit (`"1month"`, `"1y"` — no fixed length in seconds; use
`time.add`/`time.sub`, which anchor at a timestamp); raises on any other
unparseable string; raises if a numeric `value` is non-finite.

```lua
time.delta("1h 30min")   -- 5400
time.delta("1h30m")      -- 5400
time.delta("4d2h5m2s")   -- 353102
time.delta(5400)         -- 5400
time.delta("1d")         -- 86400, a day is 24h here
time.delta("1w")         -- 604800
time.delta("1200")       -- error: needs a unit
time.delta("1month")     -- error: use time.add(t, "1month")
```

## `time.format_delta`

The inverse of `time.delta` — the only delta-printing spelling in this
module.

**Signature:** `time.format_delta(value: number|string) -> string`

**Parameters:**
- `value` — anything `time.delta` accepts (so formatting a duration string
  normalises it).

**Returns:** e.g. `"4d 2h 5m 2s"`. Any sub-second fraction is dropped
(the second is this module's display resolution). A negative value (from
subtraction, not from input) prints with a leading `-`. All-zero prints as
`"0s"`.

**Errors:** same as `time.delta`.

```lua
time.format_delta(5400)      --> "1h 30m"
time.format_delta(353102)    --> "4d 2h 5m 2s"
time.format_delta("90m")     --> "1h 30m"
time.format_delta(-90)       --> "-1m 30s"
time.format_delta(0.5)       --> "0s"
```

## `time.round`

Truncates a delta toward zero at a unit, returning one value.

**Signature:** `time.round(value: number|string, unit: string) -> number`

**Parameters:**
- `value` — anything `time.delta` accepts.
- `unit` — one of `"s"`, `"m"`, `"h"`, `"d"`, `"w"` (long spellings —
  `"seconds"`, `"minutes"`, etc. — also accepted). No finer unit than
  second (nothing displays sub-second); no coarser than week (a month/year
  has no fixed length).

**Returns:** the delta truncated toward zero at `unit`, in seconds — still
a delta, so it composes into `format_delta`, `diff`, or further arithmetic.

**Errors:** raises on an unknown unit (including `"month"`/`"y"`); raises
on an invalid `value` per `time.delta`.

```lua
time.round(90, "m")     -- 60
time.round(119, "m")    -- 60, toward zero not to-nearest
time.round(-90, "m")    -- -60, symmetric across the sign

local age = time.diff(t, time.now())
time.format_delta(time.round(age, "m"))   -- e.g. "7d 5h 44m"
```

## `time.round_with_rest`

As `time.round`, but also returns what was truncated off.

**Signature:** `time.round_with_rest(value: number|string, unit: string) -> number, number`

**Parameters:** same as `time.round`.

**Returns:** two numbers — the rounded value, then the remainder below
`unit`. Both are whole seconds, so `rounded + remainder` equals the input
delta truncated to seconds (not the original fractional delta). Kept
separate from `time.round` because a trailing multi-value call expands into
the next call's argument list — `format_delta(round(d, "h"))` would
otherwise silently receive the remainder as its own second argument.

**Errors:** same as `time.round`.

```lua
local hours, rest = time.round_with_rest(age, "h")
print(time.format_delta(hours) .. " and " .. math.floor(rest / 60) .. "m")

time.round_with_rest(90, "m")   -- 60, 30
```

## `time.diff`

The difference between two timestamps.

**Signature:** `time.diff(a: number|string, b: number|string) -> number`

**Parameters:**
- `a`, `b` — timestamps, either spelling; need not match each other's
  shape.

**Returns:** `b - a`, in seconds (a delta, possibly negative).

**Errors:** raises if either side cannot be read as a timestamp. Under the
pure registration, an offset-less string on either side raises (no zone to
place it in); under `ito`/`irun` this name is **rebound** to complete such
input with the host's local offset instead.

```lua
time.diff("2026-08-10T12:00:00Z", "2026-08-10T13:30:00Z")   -- 5400
time.diff(t, t + 90)                                        -- 90

-- ito/irun only: an age is diff against now, not a dedicated since()
time.diff(t, time.now())
```

## `time.add` / `time.sub`

Shift a timestamp by a delta. Calendar units (`"P1M"`, `"P1Y"`) anchor at
the timestamp, so they resolve calendar-correctly rather than through a
fixed-length approximation.

**Signature:**
`time.add(t: number|string, delta: number|string) -> number`
`time.sub(t: number|string, delta: number|string) -> number`

**Parameters:**
- `t` — a timestamp to shift from.
- `delta` — a number of seconds, a humantime-style string (`"1h30m"`), or
  (unlike `time.delta`) an ISO 8601 span with calendar units (`"P1M"`,
  `"P1Y"`, jiff's `Span` grammar) — accepted here specifically because `t`
  gives them something to anchor to.

**Returns:** the shifted timestamp, in Unix seconds. `sub` negates the
delta before applying it.

**Errors:** raises if `t` cannot be read as a timestamp; raises if `delta`
is not a valid number, duration string, or span; raises if the shift is
out of range. Under the pure registration, an offset-less string `t`
raises; under `ito`/`irun` these names are **rebound** to complete such
input with the host's local offset.

```lua
-- January 31 + one month is the end of February, not 30 days later
time.format(time.add(time.parse_rfc3339("2026-01-31T00:00:00Z"), "P1M"))
-- "2026-02-28T00:00:00Z"

time.sub(time.add(1786363200, "1h"), "1h")   -- 1786363200, inverses
```

## `time.now`

The current instant. Host-only (`ito`/`irun`).

**Signature:** `time.now() -> number`

**Returns:** Unix seconds, fractional.

**Errors:** does not exist outside `ito`/`irun` — `time.now` is `nil`
there; feature-test with `if time.now then`.

```lua
local age = time.diff(started_at, time.now())
```

## `time.parse`

The do-as-I-intend parser: reads any shape this module understands. Host-only
(`ito`/`irun`).

**Signature:** `time.parse(value: number|string) -> number`

**Parameters:**
- `value` — a timestamp already in Unix seconds, or a string in any of:
  RFC 3339 with an offset (`"2026-08-10T12:00:00Z"`, and offset variants,
  including the compact `"20260810T120000Z"` and a space instead of `T`),
  an offset-less date-time (`"2026-08-10T12:00:00"`) read as **local**
  time, or a bare date (`"2026-08-10"`) read as **local midnight**. Does
  **not** sniff RFC 2822 — a `Date:`-style header is rejected; use
  `time.parse_rfc2822` explicitly. ISO week dates and ordinal dates are not
  supported.

**Returns:** Unix seconds.

**Errors:** raises if `value` matches none of the above shapes. Absent
(not a stub) outside `ito`/`irun`; feature-test with `if time.parse then
… else time.parse_rfc3339(...) end`.

```lua
time.parse("2026-08-10T12:00:00Z")       -- RFC 3339
time.parse("2026-08-10T14:00:00+02:00")  -- any offset
time.parse("2026-08-10 12:00:00")        -- space instead of T
time.parse("2026-08-10T12:00:00")        -- no offset: local time
time.parse("2026-08-10")                 -- a date: local midnight
time.parse(1786363200)                   -- already a timestamp
```

## `time.format_local`

Renders a timestamp as RFC 3339 (or a strftime pattern) in the host's
current local offset. Host-only (`ito`/`irun`).

**Signature:** `time.format_local(t: number|string, pattern?: string) -> string`

**Parameters:**
- `t` — a timestamp.
- `pattern` — optional strftime pattern; defaults to RFC 3339 in the local
  offset (a real numeric offset, e.g. `"+02:00"`, not `"Z"`, unless the
  host itself is at UTC).

**Returns:** the formatted string.

**Errors:** raises if `t` cannot be read as a timestamp, or if `pattern`
looks like an offset/zone. Absent outside `ito`/`irun`.

```lua
time.format(t)         --> "2026-08-10T12:00:00Z"
time.format_local(t)   --> "2026-08-10T14:00:00+02:00"  -- host in CEST
```

## `os.time`/`os.date` still work

Under `ito`/`irun` the Luau standard library's `os.time`, `os.date`,
`os.clock`, and `os.difftime` remain available and unchanged — `time` is
additive. `time.now()` differs from `os.time()` only in keeping the
fractional part.
