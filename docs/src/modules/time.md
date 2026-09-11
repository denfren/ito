# `time` — timestamps and deltas

Two representations, both ordinary Luau numbers, so `<` compares and `+`
arithmetic works without unwrapping anything and a value survives a trip
through [`json`](json.md):

- a **timestamp** is Unix seconds — what `os.time()` returns, with a
  fractional part when the source had one.
- a **delta** is a number of seconds. As *output* it is signed (subtraction
  can go negative); as *input* the string grammar reads magnitudes only.

## Naming

Every parser and formatter names its **format** first, then — where the
format leaves the zone open — how the zone is resolved:

- no zone suffix: the format carries its own zone (RFC 3339's offset, RFC
  2822's zone letters/offset). Nothing to resolve, nothing to choose.
- `_utc`: the value carries **no** zone; read/write it in UTC.
- `_local`: the value carries **no** zone; read/write it in the host's
  time zone, with that zone's DST rules at the instant in question.
  Not available in `ied`: the host zone is ambient state
  ([reproducibility](../binaries/ied.md#reproducibility)).
- `_tz`: the value carries **no** zone; an extra `zone` argument names an
  IANA zone (`"Europe/Berlin"`) to read/write it in, DST-correct for the
  instant in question.

A format only gets the suffixes that are reasonable for it in the wild. RFC
3339 and RFC 2822 both carry their own zone syntactically, so each has
exactly one **parser** and no zone suffix there. Their **formatters** must
pick a zone: `format_rfc3339_{utc,tz,local}` (`format_rfc2822` and
`format_imf` are always UTC). `civil` (a bare `"2026-08-10"` or
`"2026-08-10T12:00:00"`, no offset in the grammar) and `fmt` (a
caller-supplied strftime pattern) can honestly go either way, so both get
the full `_utc`/`_local`/`_tz` family in both directions.

A timestamp argument `<t>` is Unix seconds, or a string with its own offset:
RFC 3339 first, then RFC 2822.

## Summary

Pure value conversions — no clock, no host zone, available everywhere, `ied` included:

| Function | Effect |
| --- | --- |
| `time.parse_rfc3339(<str>)` | RFC 3339 only; a UTC offset is required. |
| `time.parse_rfc2822(<str>)` | RFC 2822 only — email `Date:` and HTTP `Date`/`Last-Modified`/`Expires`, numeric or letter zones. |
| `time.parse_civil_utc(<str>)` | A date/date-time with no offset in it, read as UTC. |
| `time.parse_civil_tz(<str> <zone>)` | As `parse_civil_utc`, in a named IANA zone. |
| `time.parse_fmt_utc(<str> <pattern>)` | Exactly `<pattern>`, read as UTC. |
| `time.parse_fmt_tz(<str> <pattern> <zone>)` | As `parse_fmt_utc`, in a named IANA zone. |
| `time.format_rfc3339_utc(<t>)` | RFC 3339 in UTC — `"...Z"`. |
| `time.format_rfc3339_tz(<t> <zone>)` | RFC 3339 in a named IANA zone's offset at `<t>`. |
| `time.format_rfc2822(<t>)` | RFC 2822 in UTC — `"Sat, 13 Jul 2024 15:09:59 -0000"`. |
| `time.format_imf(<t>)` | IMF-fixdate for HTTP headers — `"Sat, 13 Jul 2024 15:09:59 GMT"`. |
| `time.format_civil_utc(<t>)` | `"2026-08-10T12:00:00"` — the inverse of `parse_civil_utc`. |
| `time.format_civil_tz(<t> <zone>)` | As `format_civil_utc`, in a named IANA zone. |
| `time.format_fmt_utc(<t> <pattern>)` | A strftime pattern over UTC fields. |
| `time.format_fmt_tz(<t> <pattern> <zone>)` | As `format_fmt_utc`, over a named IANA zone's fields. |
| `time.parse_delta(<value>)` | A duration string, or a number already in seconds, → seconds. |
| `time.format_delta(<value>)` | The inverse — a delta → `"4d 2h 5m 2s"`. |
| `time.minutes(<n>)` / `time.hours(<n>)` / `time.days(<n>)` | A count of the unit → seconds. |
| `time.truncate(<secs> <unit>)` | Floor a delta to whole units of `"s"`/`"m"`/`"h"`/`"d"`, in seconds. |
| `time.count(<secs> <unit>)` | As `truncate`, but returns the unit count instead of seconds. |
| `time.add_utc(<t> <delta>)` / `time.sub_utc(<t> <delta>)` | Shift a timestamp in UTC; months and years (`"1M"`) anchor at `<t>`. |
| `time.add_tz(<t> <delta> <zone>)` / `time.sub_tz(<t> <delta> <zone>)` | As the `_utc` pair, anchoring in a named IANA zone. |

Ambient-state functions that read the host's clock or time zone. Not
available in `ied` (an `ied` run must be reproducible; see
[reproducibility](../binaries/ied.md#reproducibility)):

| Function | Effect |
| --- | --- |
| `time.now()` | Unix seconds, fractional — the current instant. |
| `time.till(<t>)` | `<t> - time.now()`, in seconds. |
| `time.ago(<t>)` | `time.now() - <t>`, in seconds. |
| `time.parse_civil_local(<str>)` | As `parse_civil_utc`, in the host's time zone. |
| `time.parse_fmt_local(<str> <pattern>)` | As `parse_fmt_utc`, in the host's time zone. |
| `time.format_rfc3339_local(<t>)` | As `format_rfc3339_utc`, in the host's time zone. |
| `time.format_civil_local(<t>)` | As `format_civil_utc`, in the host's time zone. |
| `time.format_fmt_local(<t> <pattern>)` | As `format_fmt_utc`, over the host's local fields. |
| `time.add_local(<t> <delta>)` / `time.sub_local(<t> <delta>)` | As the `_utc` pair, anchoring in the host's time zone. |

## Example

```lua
-- works everywhere: RFC 3339 requires an offset, so no host state is needed
local built = time.parse_rfc3339("2026-08-10T14:00:00+02:00")
print(time.format_rfc3339_utc(built))               --> "2026-08-10T12:00:00Z"
print(time.format_rfc3339_tz(built, "Europe/Berlin")) --> "2026-08-10T14:00:00+02:00"

-- not in `ied`: reads local time, ages against the clock
if time.now then
    local at = time.parse_civil_local("2026-08-10 12:00:00")
    print(time.format_delta(time.ago(at)))
end
```

## Conventions

**Pure vs. ambient.** `time` is registered in two halves. The pure half
(`register`) is every value conversion where the answer follows from its
arguments alone — RFC 3339/2822 carry a zone in the input itself, and a
named-zone function (`_tz`) takes the zone as an *argument* resolved against
the tzdb **bundled into the binary**, not the host's configured zone. All of
that is identical on every machine and every run, so it ships in
`ied` and `ito` alike. The ambient half (`register_host_time`) is
everything that consults the running machine, and is absent in `ied`
(see [reproducibility](../binaries/ied.md#reproducibility)): the clock (`time.now`, `time.till`,
`time.ago`) and the host time zone (every `_local` function, plus
`add_local`/`sub_local`), which follows that zone's DST rules at each instant. Feature-test with `if time.now then … else … end`;
there is no stub that raises "not available" at call time, the function is simply
absent.

**Civil vs. zone-bearing input.** `parse_civil_utc`/`parse_civil_tz`/
`parse_civil_local` all *require* the input to carry no offset of its own —
passing `"2026-08-10T12:00:00Z"` to any of them raises, pointing at
`parse_rfc3339`, rather than silently ignoring the zone the string already
names.

**Named zone vs. pattern.** `_tz` functions take the zone as a separate
argument, never folded into the pattern. Passing an offset or a zone name
where a strftime *pattern* belongs is rejected, and so is a pattern with no
`%` conversion at all (it would otherwise format to itself). A
`parse_fmt_*` pattern that reads an offset (`%z`) is rejected too: the zone
argument would answer for nothing — use `parse_rfc3339` for those.

**Deltas funnel through one coercion.** `time.parse_delta` is the single
implementation every delta-taking function calls — `add_*`, `sub_*`,
`truncate`, `count`, `format_delta`, and (outside this module) `task.wait`
and the `ui.progress` intervals. A *number* means seconds already; a *string*
is read by humantime's grammar (`"1h30m"`, `"4d2h5m2s"`, spaces optional).
A bare numeric string (`"1200"`) is rejected — the grammar demands a unit,
and a caller who means seconds can pass the number `1200` directly. Deltas
are **unsigned as input** (`"-90s"` does not parse; use `sub_*`);
`format_delta` may still print a leading `-` because subtraction is not
required to stay non-negative.

**A day is a fixed 24 hours everywhere** — `"1d"`, `86400` and
`time.days(1)` are the same delta, `add_tz`/`add_local` included (across a
DST change the wall clock moves by an hour). Only **months and years** are
calendar units: `time.parse_delta` rejects them since neither has a fixed
length in seconds, while `add_*`/`sub_*` accept them (`"1M"`, `"1month"`,
`"2y"`) on top of the `parse_delta` grammar, anchored at the timestamp in the
function's zone. The jiff span spellings (`"PT1H"`, `"-1h"`, `"2h ago"`) are
not accepted.

**`truncate`/`count` stop at the day.** There is no week/month/year unit —
`minutes`/`hours`/`days` are the fixed-length builders this module offers,
and anything longer has no fixed length in seconds until a date anchors it
(the same line `parse_delta` draws).

**An age is `time.ago(t)`/`time.till(t)`**, named for the direction, rather
than a bare subtraction whose argument order has to be recalled.

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
in its obsolete-zone form, by HTTP headers. Reads both zone spellings RFC
2822 allows — a numeric offset and the obsolete named zones — in one
function.

**Signature:** `time.parse_rfc2822(text: string) -> number`

**Parameters:**
- `text` — e.g. `"Thu, 29 Feb 2024 05:34:45 -0500"` or
  `"Sat, 13 Jul 2024 15:09:59 GMT"`. Accepts the obsolete named zones
  (`GMT`, `UT`, the single-letter military zones), which is what lets this
  same function read an HTTP `Date`/`Last-Modified`/`Expires` header as well
  as a mail header.

**Returns:** Unix seconds.

**Errors:** raises if `text` is not RFC 2822 — an RFC 3339 string
(`"2026-08-10T12:00:00Z"`) is rejected here.

```lua
local at = time.parse_rfc2822("Sat, 13 Jul 2024 15:09:59 GMT")
time.format_rfc3339_utc(at)   --> "2024-07-13T15:09:59Z"
```

## `time.parse_civil_utc`

Parses a date or date-time with no offset in it, read as UTC.

**Signature:** `time.parse_civil_utc(text: string) -> number`

**Parameters:**
- `text` — a date-time (`"2026-08-10T12:00:00"`) or a bare date
  (`"2026-08-10"`, read as midnight). Must carry **no** offset of its own.

**Returns:** Unix seconds.

**Errors:** raises if `text` already carries a UTC offset (points at
`time.parse_rfc3339`); raises if `text` is neither a date nor a date-time.

```lua
time.parse_civil_utc("2026-08-10T12:00:00")   -- 1786363200
time.parse_civil_utc("2026-08-10")            -- midnight UTC
```

## `time.parse_civil_tz`

As `parse_civil_utc`, placing the offset-less reading in a named IANA zone
instead of UTC.

**Signature:** `time.parse_civil_tz(text: string, zone: string) -> number`

**Parameters:**
- `text` — as `parse_civil_utc`.
- `zone` — an IANA zone name, e.g. `"Europe/Berlin"`, resolved against the
  **bundled** tzdb.

**Returns:** Unix seconds.

**Errors:** raises if `text` already carries an offset; raises if `zone` is
unknown; raises if `text` is neither a date nor a date-time.

**DST edges:** resolved rather than raised (jiff's "compatible" strategy).
On the repeated hour of a fall-back transition, the reading takes the
**first** occurrence; a time that never happened (a spring-forward gap)
shifts **forward** past the gap.

```lua
time.parse_civil_tz("2026-08-10T14:00:00", "Europe/Berlin")   -- 1786363200

-- 02:30 happens twice on this date; this is the earlier one
time.parse_civil_tz("2026-10-25T02:30:00", "Europe/Berlin")
-- formats back as 02:30+02:00

-- 02:30 does not exist on this date; shifts forward
time.parse_civil_tz("2026-03-29T02:30:00", "Europe/Berlin")
-- formats back as 03:30+02:00
```

## `time.parse_fmt_utc`

Parses `text` against exactly the given strftime pattern, read as UTC.

**Signature:** `time.parse_fmt_utc(text: string, pattern: string) -> number`

**Parameters:**
- `text` — the string to parse.
- `pattern` — a strftime pattern, e.g. `"%d.%m.%Y"`.

**Returns:** Unix seconds. A pattern yielding a date with no time component
reads as midnight.

**Errors:** raises if `text` does not match `pattern`, or if the match
yields no date at all; raises if `pattern` has no `%` conversion; raises if
the pattern itself reads an offset (`%z`) — the string would then name an
instant and the zone would answer for nothing (use `parse_rfc3339`).

```lua
time.parse_fmt_utc("10.08.2026", "%d.%m.%Y")   -- midnight UTC, 1786320000
```

## `time.parse_fmt_tz`

As `parse_fmt_utc`, placing an offset-less result in a named IANA zone
instead of UTC.

**Signature:** `time.parse_fmt_tz(text: string, pattern: string, zone: string) -> number`

**Parameters:**
- `text`, `pattern` — as `parse_fmt_utc`.
- `zone` — an IANA zone name.

**Returns:** Unix seconds.

**Errors:** raises if `text` does not match `pattern`; raises if `zone` is
unknown.

```lua
time.parse_fmt_tz("10.08.2026 14:00", "%d.%m.%Y %H:%M", "Europe/Berlin")
```

## `time.format_rfc3339_utc`

Renders a timestamp as RFC 3339 in UTC.

**Signature:** `time.format_rfc3339_utc(t: number|string) -> string`

**Parameters:**
- `t` — a timestamp: Unix seconds, or a string that carries its own offset
  (RFC 3339, then RFC 2822).

**Returns:** e.g. `"2026-08-10T12:00:00Z"`. Sub-second precision is kept
(`"2026-08-10T12:00:00.5Z"`) and omitted for a whole second.

**Errors:** raises if `t` cannot be read as a timestamp.

```lua
time.format_rfc3339_utc(1786363200)   --> "2026-08-10T12:00:00Z"
```

## `time.format_civil_utc`

Renders a timestamp as an offset-less date-time in UTC — the inverse of
`parse_civil_utc`.

**Signature:** `time.format_civil_utc(t: number|string) -> string`

**Parameters:**
- `t` — a timestamp.

**Returns:** e.g. `"2026-08-10T12:00:00"` (with a fraction when there is one).

**Errors:** raises if `t` cannot be read as a timestamp.

```lua
time.format_civil_utc(1786363200)   --> "2026-08-10T12:00:00"
```

## `time.format_civil_tz`

As `format_civil_utc`, over a named IANA zone's wall clock.

**Signature:** `time.format_civil_tz(t: number|string, zone: string) -> string`

**Parameters:**
- `t` — a timestamp.
- `zone` — an IANA zone name.

**Returns:** e.g. `"2026-08-10T14:00:00"`.

**Errors:** raises if `zone` is unknown; raises if `t` cannot be read as a
timestamp.

```lua
time.format_civil_tz(1786363200, "Europe/Berlin")   --> "2026-08-10T14:00:00"
```

## `time.format_rfc3339_tz`

Renders a timestamp as RFC 3339 in a named IANA zone, DST-correct for that
instant's date.

**Signature:** `time.format_rfc3339_tz(t: number|string, zone: string) -> string`

**Parameters:**
- `t` — a timestamp.
- `zone` — an IANA zone name, e.g. `"Europe/Berlin"`. Resolved against the
  **bundled** tzdb, so the answer is identical on every machine.

**Returns:** e.g. `"2026-08-10T14:00:00+02:00"`.

**Errors:** raises if `zone` is a fixed-offset spelling instead of a zone
name, or is not a known IANA name; raises if `t` cannot be read as a
timestamp.

```lua
time.format_rfc3339_tz(summer_t, "Europe/Berlin")   --> "2026-08-10T14:00:00+02:00"
time.format_rfc3339_tz(winter_t, "Europe/Berlin")   --> "2026-01-10T13:00:00+01:00"
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

## `time.format_fmt_utc`

Renders a timestamp through a strftime pattern over UTC fields.

**Signature:** `time.format_fmt_utc(t: number|string, pattern: string) -> string`

**Parameters:**
- `t` — a timestamp.
- `pattern` — a strftime pattern, e.g. `"%Y-%m-%d"`.

**Returns:** the formatted string.

**Errors:** raises if `t` cannot be read as a timestamp; raises if
`pattern` has no `%` conversion (an offset or zone name there gets an
argument-order hint) or is otherwise invalid.

```lua
time.format_fmt_utc(1786363200, "%Y-%m-%d")     --> "2026-08-10"
```

## `time.format_fmt_tz`

As `format_fmt_utc`, over a named IANA zone's fields.

**Signature:** `time.format_fmt_tz(t: number|string, pattern: string, zone: string) -> string`

**Parameters:**
- `t`, `pattern` — as `format_fmt_utc`.
- `zone` — an IANA zone name.

**Returns:** the formatted string.

**Errors:** raises if `zone` is a fixed-offset spelling instead of a zone
name, or is not a known IANA name; raises if `pattern` looks like an
offset/zone; raises if `t` cannot be read as a timestamp.

```lua
time.format_fmt_tz(t, "%Y-%m-%d %H:%M %:z", "Europe/Berlin")
```

## `time.parse_delta`

Reads a delta from a number (seconds) or a duration string.

**Signature:** `time.parse_delta(value: number|string) -> number`

**Parameters:**
- `value` — a number of seconds, or a duration string like `"30s"`,
  `"1h 30min"`, `"4d2h5m"` (humantime grammar; spaces optional, units
  largest-first).

**Returns:** the delta in seconds.

**Errors:** raises if `value` is a bare numeric string with no unit
(`"1200"` — write `"1200s"` or pass the number `1200`); raises on a month
or year unit (`"1month"`, `"1y"` — no fixed length in seconds; use
`time.add_utc`/`time.sub_utc` or the `_local`/`_tz` variants, which anchor
at a timestamp); raises on a leading `-` (deltas are unsigned; use `sub_*`);
raises on any other unparseable string; raises if a numeric `value` is
non-finite.

```lua
time.parse_delta("1h 30min")   -- 5400
time.parse_delta("1h30m")      -- 5400
time.parse_delta("4d2h5m2s")   -- 353102
time.parse_delta(5400)         -- 5400
time.parse_delta("1d")         -- 86400, a day is 24h here
time.parse_delta("1w")         -- 604800
time.parse_delta("1200")       -- error: needs a unit
time.parse_delta("1month")     -- error: use time.add_utc(t, "1month")
```

## `time.format_delta`

The inverse of `time.parse_delta`.

**Signature:** `time.format_delta(value: number|string) -> string`

**Parameters:**
- `value` — anything `time.parse_delta` accepts (so formatting a duration
  string normalises it).

**Returns:** e.g. `"4d 2h 5m 2s"`. Any sub-second fraction is dropped
(the second is this module's display resolution). A negative value (from
subtraction, not from input) prints with a leading `-`. All-zero prints as
`"0s"`.

**Errors:** same as `time.parse_delta`.

```lua
time.format_delta(5400)      --> "1h 30m"
time.format_delta(353102)    --> "4d 2h 5m 2s"
time.format_delta("90m")     --> "1h 30m"
time.format_delta(-90)       --> "-1m 30s"
time.format_delta(0.5)       --> "0s"
```

## `time.minutes` / `time.hours` / `time.days`

Build a delta from a count of the unit.

**Signature:**
`time.minutes(n: number) -> number`
`time.hours(n: number) -> number`
`time.days(n: number) -> number`

**Parameters:**
- `n` — a count, e.g. `2`.

**Returns:** the delta in seconds — `time.hours(2)` is `7200`, the same
number `time.parse_delta("2h")` gives, without a string in between.

**Errors:** raises (naming the function) if `n` is not a finite number — a
string such as `"2h"` is not accepted here; use `time.parse_delta`.

```lua
time.minutes(2)   -- 120
time.hours(2)     -- 7200
time.days(2)      -- 172800
```

## `time.truncate`

Floors a delta to whole units, returning the result in seconds.

**Signature:** `time.truncate(value: number|string, unit: string) -> number`

**Parameters:**
- `value` — anything `time.parse_delta` accepts.
- `unit` — one of `"s"`, `"m"`, `"h"`, `"d"` (long
  spellings also accepted). No finer unit than second, no coarser than day
  — `minutes`/`hours`/`days` are this module's fixed-length builders, and
  anything longer has no fixed length in seconds.

**Returns:** the delta floored to whole `unit`s, still in seconds — so it
composes into `format_delta` or further arithmetic.

**Errors:** raises on an unknown unit; raises on an invalid `value` per
`time.parse_delta`.

```lua
time.truncate(7500, "h")     -- 7200, two whole hours
time.truncate(90, "m")       -- 60
```

## `time.count`

As `time.truncate`, but returns the unit count instead of the seconds it
represents.

**Signature:** `time.count(value: number|string, unit: string) -> number`

**Parameters:** as `time.truncate`.

**Returns:** the whole number of `unit`s in `value`.

**Errors:** as `time.truncate`.

```lua
time.count(7500, "h")    -- 2
time.count(90, "m")      -- 1
```

## `time.add_utc` / `time.sub_utc`

Shift a timestamp by a delta, anchored in UTC. Months and years (`"1M"`,
`"1y"`) anchor at the timestamp, so they resolve calendar-correctly rather
than through a fixed-length approximation; everything else is a fixed
duration (a day is 24 hours).

**Signature:**
`time.add_utc(t: number|string, delta: number|string) -> number`
`time.sub_utc(t: number|string, delta: number|string) -> number`

**Parameters:**
- `t` — a timestamp to shift from.
- `delta` — a number of seconds, or a string in `time.parse_delta`'s grammar
  (`"1h30m"`) **plus** months and years (`"1M"`, `"1month"`, `"2y"`, also
  combined: `"1M 2d"`) — accepted here because `t` gives them something to
  anchor to, in UTC. Months/years apply first, then the fixed part. The ISO
  8601 and jiff spellings (`"PT1H"`, `"-1h"`, `"2h ago"`) are not accepted.

**Returns:** the shifted timestamp, in Unix seconds. `sub_utc` negates the
delta before applying it.

**Errors:** raises (prefixed `time.add_utc`/`time.sub_utc`) if `t` cannot be
read as a timestamp; raises if `delta` is not a valid number or duration
string; raises if the result is out of range.

```lua
-- January 31 + one month is the end of February, not 30 days later
time.format_rfc3339_utc(time.add_utc(time.parse_rfc3339("2026-01-31T00:00:00Z"), "1M"))
-- "2026-02-28T00:00:00Z"

time.sub_utc(time.add_utc(1786363200, "1h"), "1h")   -- 1786363200, inverses
```

## `time.add_tz` / `time.sub_tz`

As `time.add_utc`/`time.sub_utc`, anchoring months and years in a named
IANA zone instead of UTC. A day is still a fixed 24 hours, so `"1d"` equals
`86400` even across a DST change.

**Signature:**
`time.add_tz(t: number|string, delta: number|string, zone: string) -> number`
`time.sub_tz(t: number|string, delta: number|string, zone: string) -> number`

**Parameters:**
- `t`, `delta` — as `time.add_utc`.
- `zone` — an IANA zone name; the calendar arithmetic (e.g. "end of the
  month") follows that zone's local calendar.

**Returns:** the shifted timestamp, in Unix seconds.

**Errors:** as `time.add_utc`, plus raises if `zone` is unknown.

```lua
time.add_tz(t, "1M", "Europe/Berlin")
```

## `time.now`

The current instant. Not available in `ied`.

**Signature:** `time.now() -> number`

**Returns:** Unix seconds, fractional.

**Errors:** does not exist in `ied` — `time.now` is `nil`
there; feature-test with `if time.now then`.

## `time.till` / `time.ago`

Now-relative subtractions, named for the direction each measures. Not
available in `ied`.

**Signature:**
`time.till(t: number|string) -> number`
`time.ago(t: number|string) -> number`

**Parameters:**
- `t` — a timestamp.

**Returns:** `time.till(t)` is `t - time.now()` (seconds until `t`, negative
if `t` is in the past); `time.ago(t)` is `time.now() - t` (seconds since
`t`, negative if `t` is in the future).

**Errors:** raises if `t` cannot be read as a timestamp (an offset-less
string is rejected — there is no zone context here to place it in; use
`parse_civil_local` first).

```lua
local age = time.ago(started_at)
print(time.format_delta(age) .. " ago")
```

## `time.parse_civil_local`

As `parse_civil_utc`, in the host's time zone. Not available in `ied`.

**Signature:** `time.parse_civil_local(text: string) -> number`

**Parameters:**
- `text` — a date-time or bare date with no offset of its own.

**Returns:** Unix seconds, placed in the host's time zone with the offset
**that zone has on that date** (a human writing `"2026-08-10"` means
midnight where they are, summer time included).

**Errors:** raises if `text` already carries an offset; raises if `text` is
neither a date nor a date-time. Absent in `ied`.

```lua
time.parse_civil_local("2026-08-10T12:00:00")
time.parse_civil_local("2026-08-10")   -- local midnight
```

## `time.parse_fmt_local`

As `parse_fmt_utc`, in the host's time zone. Not available in `ied`.

**Signature:** `time.parse_fmt_local(text: string, pattern: string) -> number`

**Parameters:** as `parse_fmt_utc`.

**Returns:** Unix seconds, placed in the host's time zone.

**Errors:** as `parse_fmt_utc`. Absent in `ied`.

## `time.format_rfc3339_local`

As `format_rfc3339_utc`, in the host's time zone. Not available in `ied`.

**Signature:** `time.format_rfc3339_local(t: number|string) -> string`

**Parameters:**
- `t` — a timestamp.

**Returns:** RFC 3339 with the offset the host zone has at `t` (e.g.
`"+02:00"` in summer, `"+01:00"` in winter), not `"Z"`, unless the host is
at UTC then.

**Errors:** raises if `t` cannot be read as a timestamp. Absent in `ied`.

```lua
time.format_rfc3339_utc(t)     --> "2026-08-10T12:00:00Z"
time.format_rfc3339_local(t)   --> "2026-08-10T14:00:00+02:00"  -- host in CEST
```

## `time.format_civil_local`

As `format_civil_utc`, over the host zone's wall clock. Not available in `ied`.

**Signature:** `time.format_civil_local(t: number|string) -> string`

**Parameters:** as `format_civil_utc`.

**Returns:** e.g. `"2026-08-10T14:00:00"`.

**Errors:** raises if `t` cannot be read as a timestamp. Absent in `ied`.

## `time.format_fmt_local`

As `format_fmt_utc`, over the host's local fields. Not available in `ied`.

**Signature:** `time.format_fmt_local(t: number|string, pattern: string) -> string`

**Parameters:** as `format_fmt_utc`.

**Returns:** the formatted string, over the host zone's fields at `t`.

**Errors:** as `format_fmt_utc`. Absent in `ied`.

## `time.add_local` / `time.sub_local`

As `time.add_utc`/`time.sub_utc`, anchoring months and years in the host's
time zone. Not available in `ied`.

**Signature:**
`time.add_local(t: number|string, delta: number|string) -> number`
`time.sub_local(t: number|string, delta: number|string) -> number`

**Parameters:** as `time.add_utc`.

**Returns:** the shifted timestamp, in Unix seconds.

**Errors:** as `time.add_utc`. Absent in `ied`.

## `os.time`/`os.date`

Luau's standard `os.time`, `os.date`, `os.clock`, and
`os.difftime` are also available (not in `ied`). `time.now()` differs from `os.time()` only
in keeping the fractional part.
