//! `time` module: timestamps and deltas as plain numbers.
//!
//! Two representations, both ordinary Luau numbers, so `<` compares and `+`
//! arithmetic works without unwrapping anything and a value survives a trip
//! through `json`:
//!
//! - a **timestamp** is Unix seconds — what `os.time()` returns;
//! - a **delta** is a number of seconds, signed.
//!
//! Every function that takes either accepts *both spellings*: the number, or a
//! string. That is what makes the delta arithmetic total — `time.add(t, "1h")`
//! and `time.add(t, 3600)` are the same call, and `time.diff` does not care
//! which of its two arguments came from where.
//!
//! # Two layers
//!
//! Each conversion comes in a **convenience** form that sniffs the input's
//! shape, and an **explicit** form named for the one format it accepts:
//!
//! ```lua
//! time.parse("2026-08-10T12:00:00Z")        -- intend: whatever this is
//! time.parse_rfc3339("2026-08-10T12:00:00Z") -- say: RFC 3339, or an error
//! time.parse_strftime("10.08.2026", "%d.%m.%Y")
//! time.parse_rfc2822("Thu, 29 Feb 2024 05:34:45 -0500")
//! ```
//!
//! The explicit ones exist so a script parsing a *known* format fails on
//! malformed input instead of silently succeeding through some other branch;
//! the convenience one exists because most input is obvious.
//!
//! `time.parse` reads the ISO-shaped forms **only** — it does not sniff RFC
//! 2822. A `Date:` header is not ambiguous input a script fell into, it is a
//! format a script went looking for, and a branch that tried both would have to
//! report failure as "neither", which is a worse message than either alone.
//!
//! # RFC 2822, and HTTP dates
//!
//! Email headers (`Date:`) and HTTP ones (`Date`, `Last-Modified`, `Expires`)
//! carry the RFC 2822 spelling, so [`register`] parses it and prints it in both
//! of its two shapes:
//!
//! ```lua
//! time.parse_rfc2822("Sat, 13 Jul 2024 15:09:59 GMT") -- either shape, in
//! time.format_rfc2822(t)  --> "Sat, 13 Jul 2024 15:09:59 -0000"
//! time.format_imf(t)      --> "Sat, 13 Jul 2024 15:09:59 GMT"
//! ```
//!
//! These are **pure**, and so live in all four binaries: RFC 2822 requires a
//! zone just as RFC 3339 requires an offset, so neither direction consults the
//! host. `format_imf` is the IMF-fixdate of RFC 9110 — always `GMT`, always a
//! two-digit day, always 29 characters — which is what an HTTP header must
//! contain and what `format_rfc2822` is one character-class too loose for.
//! There is no `parse_imf`, because IMF-fixdate is a *subset* going in:
//! `parse_rfc2822` accepts the obsolete named zones (`GMT`, `UT`, the military
//! letters) and so already reads every header the printer emits. The narrowing
//! only matters on output, where a receiver may be strict.
//!
//! # Time zones, and why `parse` is host-only
//!
//! RFC 3339 **requires** an offset (`Z` or `±HH:MM`) — that is the main thing
//! its grammar narrows out of ISO 8601 — so a full timestamp needs no zone
//! context and parsing one is a pure value conversion, available everywhere.
//!
//! An offset-*less* input is a different question. `"2026-08-10"` written by a
//! human means midnight where that human is, so [`register_host_time`] reads
//! the **local** offset for it — which makes it ambient state, and puts
//! `time.parse` (which accepts those forms) under `ito`/`irun` only, beside
//! `time.now()`. `iq`/`ied` keep the whole of RFC 3339 through
//! `time.parse_rfc3339`; what they lose is only the guessing. Absent rather
//! than stubbed, so `if time.now then` is a feature test.
//!
//! Output in the shared layer is UTC (`time.format` emits `Z`, and a strftime
//! pattern renders UTC fields). Because a timestamp is an instant, an input's
//! offset is *honored and then absorbed*: `12:00:00Z` and `14:00:00+02:00`
//! parse to the same number, and formatting either returns the `Z` spelling.
//! Parse→format does not preserve the original offset, which is the price of
//! numbers over opaque values.
//!
//! # Deltas
//!
//! A delta is **a number of seconds**, and there is exactly one way to read one
//! and one way to write it. Everywhere a delta is taken — `time.add`,
//! `time.sub`, `task.wait` — the same coercion applies: a *number* is seconds
//! already, a *string* goes through humantime's grammar.
//!
//! ```lua
//! time.delta("1h 30min")   -- 5400
//! time.delta("1h30m")      -- 5400   spaces are optional
//! time.delta("4d2h5m2s")   -- 353102
//! time.delta(5400)         -- 5400   already seconds
//! time.delta("1200")       -- error: a bare number in a string has no unit
//! ```
//!
//! Formatting is the inverse, and the only spelling there is:
//!
//! ```lua
//! time.format_delta(5400)     -- "1h 30m"
//! time.format_delta(353102)   -- "4d 2h 5m 2s"
//! ```
//!
//! A string needs a unit. `"1200"` is rejected rather than read as seconds: the
//! grammar demands one, and a caller who means seconds can pass the number
//! `1200`, which is unambiguous in a way the string is not.
//!
//! **Deltas are durations, so they do not carry a sign.** The parser reads
//! magnitudes only. Subtraction can still land below zero, and
//! [`format_delta`] renders such a value with a leading `-` rather than
//! refusing it — the arithmetic is the caller's, and printing it is not the
//! place to argue.
//!
//! `time.round(secs, unit)` truncates a delta toward zero, which is what makes
//! a fractional delta printable — without it the sub-second remainder renders
//! all the way into nanoseconds. It returns a single number, so it composes
//! directly into [`format_delta`]. When the part below the unit is wanted too,
//! `time.round_with_rest` returns both; it is separate precisely because a
//! second return value silently becomes the next call's second *argument*.
//!
//! `P1D`/`P1W` are fixed deltas (a day is 24h here, matching what humantime's
//! `"1day"` has always meant). `P1M`/`P1Y` are **not** — a month has no length
//! until you say which month — so they are rejected by the delta parsers and
//! accepted by [`add`](fn@register)/`sub`, where the timestamp anchors them and
//! the result is calendar-correct. For the same reason `time.round` has no
//! month or year unit.

use jiff::civil::DateTime;
use jiff::fmt::rfc2822;
use jiff::fmt::strtime;
use jiff::fmt::temporal::DateTimeParser;
use jiff::tz::{Offset, TimeZone};
use jiff::{Span, Timestamp};
use mlua::{Lua, Result as LuaResult, Table, Value};

/// Parser for the offset spellings, reused rather than rebuilt per call.
static ZONE_PARSER: DateTimeParser = DateTimeParser::new();

/// A timestamp argument, in whichever spelling the script used.
///
/// The string arm carries no zone assumption of its own: it is resolved by
/// whichever entry point read it, so the shared layer can demand an offset
/// while the host-only layer fills in the local one.
enum TimeArg {
    Unix(f64),
    Text(String),
}

impl TimeArg {
    fn from_value(value: &Value, what: &str) -> LuaResult<Self> {
        match value {
            Value::Integer(n) => Ok(TimeArg::Unix(*n as f64)),
            Value::Number(n) => {
                if !n.is_finite() {
                    return Err(mlua::Error::runtime(format!(
                        "{what} expects a finite Unix timestamp, got {n}"
                    )));
                }
                Ok(TimeArg::Unix(*n))
            }
            Value::String(s) => Ok(TimeArg::Text(s.to_str()?.trim().to_string())),
            other => Err(mlua::Error::runtime(format!(
                "{what} expects a timestamp -- Unix seconds as a number, or a \
                 string like \"2026-08-10T12:00:00Z\" -- got {}",
                other.type_name()
            ))),
        }
    }

    /// Resolve to Unix seconds, using `offset` for an input that carries none.
    ///
    /// `None` rejects offset-less input, which is what the shared layer passes:
    /// with no zone context there is no honest answer, only a guess.
    fn resolve(self, offset: Option<Offset>, what: &str) -> LuaResult<f64> {
        match self {
            TimeArg::Unix(secs) => Ok(secs),
            TimeArg::Text(text) => parse_timestamp(&text, offset, what),
        }
    }
}

/// Parse a timestamp string to Unix seconds.
///
/// Tries the offset-bearing forms first, so an explicit offset always wins over
/// `offset`; only then the naive ones, which `offset` completes.
fn parse_timestamp(text: &str, offset: Option<Offset>, what: &str) -> LuaResult<f64> {
    // Offset-bearing: RFC 3339 and the compact/space-separated variants jiff
    // accepts for the same instant.
    if let Ok(ts) = text.parse::<Timestamp>() {
        return Ok(timestamp_to_secs(ts));
    }

    let Some(offset) = offset else {
        return Err(mlua::Error::runtime(format!(
            "{what} could not read '{text}' as a timestamp: it carries no UTC \
             offset, and RFC 3339 requires one (a trailing \"Z\" or \"+02:00\"). \
             Under ito/irun, time.parse reads such an input as local time."
        )));
    };

    // Naive: a date-time, then a bare date as midnight. Both are completed with
    // `offset`, which the caller has already decided is the right context.
    if let Ok(dt) = text.parse::<DateTime>() {
        return Ok(timestamp_to_secs(offset.to_timestamp(dt).map_err(|e| {
            mlua::Error::runtime(format!("{what} could not place '{text}' in time: {e}"))
        })?));
    }
    if let Ok(date) = text.parse::<jiff::civil::Date>() {
        let dt = date.at(0, 0, 0, 0);
        return Ok(timestamp_to_secs(offset.to_timestamp(dt).map_err(|e| {
            mlua::Error::runtime(format!("{what} could not place '{text}' in time: {e}"))
        })?));
    }

    Err(mlua::Error::runtime(format!(
        "{what} could not read '{text}' as a timestamp; expected something like \
         \"2026-08-10T12:00:00Z\", \"2026-08-10 12:00:00\" or \"2026-08-10\""
    )))
}

/// Unix seconds for an instant, keeping sub-second precision.
fn timestamp_to_secs(ts: Timestamp) -> f64 {
    ts.as_second() as f64 + f64::from(ts.subsec_nanosecond()) / 1e9
}

/// An instant from Unix seconds, splitting the fraction off so a negative
/// timestamp truncates toward the epoch rather than away from it.
fn secs_to_timestamp(secs: f64, what: &str) -> LuaResult<Timestamp> {
    let whole = secs.trunc() as i64;
    let nanos = ((secs - secs.trunc()) * 1e9).round() as i32;
    Timestamp::new(whole, nanos)
        .map_err(|e| mlua::Error::runtime(format!("{what}: {secs} is not a valid timestamp ({e})")))
}

/// Read a delta from a number of seconds or a duration string.
///
/// The *one* delta coercion: every function taking a delta calls this, so
/// `time.add`, `time.round` and `task.wait` cannot disagree about what a
/// duration string means. A number is seconds; a string goes through
/// humantime's grammar, which has no notion of a month or year -- units with no
/// fixed length in seconds. `add`/`sub` take those separately, having a
/// timestamp to anchor them.
fn delta_from(value: &Value, what: &str) -> LuaResult<f64> {
    match value {
        Value::Integer(n) => Ok(*n as f64),
        Value::Number(n) => {
            if !n.is_finite() {
                return Err(mlua::Error::runtime(format!(
                    "{what} expects a finite number of seconds, got {n}"
                )));
            }
            Ok(*n)
        }
        Value::String(s) => {
            let text = s.to_str()?;
            parse_delta_str(text.trim(), what)
        }
        other => Err(mlua::Error::runtime(format!(
            "{what} expects a delta -- seconds as a number, or a string like \
             \"30s\", \"1h 30min\" or \"4d2h\" -- got {}",
            other.type_name()
        ))),
    }
}

/// Name the calendar unit in a duration string, if it has one.
///
/// Scans for humantime's month and year spellings as whole tokens, so `1m`
/// (minutes) and `1mo`-free text pass while `1month`/`1M`/`2years` are caught.
/// The check is on the *unit letters* following a digit, which is the only
/// place a unit can appear in that grammar.
fn calendar_unit(text: &str) -> Option<&'static str> {
    let mut rest = text;
    while let Some(start) = rest.find(|c: char| c.is_ascii_alphabetic()) {
        let tail = &rest[start..];
        let end = tail
            .find(|c: char| !c.is_ascii_alphabetic())
            .unwrap_or(tail.len());
        let (word, next) = tail.split_at(end);
        match word {
            // `M` is humantime's month; lowercase `m` is minutes.
            "M" | "month" | "months" => return Some("month"),
            "y" | "year" | "years" => return Some("year"),
            _ => {}
        }
        rest = next;
    }
    None
}

/// Parse a duration string into seconds.
///
/// Durations are unsigned: humantime reads a magnitude, and a leading `-` is
/// not part of the grammar. A delta that went below zero is a result of
/// arithmetic, not something anyone writes down.
///
/// Months and years are **rejected before humantime sees them**. It accepts
/// both and resolves them to fixed approximations -- a month is 30.44 days, a
/// year 365.25 -- which is precisely the answer this module refuses to invent:
/// neither has a length in seconds until a date anchors it. `add`/`sub` take
/// them instead, and say so in the error.
fn parse_delta_str(text: &str, what: &str) -> LuaResult<f64> {
    if let Some(unit) = calendar_unit(text) {
        return Err(mlua::Error::runtime(format!(
            "{what} cannot read '{text}': a {unit} has no fixed length in \
             seconds until it is anchored to a date. Use \
             time.add(t, '{text}') / time.sub(t, '{text}'), which anchor at \
             the timestamp."
        )));
    }
    humantime::parse_duration(text)
        .map(|d| d.as_secs_f64())
        .map_err(|e| {
            // A bare number is the one failure worth naming: it is the mistake
            // a caller makes on purpose, expecting seconds.
            if !text.is_empty() && text.chars().all(|c| c.is_ascii_digit() || c == '.') {
                return mlua::Error::runtime(format!(
                    "{what} needs a unit on '{text}' -- write \"{text}s\" for \
                     seconds, or pass the number {text}."
                ));
            }
            mlua::Error::runtime(format!(
                "{what} could not read '{text}' as a delta ({e}); expected \
                 something like \"30s\", \"1h 30min\" or \"4d2h5m\". A month or \
                 year has no fixed length in seconds -- use time.add(t, ...) / \
                 time.sub(t, ...), which anchor at the timestamp."
            ))
        })
}

/// Parse either duration spelling into a span.
///
/// One grammar covers both: jiff's `Span` reads ISO 8601 (`PT1H30M`) *and* the
/// friendly form (`1h 30min`) `task.wait` has always taken, so there is no mode
/// to select and no second parser to disagree with the first.
fn parse_span(text: &str, what: &str) -> LuaResult<Span> {
    text.parse::<Span>().map_err(|e| {
        // A bare number is the one failure worth naming: it is the mistake a
        // caller makes on purpose, expecting seconds.
        if !text.is_empty()
            && text
                .chars()
                .all(|c| c.is_ascii_digit() || c == '.' || c == '-')
        {
            return mlua::Error::runtime(format!(
                "{what} could not read '{text}' as a delta: a duration string \
                 needs a unit (\"{text}s\"). Pass the number {text} to mean seconds."
            ));
        }
        mlua::Error::runtime(format!(
            "{what} could not read '{text}' as a delta ({e}); expected something \
             like \"30s\", \"1h 30min\" or \"PT1H30M\""
        ))
    })
}

/// Parse a **fixed UTC offset** — `"+05:30"`, `"-08:00"`, `"Z"`.
///
/// Deliberately narrower than what `parse_time_zone` accepts: that also reads
/// IANA identifiers and POSIX strings, and letting either through here would
/// make `format_offset` answer for input the name does not promise (a named
/// zone has a *varying* offset, which is the whole distinction from
/// `format_tz`). `"Z"`/`"UTC"` are special-cased because they are what a caller
/// reaches for first and jiff rejects them as offsets.
fn offset_from(text: &str, what: &str) -> LuaResult<Offset> {
    let text = text.trim();
    if text.eq_ignore_ascii_case("z") || text.eq_ignore_ascii_case("utc") {
        return Ok(Offset::UTC);
    }
    // An offset must be signed; anything else is a zone name or nonsense, and
    // saying so beats jiff's "tzdb lookup failed".
    if !text.starts_with('+') && !text.starts_with('-') {
        return Err(mlua::Error::runtime(format!(
            "{what} expects a fixed UTC offset like \"+05:30\", \"-08:00\" or \"Z\", \
             got '{text}'. For a named zone (\"Europe/Berlin\") use time.format_tz."
        )));
    }
    let zone = ZONE_PARSER.parse_time_zone(text).map_err(|e| {
        mlua::Error::runtime(format!(
            "{what} could not read '{text}' as a UTC offset: {e}"
        ))
    })?;
    zone.to_fixed_offset().map_err(|_| {
        mlua::Error::runtime(format!(
            "{what} expects a fixed UTC offset like \"+05:30\", got '{text}'"
        ))
    })
}

/// Parse an **IANA time zone name** — `"Europe/Berlin"`, `"UTC"`.
///
/// Resolved against the bundled tzdb, so the answer is the same on every
/// machine (see the `jiff` features in the workspace manifest) — which is what
/// keeps this a pure value conversion rather than ambient state.
fn zone_from(text: &str, what: &str) -> LuaResult<TimeZone> {
    let text = text.trim();
    // A fixed offset is not a zone: it has no DST rules, so accepting one here
    // would quietly answer a different question than the caller asked.
    if text.starts_with('+') || text.starts_with('-') {
        return Err(mlua::Error::runtime(format!(
            "{what} expects an IANA time zone name like \"Europe/Berlin\", got the \
             fixed offset '{text}'. Use time.format_offset for a fixed offset."
        )));
    }
    TimeZone::get(text).map_err(|_| {
        mlua::Error::runtime(format!(
            "{what} does not know the time zone '{text}'; expected an IANA name \
             like \"Europe/Berlin\", \"America/New_York\" or \"UTC\""
        ))
    })
}

/// Reject an offset or zone name passed where a strftime pattern belongs.
///
/// Without this the mistake is *silent*: a pattern containing no `%` is
/// returned verbatim, so `time.format(t, "+05:30")` yields the string
/// `"+05:30"` rather than a timestamp. That reads as a plausible answer, which
/// makes it worse than an error.
fn check_pattern(pattern: &str, what: &str) -> LuaResult<()> {
    if pattern.contains('%') {
        return Ok(());
    }
    let looks_like_offset = pattern.starts_with('+')
        || (pattern.starts_with('-') && pattern.len() > 1)
        || pattern.eq_ignore_ascii_case("z")
        || pattern.eq_ignore_ascii_case("utc");
    if looks_like_offset {
        return Err(mlua::Error::runtime(format!(
            "{what} takes a strftime pattern, not the offset '{pattern}'. Use \
             time.format_offset(t, '{pattern}') to render in a fixed offset."
        )));
    }
    if pattern.contains('/') {
        return Err(mlua::Error::runtime(format!(
            "{what} takes a strftime pattern, not the time zone '{pattern}'. Use \
             time.format_tz(t, '{pattern}') to render in a named zone."
        )));
    }
    Ok(())
}

/// Render an instant through a strftime pattern in `zone`, or as RFC 3339 when
/// no pattern was given.
fn format_at(
    ts: Timestamp,
    zone: TimeZone,
    pattern: Option<&str>,
    what: &str,
) -> LuaResult<String> {
    if let Some(pattern) = pattern {
        check_pattern(pattern, what)?;
    }
    match pattern {
        // RFC 3339. In UTC that is the `Z` spelling; in a fixed-offset zone the
        // `+HH:MM` one, which is still RFC 3339 and the point of asking.
        None => {
            let zoned = ts.to_zoned(zone);
            Ok(strtime::format("%Y-%m-%dT%H:%M:%S%:z", &zoned)
                .map_err(|e| mlua::Error::runtime(format!("{what} could not format: {e}")))?
                .replace("+00:00", "Z"))
        }
        Some(pattern) => {
            let zoned = ts.to_zoned(zone);
            strtime::format(pattern, &zoned).map_err(|e| {
                mlua::Error::runtime(format!(
                    "{what} could not format with '{pattern}': {e}; expected \
                     strftime conversions like %Y-%m-%d or %H:%M:%S"
                ))
            })
        }
    }
}

/// Format a delta in seconds as `"4d 2h 5m 2s"`.
///
/// **The second is this module's display resolution.** A delta arrives as an
/// `f64` and a computed one carries a fraction; rendering it to the last digit
/// gives `173h 44m 24s 737ms 129µs 927ns`, which is not a more precise answer
/// but an unreadable one. So the fraction is dropped here rather than left to
/// every call site to remember, and a script wanting a coarser unit than the
/// second says so with [`round_delta`].
///
/// The decomposition is written out rather than delegated because neither
/// candidate renders the wanted spelling: jiff's `SignedDuration` is
/// calendar-less and stops at the hour (4 days prints as `96h`), and
/// humantime's formatter spells the large units in full (`4days 2h 5m 2s`) and
/// approximates years and months. The units here stop at the day for the same
/// reason [`round_unit`]'s ceiling is the week -- anything longer has no fixed
/// length in seconds. Output stays readable by [`parse_delta_str`].
///
/// Durations are unsigned, but subtraction is not: a negative value keeps a
/// leading `-` on the magnitude rather than being refused, since the arithmetic
/// was the caller's business and this is only the printer.
fn format_delta(secs: f64) -> String {
    let secs = secs.trunc();
    let sign = if secs < 0.0 { "-" } else { "" };
    let mut rest = secs.abs() as u64;
    let mut parts = Vec::new();
    for (unit, label) in [(86_400, "d"), (3_600, "h"), (60, "m"), (1, "s")] {
        let n = rest / unit;
        if n > 0 {
            parts.push(format!("{n}{label}"));
            rest -= n * unit;
        }
    }
    // Every unit was zero: say so in the finest one rather than returning "".
    if parts.is_empty() {
        return format!("{sign}0s");
    }
    format!("{sign}{}", parts.join(" "))
}

/// The unit a delta rounds to, as a number of seconds.
///
/// The second is the floor, matching [`format_delta`]'s resolution: rounding to
/// a fraction of one would produce a value nothing here can display. The
/// ceiling is the week -- a month or year has no length in seconds until a date
/// anchors it, the same line [`parse_delta_str`] draws.
fn round_unit(name: &str, what: &str) -> LuaResult<f64> {
    match name.trim() {
        "s" | "sec" | "secs" | "second" | "seconds" => Ok(1.0),
        "m" | "min" | "mins" | "minute" | "minutes" => Ok(60.0),
        "h" | "hr" | "hrs" | "hour" | "hours" => Ok(3600.0),
        "d" | "day" | "days" => Ok(86_400.0),
        "w" | "week" | "weeks" => Ok(604_800.0),
        other => Err(mlua::Error::runtime(format!(
            "{what} does not know the unit '{other}'; expected one of \"s\", \
             \"m\", \"h\", \"d\", \"w\". The second is the finest resolution \
             this module displays; a month or year has no fixed length."
        ))),
    }
}

/// Truncate a delta toward zero at `unit`, returning the rounded value and what
/// was left below it.
///
/// Both are **whole seconds**: the sub-second part is dropped outright rather
/// than surviving in the remainder, since nothing here displays it. So
/// `rounded + remainder` is the delta truncated to seconds, not the delta.
///
/// Toward zero rather than to-nearest so an age never overstates: a delta of
/// `1m59s` displayed to the minute reads `1m`, the way a human says "an hour
/// ago" of something 90 minutes old. Symmetric across the sign -- a delta that
/// arithmetic pushed below zero truncates toward zero from that side too, which
/// is why the remainder carries the delta's sign.
fn round_delta(secs: f64, unit: f64) -> (f64, f64) {
    let secs = secs.trunc();
    let rounded = (secs / unit).trunc() * unit;
    (rounded, secs - rounded)
}

/// The local offset, read once per call so a long-running script follows a DST
/// transition instead of pinning the offset it started with.
fn local_offset() -> Offset {
    TimeZone::system().to_offset(Timestamp::now())
}

// ---------------------------------------------------------------------------
// Engine registration
// ---------------------------------------------------------------------------

/// Register the `time` module on `lua`: the value layer, available everywhere.
///
/// Everything here is a pure conversion between values the script already has.
/// Nothing reads the clock and nothing reads the host's zone, which is why all
/// four binaries get it — and why the timestamp parsers demand an explicit
/// offset (see [`register_host_time`] for the half that may guess).
pub fn register(lua: &Lua) -> LuaResult<()> {
    let module = lua.create_table()?;

    // time.parse_rfc3339(str) -> number
    // Explicit: RFC 3339, which means the offset is required. A date with no
    // time, or a time with no offset, is an error here rather than a guess.
    module.set(
        "parse_rfc3339",
        lua.create_function(|_, text: String| {
            let text = text.trim();
            let ts = text.parse::<Timestamp>().map_err(|e| {
                mlua::Error::runtime(format!(
                    "time.parse_rfc3339 could not read '{text}' ({e}); RFC 3339 \
                     requires a full date-time with a UTC offset, like \
                     \"2026-08-10T12:00:00Z\" or \"2026-08-10T14:00:00+02:00\""
                ))
            })?;
            Ok(timestamp_to_secs(ts))
        })?,
    )?;

    // time.parse_rfc2822(str) -> number
    // Explicit: RFC 2822, the email/HTTP spelling -- "Thu, 29 Feb 2024 05:34:45
    // -0500". Pure for the same reason `parse_rfc3339` is: the grammar requires
    // a zone, so nothing is read from the host. The obsolete named zones are
    // accepted (`GMT`, `UT`, and the military letters), which is what makes
    // this read an HTTP `Date`/`Last-Modified`/`Expires` header as well as a
    // mail header -- so there is no separate IMF *parser*, only the printer
    // below, IMF-fixdate being a subset on the way in and a narrowing on the
    // way out.
    module.set(
        "parse_rfc2822",
        lua.create_function(|_, text: String| {
            let text = text.trim();
            let ts = rfc2822::DateTimeParser::new()
                .parse_timestamp(text)
                .map_err(|e| {
                    mlua::Error::runtime(format!(
                        "time.parse_rfc2822 could not read '{text}' ({e}); RFC 2822 \
                         expects a date-time with a zone, like \
                         \"Thu, 29 Feb 2024 05:34:45 -0500\" or \
                         \"Sat, 13 Jul 2024 15:09:59 GMT\""
                    ))
                })?;
            Ok(timestamp_to_secs(ts))
        })?,
    )?;

    // time.parse_strftime(str, pattern) -> number
    // Explicit: exactly this pattern. A pattern that fixes no offset is read as
    // UTC, since the caller named the layout rather than a zone; a pattern with
    // no time component is midnight.
    module.set(
        "parse_strftime",
        lua.create_function(|_, (text, pattern): (String, String)| {
            let text = text.trim();
            let parsed = strtime::parse(&pattern, text).map_err(|e| {
                mlua::Error::runtime(format!(
                    "time.parse_strftime could not read '{text}' with '{pattern}': {e}"
                ))
            })?;
            // An offset in the input wins; otherwise UTC, matching the rest of
            // the shared layer.
            if let Ok(ts) = parsed.to_timestamp() {
                return Ok(timestamp_to_secs(ts));
            }
            let dt = parsed.to_datetime().or_else(|_| {
                parsed.to_date().map(|d| d.at(0, 0, 0, 0)).map_err(|e| {
                    mlua::Error::runtime(format!(
                        "time.parse_strftime read '{text}' with '{pattern}', but it \
                         yields no date: {e}"
                    ))
                })
            })?;
            let ts = Offset::UTC.to_timestamp(dt).map_err(|e| {
                mlua::Error::runtime(format!("time.parse_strftime could not place '{text}': {e}"))
            })?;
            Ok(timestamp_to_secs(ts))
        })?,
    )?;

    // time.format(t, [pattern]) -> string
    // RFC 3339 in UTC by default; a strftime pattern renders UTC fields.
    module.set(
        "format",
        lua.create_function(|_, (t, pattern): (Value, Option<String>)| {
            let secs = TimeArg::from_value(&t, "time.format")?.resolve(None, "time.format")?;
            let ts = secs_to_timestamp(secs, "time.format")?;
            format_at(ts, TimeZone::UTC, pattern.as_deref(), "time.format")
        })?,
    )?;

    // time.format_offset(t, offset, [pattern]) -> string
    // A *fixed* offset, so DST is the caller's problem -- which is the reason
    // `format_tz` exists beside it. Pure: the offset is an argument, not a
    // reading of the host.
    module.set(
        "format_offset",
        lua.create_function(|_, (t, offset, pattern): (Value, String, Option<String>)| {
            let secs = TimeArg::from_value(&t, "time.format_offset")?
                .resolve(None, "time.format_offset")?;
            let ts = secs_to_timestamp(secs, "time.format_offset")?;
            let offset = offset_from(&offset, "time.format_offset")?;
            format_at(
                ts,
                TimeZone::fixed(offset),
                pattern.as_deref(),
                "time.format_offset",
            )
        })?,
    )?;

    // time.format_rfc2822(t) -> string   "Sat, 13 Jul 2024 15:09:59 -0000"
    // The inverse of `parse_rfc2822`, in UTC like the rest of the shared output
    // layer. No pattern argument: the format *is* the argument -- a caller who
    // wants their own layout has `format`, and one who wants a zone has
    // `format_tz`.
    //
    // The zone renders as `-0000`, not `+0000`: RFC 2822 3.3 gives that
    // spelling the meaning "this is UTC, and the sender's own zone is unknown",
    // which is exactly true of a timestamp here -- the offset it was parsed
    // with was absorbed into the instant and is not recoverable. `+0000` would
    // claim the sender was *in* UTC. Receivers treat both as the same instant.
    module.set(
        "format_rfc2822",
        lua.create_function(|_, t: Value| {
            let secs = TimeArg::from_value(&t, "time.format_rfc2822")?
                .resolve(None, "time.format_rfc2822")?;
            let ts = secs_to_timestamp(secs, "time.format_rfc2822")?;
            let mut out = String::new();
            rfc2822::DateTimePrinter::new()
                .print_timestamp(&ts, &mut out)
                .map_err(|e| {
                    mlua::Error::runtime(format!("time.format_rfc2822 could not format: {e}"))
                })?;
            Ok(out)
        })?,
    )?;

    // time.format_imf(t) -> string   "Sat, 13 Jul 2024 15:09:59 GMT"
    // IMF-fixdate: what RFC 9110 requires in an HTTP `Date`, `Expires` or
    // `If-Modified-Since` header. A narrowing of RFC 2822 rather than a
    // different format -- always `GMT`, always a two-digit day, always 29
    // characters -- which is why it is a printer with no matching parser:
    // `parse_rfc2822` already reads what this writes.
    module.set(
        "format_imf",
        lua.create_function(|_, t: Value| {
            let secs =
                TimeArg::from_value(&t, "time.format_imf")?.resolve(None, "time.format_imf")?;
            let ts = secs_to_timestamp(secs, "time.format_imf")?;
            let mut out = String::new();
            rfc2822::DateTimePrinter::new()
                .print_timestamp_rfc9110(&ts, &mut out)
                .map_err(|e| {
                    mlua::Error::runtime(format!("time.format_imf could not format: {e}"))
                })?;
            Ok(out)
        })?,
    )?;

    // time.format_tz(t, zone, [pattern]) -> string
    // A named zone, so the offset follows that zone's DST rules at `t`: Berlin
    // renders +02:00 in summer and +01:00 in winter. Pure -- the zone is an
    // argument and the tzdb is bundled, so every machine agrees.
    module.set(
        "format_tz",
        lua.create_function(|_, (t, zone, pattern): (Value, String, Option<String>)| {
            let secs =
                TimeArg::from_value(&t, "time.format_tz")?.resolve(None, "time.format_tz")?;
            let ts = secs_to_timestamp(secs, "time.format_tz")?;
            let zone = zone_from(&zone, "time.format_tz")?;
            format_at(ts, zone, pattern.as_deref(), "time.format_tz")
        })?,
    )?;

    // time.parse_tz(str, zone) -> number
    // A wall-clock reading *in* `zone` -> an instant: the inverse of
    // `format_tz`, and the explicit form of what `time.parse` does implicitly
    // with the host's zone. An input carrying its own offset keeps it, since
    // that names an instant already.
    //
    // The DST edges resolve rather than raise (jiff's "compatible" strategy, as
    // every other datetime library does): on a fall-back date the repeated hour
    // takes its *first* occurrence, and a time that never happened shifts
    // forward past the gap.
    module.set(
        "parse_tz",
        lua.create_function(|_, (text, zone): (String, String)| {
            let text = text.trim();
            let zone = zone_from(&zone, "time.parse_tz")?;
            // An explicit offset already fixes the instant; the zone would only
            // re-interpret what the input already said.
            if let Ok(ts) = text.parse::<Timestamp>() {
                return Ok(timestamp_to_secs(ts));
            }
            let dt = text
                .parse::<DateTime>()
                .or_else(|_| text.parse::<jiff::civil::Date>().map(|d| d.at(0, 0, 0, 0)))
                .map_err(|_| {
                    mlua::Error::runtime(format!(
                        "time.parse_tz could not read '{text}' as a date or date-time; \
                         expected something like \"2026-08-10T12:00:00\" or \"2026-08-10\""
                    ))
                })?;
            let zoned = zone.to_ambiguous_zoned(dt).compatible().map_err(|e| {
                mlua::Error::runtime(format!("time.parse_tz could not place '{text}': {e}"))
            })?;
            Ok(timestamp_to_secs(zoned.timestamp()))
        })?,
    )?;

    // time.zone_offset(t, zone) -> string
    // The offset `zone` is on at `t`, as "+02:00" -- which is what makes the
    // DST rules inspectable, and feeds straight back into `format_offset`.
    module.set(
        "zone_offset",
        lua.create_function(|_, (t, zone): (Value, String)| {
            let secs =
                TimeArg::from_value(&t, "time.zone_offset")?.resolve(None, "time.zone_offset")?;
            let ts = secs_to_timestamp(secs, "time.zone_offset")?;
            let zone = zone_from(&zone, "time.zone_offset")?;
            // `%:z` renders `+02:00`; jiff's own Display gives the shorter
            // `+02`, which is a less useful thing to hand back to a script.
            let zoned = ts.to_zoned(zone);
            strtime::format("%:z", &zoned).map_err(|e| {
                mlua::Error::runtime(format!("time.zone_offset could not format: {e}"))
            })
        })?,
    )?;

    // time.delta(value) -> number
    // The delta coercion, exposed. A number is seconds already, a string is
    // read by humantime's grammar -- the same reading every delta-taking
    // function in this module does, so there is one answer to "what does
    // '1h30m' mean" rather than one per call site.
    module.set(
        "delta",
        lua.create_function(|_, value: Value| delta_from(&value, "time.delta"))?,
    )?;

    // time.format_delta(value) -> string   "4d 2h 5m 2s"
    // The inverse of `time.delta`, and the only delta spelling there is. Takes
    // the same coercion as everything else, so formatting a duration *string*
    // is a normalisation rather than an error.
    module.set(
        "format_delta",
        lua.create_function(|_, value: Value| {
            Ok(format_delta(delta_from(&value, "time.format_delta")?))
        })?,
    )?;

    // time.round(secs, unit) -> number
    // Truncate a delta toward zero at a unit, so an age reads "173h 44m"
    // instead of trailing into nanoseconds. A number, not a string: rounding
    // and formatting are separate questions, and the result is still a delta to
    // compare or subtract.
    //
    // One return value, deliberately. Returning the remainder too made
    // `time.format_delta(time.round(d, "h"))` silently pass it as
    // format_delta's *second argument* -- the multi-value expansion of a
    // trailing call. `round_with_rest` is where that pair lives now, behind a
    // name that says a second value is coming.
    module.set(
        "round",
        lua.create_function(|_, (value, unit): (Value, String)| {
            let secs = delta_from(&value, "time.round")?;
            Ok(round_delta(secs, round_unit(&unit, "time.round")?).0)
        })?,
    )?;

    // time.round_with_rest(secs, unit) -> number, number
    // `round`, plus what was left below the unit -- for the caller who rounds
    // to the hour and wants the minutes too, since recomputing them means
    // repeating the unit. Both are seconds.
    module.set(
        "round_with_rest",
        lua.create_function(|_, (value, unit): (Value, String)| {
            let secs = delta_from(&value, "time.round_with_rest")?;
            Ok(round_delta(
                secs,
                round_unit(&unit, "time.round_with_rest")?,
            ))
        })?,
    )?;

    // time.diff(a, b) -> number
    // `b - a` in seconds. Both sides go through the timestamp coercion, so
    // either may be a number or a string and the pair need not match.
    module.set(
        "diff",
        lua.create_function(|_, (a, b): (Value, Value)| {
            let a = TimeArg::from_value(&a, "time.diff")?.resolve(None, "time.diff")?;
            let b = TimeArg::from_value(&b, "time.diff")?.resolve(None, "time.diff")?;
            Ok(b - a)
        })?,
    )?;

    // time.add(t, delta) -> number
    // time.sub(t, delta) -> number
    // Calendar-aware: a month or year span anchors at `t`, so adding "P1M" to
    // the 31st lands where the calendar says rather than 30 days later.
    for (name, negate) in [("add", false), ("sub", true)] {
        module.set(
            name,
            lua.create_function(move |_, (t, delta): (Value, Value)| {
                let what = if negate { "time.sub" } else { "time.add" };
                let secs = TimeArg::from_value(&t, what)?.resolve(None, what)?;
                shift(secs, &delta, negate, what)
            })?,
        )?;
    }

    lua.globals().set("time", module)?;
    Ok(())
}

/// Shift a timestamp by a delta, anchoring calendar units at the timestamp.
///
/// Shared by `add`/`sub` and by their host-only rebinding, so the calendar
/// behaviour cannot differ between the two registrations.
fn shift(secs: f64, delta: &Value, negate: bool, what: &str) -> LuaResult<f64> {
    let ts = secs_to_timestamp(secs, what)?;
    match delta {
        // A number is already seconds; no anchoring needed.
        Value::Integer(_) | Value::Number(_) => {
            let d = delta_from(delta, what)?;
            Ok(secs + if negate { -d } else { d })
        }
        Value::String(s) => {
            let text = s.to_str()?;
            let text = text.trim();
            let span = parse_span(text, what)?;
            let span = if negate { -span } else { span };
            // Anchored at `ts` in UTC: this is the whole reason `add` accepts
            // the calendar units the delta parsers reject.
            let zoned = ts.to_zoned(TimeZone::UTC);
            let shifted = zoned.checked_add(span).map_err(|e| {
                mlua::Error::runtime(format!("{what} could not shift by '{text}': {e}"))
            })?;
            Ok(timestamp_to_secs(shifted.timestamp()))
        }
        other => Err(mlua::Error::runtime(format!(
            "{what} expects a delta -- seconds as a number, or a string like \
             \"30s\", \"1h 30min\" or \"1month\" -- got {}",
            other.type_name()
        ))),
    }
}

/// Add the clock-reading and local-zone half of `time` (`ito`/`irun` only).
///
/// Host-only for the reason `os.getenv` and `fs.cwd()` are: each answer depends
/// on ambient state the caller never passed and cannot see, so two runs of one
/// script can disagree. Where the output is a *preview* of what a run would do
/// that is a correctness problem rather than a taste one — an `ied --diff` that
/// read the clock could differ from the `--write` after it.
///
/// Three kinds of thing land here:
///
/// - `time.now()` — Unix seconds, fractional. An age against it is
///   `time.diff(t, time.now())`, spelled out at the call site: a dedicated
///   `since`/`until` pair only moved the subtraction behind a name whose sign
///   convention the reader then had to recall.
/// - `time.parse` and `time.format_local` — the two that may consult the host's
///   **offset**. `parse` is here in full, rather than as a widening of a shared
///   version, because a function that sometimes reads ambient state and
///   sometimes does not would be the worst of both: `iq` keeps the strict
///   `time.parse_rfc3339`, and `if time.parse then` tells a script which it got.
///
/// What is *not* here is the named-zone surface — `format_tz`, `parse_tz` and
/// `zone_offset` are in [`register`], because their zone arrives as an
/// **argument** and resolves against the *bundled* tzdb: same input, same
/// answer, on every machine and every run. The line is where the answer comes
/// from, not whether dates are involved. Which is also why there is
/// deliberately no `zone_offset(t)` defaulting to the host zone and no
/// `local_zone()` naming it — either would smuggle ambient state into a
/// function that looks pure.
///
/// Like the other reach-into-the-stdlib steps in `Engine::build`, this must run
/// before `sandbox(true)` freezes the `time` table.
pub fn register_host_time(lua: &Lua) -> LuaResult<()> {
    let module: Table = lua.globals().get("time")?;

    // time.now() -> number
    module.set(
        "now",
        lua.create_function(|_, ()| Ok(timestamp_to_secs(Timestamp::now())))?,
    )?;

    // time.parse(value) -> number
    // The do-as-I-intend parser: any shape this module understands. An input
    // carrying an offset keeps it; one without is read as *local* time, because
    // a human writing "2026-08-10" means midnight where they are.
    module.set(
        "parse",
        lua.create_function(|_, value: Value| {
            TimeArg::from_value(&value, "time.parse")?.resolve(Some(local_offset()), "time.parse")
        })?,
    )?;

    // time.format_local(t, [pattern]) -> string
    // As `time.format`, in the host's offset: RFC 3339 with the real offset
    // (`+02:00`) rather than `Z`, or a strftime pattern over local fields.
    module.set(
        "format_local",
        lua.create_function(|_, (t, pattern): (Value, Option<String>)| {
            let secs = TimeArg::from_value(&t, "time.format_local")?
                .resolve(Some(local_offset()), "time.format_local")?;
            let ts = secs_to_timestamp(secs, "time.format_local")?;
            format_at(
                ts,
                TimeZone::fixed(local_offset()),
                pattern.as_deref(),
                "time.format_local",
            )
        })?,
    )?;

    // The shared `add`/`sub`/`diff`/`format` reject offset-less strings, having
    // no zone to place them in. Here there is one, so they are rebound to the
    // same functions with the local offset supplied -- one implementation, one
    // extra input.
    module.set(
        "diff",
        lua.create_function(|_, (a, b): (Value, Value)| {
            let a =
                TimeArg::from_value(&a, "time.diff")?.resolve(Some(local_offset()), "time.diff")?;
            let b =
                TimeArg::from_value(&b, "time.diff")?.resolve(Some(local_offset()), "time.diff")?;
            Ok(b - a)
        })?,
    )?;
    for (name, negate) in [("add", false), ("sub", true)] {
        module.set(
            name,
            lua.create_function(move |_, (t, delta): (Value, Value)| {
                let what = if negate { "time.sub" } else { "time.add" };
                let secs = TimeArg::from_value(&t, what)?.resolve(Some(local_offset()), what)?;
                shift(secs, &delta, negate, what)
            })?,
        )?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `Lua` with the shared layer only, as `iq`/`ied` see it.
    fn value_only() -> Lua {
        let lua = Lua::new();
        register(&lua).unwrap();
        lua
    }

    /// A `Lua` with both layers, as `ito`/`irun` see it.
    fn with_host() -> Lua {
        let lua = value_only();
        register_host_time(&lua).unwrap();
        lua
    }

    #[test]
    fn rfc3339_absorbs_the_offset_into_one_instant() {
        let lua = value_only();
        let z: f64 = lua
            .load(r#"return time.parse_rfc3339("2026-08-10T12:00:00Z")"#)
            .eval()
            .unwrap();
        let plus: f64 = lua
            .load(r#"return time.parse_rfc3339("2026-08-10T14:00:00+02:00")"#)
            .eval()
            .unwrap();
        assert_eq!(z, plus, "the same instant in two spellings");
        assert_eq!(z, 1786363200.0);
    }

    #[test]
    fn rfc3339_requires_an_offset() {
        let lua = value_only();
        let err = lua
            .load(r#"return time.parse_rfc3339("2026-08-10T12:00:00")"#)
            .eval::<f64>()
            .unwrap_err()
            .to_string();
        assert!(err.contains("RFC 3339 requires"), "{err}");
    }

    #[test]
    fn format_round_trips_rfc3339_through_utc() {
        let lua = value_only();
        let s: String = lua
            .load(r#"return time.format(time.parse_rfc3339("2026-08-10T14:00:00+02:00"))"#)
            .eval()
            .unwrap();
        // The offset is absorbed: an instant formats as `Z`, not `+02:00`.
        assert_eq!(s, "2026-08-10T12:00:00Z");
    }

    #[test]
    fn rfc2822_round_trips_through_utc() {
        let lua = value_only();
        let t: f64 = lua
            .load(r#"return time.parse_rfc2822("Thu, 29 Feb 2024 05:34:45 -0500")"#)
            .eval()
            .unwrap();
        // The offset is absorbed, exactly as RFC 3339 parsing absorbs it.
        let back: String = lua
            .load(format!("return time.format_rfc2822({t})"))
            .eval()
            .unwrap();
        // `-0000` is RFC 2822's "UTC, sender's zone unknown" -- which is what a
        // timestamp that absorbed its offset can honestly claim.
        assert_eq!(back, "Thu, 29 Feb 2024 10:34:45 -0000");
        let iso: String = lua.load(format!("return time.format({t})")).eval().unwrap();
        assert_eq!(iso, "2024-02-29T10:34:45Z");
    }

    #[test]
    fn rfc2822_reads_an_http_date_header() {
        let lua = value_only();
        // `GMT` is one of RFC 2822's obsolete zone names, which is what lets
        // this same function read an HTTP `Date`/`Last-Modified`.
        let t: f64 = lua
            .load(r#"return time.parse_rfc2822("Sat, 13 Jul 2024 15:09:59 GMT")"#)
            .eval()
            .unwrap();
        let iso: String = lua.load(format!("return time.format({t})")).eval().unwrap();
        assert_eq!(iso, "2024-07-13T15:09:59Z");
    }

    #[test]
    fn imf_narrows_rfc2822_for_http() {
        let lua = value_only();
        // A single-digit day: IMF pads it and names the zone `GMT`, where the
        // RFC 2822 spelling does neither.
        let t: f64 = lua
            .load(r#"return time.parse_rfc3339("2024-02-05T05:34:45Z")"#)
            .eval()
            .unwrap();
        let imf: String = lua
            .load(format!("return time.format_imf({t})"))
            .eval()
            .unwrap();
        assert_eq!(imf, "Mon, 05 Feb 2024 05:34:45 GMT");
        assert_eq!(imf.len(), 29, "IMF-fixdate is a fixed 29 characters");
        let rfc: String = lua
            .load(format!("return time.format_rfc2822({t})"))
            .eval()
            .unwrap();
        assert_eq!(rfc, "Mon, 5 Feb 2024 05:34:45 -0000");
        // ...and what the printer emits, the parser reads back.
        let round: f64 = lua
            .load(format!("return time.parse_rfc2822({imf:?})"))
            .eval()
            .unwrap();
        assert_eq!(round, t);
    }

    #[test]
    fn rfc2822_rejects_an_iso_timestamp() {
        let lua = value_only();
        let err = lua
            .load(r#"return time.parse_rfc2822("2026-08-10T12:00:00Z")"#)
            .eval::<f64>()
            .unwrap_err()
            .to_string();
        assert!(err.contains("RFC 2822 expects"), "{err}");
    }

    #[test]
    fn parse_does_not_sniff_rfc2822() {
        // The convenience form reads the ISO-shaped inputs only; an email date
        // is a format a script asks for by name.
        let lua = with_host();
        let err = lua
            .load(r#"return time.parse("Thu, 29 Feb 2024 05:34:45 -0500")"#)
            .eval::<f64>()
            .unwrap_err()
            .to_string();
        assert!(!err.is_empty());
    }

    #[test]
    fn format_takes_a_strftime_pattern() {
        let lua = value_only();
        let s: String = lua
            .load(r#"return time.format(1786363200, "%Y-%m-%d %H:%M")"#)
            .eval()
            .unwrap();
        assert_eq!(s, "2026-08-10 12:00");
    }

    #[test]
    fn parse_strftime_reads_exactly_its_pattern() {
        let lua = value_only();
        let t: f64 = lua
            .load(r#"return time.parse_strftime("10.08.2026", "%d.%m.%Y")"#)
            .eval()
            .unwrap();
        assert_eq!(t, 1786320000.0, "midnight UTC");

        let err = lua
            .load(r#"return time.parse_strftime("2026-08-10", "%d.%m.%Y")"#)
            .eval::<f64>()
            .unwrap_err()
            .to_string();
        assert!(err.contains("could not read"), "{err}");
    }

    #[test]
    fn one_grammar_reads_every_delta_spelling() {
        let lua = value_only();
        for src in [r#""1h 30min""#, r#""1h30m""#, r#""90m""#, "5400"] {
            let secs: f64 = lua
                .load(format!("return time.delta({src})"))
                .eval()
                .unwrap();
            assert_eq!(secs, 5400.0, "{src}");
        }
    }

    /// The delta surface is the *only* one: `add`/`round`/`format_delta` all
    /// read a duration string through `time.delta`, so none can disagree.
    #[test]
    fn every_delta_taker_shares_the_one_coercion() {
        let lua = value_only();
        let same: bool = lua
            .load(
                r#"return time.round("1h30m", "m") == time.delta("1h30m")
                   and time.add(0, "1h30m") == time.delta("1h30m")
                   and time.format_delta("1h30m") == time.format_delta(5400)"#,
            )
            .eval()
            .unwrap();
        assert!(same);
    }

    #[test]
    fn a_bare_number_in_a_string_is_rejected_with_the_fix() {
        let lua = value_only();
        let err = lua
            .load(r#"return time.delta("1200")"#)
            .eval::<f64>()
            .unwrap_err()
            .to_string();
        assert!(err.contains("needs a unit"), "{err}");
        // ...while the number itself means seconds.
        let secs: f64 = lua.load("return time.delta(1200)").eval().unwrap();
        assert_eq!(secs, 1200.0);
    }

    #[test]
    fn fixed_units_are_deltas_but_calendar_units_are_not() {
        let lua = value_only();
        let day: f64 = lua.load(r#"return time.delta("1d")"#).eval().unwrap();
        assert_eq!(day, 86400.0, "a day is 24h here");
        let week: f64 = lua.load(r#"return time.delta("1w")"#).eval().unwrap();
        assert_eq!(week, 604800.0);

        // A month or year has no length until a date anchors it, so the delta
        // reader refuses; `add`/`sub` take them, having a timestamp.
        for src in [r#""1month""#, r#""1year""#] {
            let err = lua
                .load(format!("return time.delta({src})"))
                .eval::<f64>()
                .unwrap_err()
                .to_string();
            assert!(err.contains("time.add"), "{src}: {err}");
        }
    }

    #[test]
    fn add_anchors_calendar_units_at_the_timestamp() {
        let lua = value_only();
        // January 31 + P1M is the end of February, not 30 days later.
        let s: String = lua
            .load(
                r#"return time.format(time.add(time.parse_rfc3339("2026-01-31T00:00:00Z"), "P1M"))"#,
            )
            .eval()
            .unwrap();
        assert_eq!(s, "2026-02-28T00:00:00Z");
    }

    #[test]
    fn add_and_sub_are_inverses_on_fixed_units() {
        let lua = value_only();
        let round: f64 = lua
            .load(r#"return time.sub(time.add(1786363200, "1h"), "1h")"#)
            .eval()
            .unwrap();
        assert_eq!(round, 1786363200.0);
    }

    #[test]
    fn diff_mixes_the_two_spellings_on_either_side() {
        let lua = value_only();
        let d: f64 = lua
            .load(r#"return time.diff("2026-08-10T12:00:00Z", 1786363200 + 90)"#)
            .eval()
            .unwrap();
        assert_eq!(d, 90.0);
    }

    /// Days are a unit here, which is the whole point of the hand-rolled
    /// formatter: jiff's calendar-less duration stops at the hour and would
    /// render four days as `96h`.
    #[test]
    fn format_delta_decomposes_down_from_days() {
        let lua = value_only();
        for (secs, want) in [
            (5400.0, "1h 30m"),
            (353102.0, "4d 2h 5m 2s"),
            (345600.0, "4d"),
            (90.0, "1m 30s"),
            (0.0, "0s"),
            (0.5, "0s"),
        ] {
            let s: String = lua
                .load(format!("return time.format_delta({secs})"))
                .eval()
                .unwrap();
            assert_eq!(s, want, "{secs}");
        }
    }

    /// The formatter's output is a duration string, so it reads back.
    #[test]
    fn format_delta_round_trips_through_delta() {
        let lua = value_only();
        for secs in [5400.0, 353102.0, 345600.0, 90.0, 1.0] {
            let back: f64 = lua
                .load(format!("return time.delta(time.format_delta({secs}))"))
                .eval()
                .unwrap();
            assert_eq!(back, secs, "{secs}");
        }
    }

    /// Durations are unsigned, but subtraction is not: the printer prepends a
    /// `-` rather than refusing a value the caller's arithmetic produced.
    #[test]
    fn a_negative_delta_prints_with_a_leading_minus() {
        let lua = value_only();
        let s: String = lua.load("return time.format_delta(-90)").eval().unwrap();
        assert_eq!(s, "-1m 30s");
        // ...but it is not a spelling anyone can write.
        assert!(
            lua.load(r#"return time.delta("-90s")"#)
                .eval::<f64>()
                .is_err()
        );
    }

    #[test]
    fn round_truncates_toward_zero_and_returns_one_value() {
        let lua = value_only();
        for (src, want) in [
            ("time.round(90, \"m\")", 60.0),
            ("time.round(-90, \"m\")", -60.0),
            ("time.round(119, \"m\")", 60.0),
            ("time.round(7265.9, \"s\")", 7265.0),
            ("time.round(7265.9, \"h\")", 7200.0),
            ("time.round(180000, \"d\")", 172800.0),
        ] {
            let got: f64 = lua.load(format!("return {src}")).eval().unwrap();
            assert_eq!(got, want, "{src}");
        }
    }

    /// `round` returning one value is what makes it compose into a formatter:
    /// a second return would arrive as `format_delta`'s second *argument*.
    #[test]
    fn round_composes_into_format_delta() {
        let lua = value_only();
        let s: String = lua
            .load(r#"return time.format_delta(time.round(time.delta("4d2h5m2s"), "h"))"#)
            .eval()
            .unwrap();
        assert_eq!(s, "4d 2h");
    }

    #[test]
    fn round_with_rest_returns_the_part_below_the_unit() {
        let lua = value_only();
        for (src, want, rest) in [
            ("time.round_with_rest(90, \"m\")", 60.0, 30.0),
            ("time.round_with_rest(-90, \"m\")", -60.0, -30.0),
            ("time.round_with_rest(7265.9, \"h\")", 7200.0, 65.0),
            ("time.round_with_rest(180000, \"d\")", 172800.0, 7200.0),
        ] {
            let (got, got_rest): (f64, f64) = lua.load(format!("return {src}")).eval().unwrap();
            assert_eq!((got, got_rest), (want, rest), "{src}");
            // The two halves reconstruct the delta, truncated to seconds.
            assert_eq!(got + got_rest, (want + rest).trunc(), "{src}");
        }
    }

    #[test]
    fn the_second_is_the_display_resolution() {
        let lua = value_only();
        // The reason any of this exists: a fractional delta formats without a
        // sub-second tail, with no rounding at the call site.
        let s: String = lua
            .load("return time.format_delta(625464.737129927)")
            .eval()
            .unwrap();
        assert_eq!(s, "7d 5h 44m 24s");
        let s: String = lua
            .load(r#"return time.format_delta(time.round(625464.737129927, "m"))"#)
            .eval()
            .unwrap();
        assert_eq!(s, "7d 5h 44m");

        // Below the second is not a resolution this module has; above the week
        // has no fixed length.
        for unit in ["ms", "month", "y"] {
            let err = lua
                .load(format!(r#"return time.round(5400, "{unit}")"#))
                .eval::<f64>()
                .unwrap_err()
                .to_string();
            assert!(err.contains("does not know the unit"), "{unit}: {err}");
        }
    }

    #[test]
    fn the_clock_half_is_absent_without_host_io() {
        let lua = value_only();
        let missing: bool = lua
            .load(
                "return time.now == nil and time.parse == nil \
                 and time.format_local == nil",
            )
            .eval()
            .unwrap();
        assert!(missing, "the ambient half must not leak into iq/ied");
        // ...and the value half is all there.
        let present: bool = lua
            .load(
                "return time.parse_rfc3339 ~= nil and time.delta ~= nil \
                 and time.parse_rfc2822 ~= nil and time.format_rfc2822 ~= nil \
                 and time.format_imf ~= nil",
            )
            .eval()
            .unwrap();
        assert!(present);
    }

    // --- fixed offsets and named zones: pure, so available everywhere ---

    #[test]
    fn format_offset_renders_a_fixed_offset() {
        let lua = value_only();
        let s: String = lua
            .load(r#"return time.format_offset(1786363200, "+05:30")"#)
            .eval()
            .unwrap();
        assert_eq!(s, "2026-08-10T17:30:00+05:30");
        // `Z` and `UTC` are accepted, being what a caller reaches for first.
        let z: String = lua
            .load(r#"return time.format_offset(1786363200, "Z")"#)
            .eval()
            .unwrap();
        assert_eq!(z, "2026-08-10T12:00:00Z");
    }

    /// The point of `format_tz` over `format_offset`: the offset follows the
    /// zone's DST rules instead of being asserted by the caller.
    #[test]
    fn format_tz_follows_dst() {
        let lua = value_only();
        let summer: String = lua
            .load(r#"return time.format_tz(1786363200, "Europe/Berlin")"#)
            .eval()
            .unwrap();
        let winter: String = lua
            .load(
                r#"return time.format_tz(time.parse_rfc3339("2026-01-10T12:00:00Z"),
                                          "Europe/Berlin")"#,
            )
            .eval()
            .unwrap();
        assert_eq!(summer, "2026-08-10T14:00:00+02:00");
        assert_eq!(winter, "2026-01-10T13:00:00+01:00");
    }

    #[test]
    fn zone_offset_reports_the_rule_in_effect() {
        let lua = value_only();
        let (summer, winter): (String, String) = lua
            .load(
                r#"return time.zone_offset(1786363200, "Europe/Berlin"),
                          time.zone_offset(time.parse_rfc3339("2026-01-10T12:00:00Z"),
                                           "Europe/Berlin")"#,
            )
            .eval()
            .unwrap();
        assert_eq!((summer.as_str(), winter.as_str()), ("+02:00", "+01:00"));
    }

    /// `zone_offset` feeds `format_offset`, which is why it returns the `+HH:MM`
    /// spelling rather than jiff's shorter `+02`.
    #[test]
    fn zone_offset_output_is_accepted_by_format_offset() {
        let lua = value_only();
        let s: String = lua
            .load(
                r#"local t = time.parse_rfc3339("2026-01-10T12:00:00Z")
                   return time.format_offset(t, time.zone_offset(t, "Europe/Berlin"))"#,
            )
            .eval()
            .unwrap();
        assert_eq!(s, "2026-01-10T13:00:00+01:00");
    }

    #[test]
    fn parse_tz_is_the_inverse_of_format_tz() {
        let lua = value_only();
        let s: String = lua
            .load(
                r#"local t = time.parse_tz("2026-08-10T14:00:00", "Europe/Berlin")
                   return time.format(t) .. " " .. time.format_tz(t, "Europe/Berlin")"#,
            )
            .eval()
            .unwrap();
        assert_eq!(s, "2026-08-10T12:00:00Z 2026-08-10T14:00:00+02:00");
    }

    /// An input that already names an instant is not re-interpreted by the zone.
    #[test]
    fn parse_tz_keeps_an_explicit_offset() {
        let lua = value_only();
        let t: f64 = lua
            .load(r#"return time.parse_tz("2026-08-10T12:00:00Z", "Asia/Tokyo")"#)
            .eval()
            .unwrap();
        assert_eq!(t, 1786363200.0);
    }

    /// The DST edges resolve rather than raise, as documented: the repeated hour
    /// takes its first occurrence, and a time that never happened shifts past
    /// the gap.
    #[test]
    fn parse_tz_resolves_the_dst_edges() {
        let lua = value_only();
        let (fall, spring): (String, String) = lua
            .load(
                r#"local f = time.parse_tz("2026-10-25T02:30:00", "Europe/Berlin")
                   local s = time.parse_tz("2026-03-29T02:30:00", "Europe/Berlin")
                   return time.format_tz(f, "Europe/Berlin"), time.format_tz(s, "Europe/Berlin")"#,
            )
            .eval()
            .unwrap();
        assert_eq!(fall, "2026-10-25T02:30:00+02:00", "the earlier of the two");
        assert_eq!(spring, "2026-03-29T03:30:00+02:00", "shifted past the gap");
    }

    /// The two argument kinds are not interchangeable, and saying which is
    /// wrong beats a tzdb-lookup error or a silently different answer.
    #[test]
    fn offsets_and_zone_names_are_not_confused_for_each_other() {
        let lua = value_only();
        let err = lua
            .load(r#"return time.format_offset(1, "Europe/Berlin")"#)
            .eval::<String>()
            .unwrap_err()
            .to_string();
        assert!(err.contains("time.format_tz"), "{err}");

        let err = lua
            .load(r#"return time.format_tz(1, "+02:00")"#)
            .eval::<String>()
            .unwrap_err()
            .to_string();
        assert!(err.contains("time.format_offset"), "{err}");

        let err = lua
            .load(r#"return time.format_tz(1, "Mars/Olympus")"#)
            .eval::<String>()
            .unwrap_err()
            .to_string();
        assert!(err.contains("does not know the time zone"), "{err}");
    }

    /// An offset or zone where a pattern belongs used to pass through as
    /// literal text, since a pattern with no `%` formats to itself -- a
    /// plausible-looking wrong answer, which is worse than an error.
    #[test]
    fn an_offset_where_a_pattern_belongs_is_rejected() {
        let lua = value_only();
        for (call, hint) in [
            (r#"time.format(1, "+05:30")"#, "time.format_offset"),
            (r#"time.format(1, "Europe/Berlin")"#, "time.format_tz"),
        ] {
            let err = lua
                .load(format!("return {call}"))
                .eval::<String>()
                .unwrap_err()
                .to_string();
            assert!(err.contains(hint), "{call}: {err}");
        }
        // A real pattern still works, including one with no `%` conversions
        // that could not be mistaken for an offset or a zone.
        let s: String = lua
            .load(r#"return time.format(1786363200, "%Y")"#)
            .eval()
            .unwrap();
        assert_eq!(s, "2026");
    }

    /// The named-zone surface is **pure** -- the zone is an argument and the
    /// tzdb is bundled -- so it must be present where ambient state is not.
    /// This is the boundary, asserted rather than assumed.
    #[test]
    fn named_zones_are_available_without_host_io() {
        let lua = value_only();
        let present: bool = lua
            .load(
                "return time.format_tz ~= nil and time.parse_tz ~= nil \
                 and time.zone_offset ~= nil and time.format_offset ~= nil",
            )
            .eval()
            .unwrap();
        assert!(present, "a named zone is a value, not ambient state");

        // ...and nothing here reads the host: no zone-less overload that would
        // default to it, and no way to ask what the host's zone is.
        let no_ambient: bool = lua
            .load(
                "return pcall(time.zone_offset, 0) == false \
                 and time.local_zone == nil and time.format_local == nil",
            )
            .eval()
            .unwrap();
        assert!(
            no_ambient,
            "no ambient reading may hide in the zone surface"
        );
    }

    #[test]
    fn parse_guesses_only_under_host_io() {
        let lua = with_host();
        // An offset-bearing input does not depend on the host zone, so its
        // value is pinned.
        let t: f64 = lua
            .load(r#"return time.parse("2026-08-10T12:00:00Z")"#)
            .eval()
            .unwrap();
        assert_eq!(t, 1786363200.0);

        // An offset-less one is read as local: the same wall-clock reading,
        // shifted by whatever the host's offset is.
        let naive: f64 = lua
            .load(r#"return time.parse("2026-08-10T12:00:00")"#)
            .eval()
            .unwrap();
        let expected = 1786363200.0 - f64::from(local_offset().seconds());
        assert_eq!(naive, expected);
    }

    #[test]
    fn a_bare_date_is_midnight() {
        let lua = with_host();
        let date: f64 = lua
            .load(r#"return time.parse("2026-08-10")"#)
            .eval()
            .unwrap();
        let noon: f64 = lua
            .load(r#"return time.parse("2026-08-10T12:00:00")"#)
            .eval()
            .unwrap();
        assert_eq!(noon - date, 43200.0, "midnight is 12h before noon");
    }

    /// `since`/`until` are gone; an age is the subtraction, spelled out. The
    /// direction is visible at the call site instead of living in a name.
    #[test]
    fn an_age_is_a_diff_against_now() {
        let lua = with_host();
        let gone: bool = lua
            .load("return time.since == nil and time['until'] == nil")
            .eval()
            .unwrap();
        assert!(gone, "the since/until pair was dropped");

        let age: f64 = lua
            .load(
                "local t = time.now() - 60 \
                 return time.diff(t, time.now())",
            )
            .eval()
            .unwrap();
        assert!((age - 60.0).abs() < 5.0, "a minute-old instant: {age}");
    }

    #[test]
    fn format_local_carries_a_real_offset() {
        let lua = with_host();
        let s: String = lua
            .load("return time.format_local(1786363200)")
            .eval()
            .unwrap();
        // Either the `Z` spelling (a UTC host) or a numeric offset, never a
        // bare local time with no offset at all.
        assert!(
            s.ends_with('Z') || s.contains('+') || s[10..].contains('-'),
            "{s}"
        );
    }

    #[test]
    fn now_is_fractional_seconds_near_the_epoch_scale() {
        let lua = with_host();
        let now: f64 = lua.load("return time.now()").eval().unwrap();
        // Sanity, not a clock test: past 2020 and before 2100.
        assert!(now > 1_577_836_800.0 && now < 4_102_444_800.0, "{now}");
    }
}
