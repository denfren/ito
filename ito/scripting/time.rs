//! `time` module: timestamps and deltas as plain numbers.
//!
//! Two representations, both ordinary Luau numbers, so `<` compares and `+`
//! arithmetic works without unwrapping anything and a value survives a trip
//! through `json`:
//!
//! - a **timestamp** is Unix seconds — what `os.time()` returns, but with a
//!   fractional part when the source had one;
//! - a **delta** is a number of seconds (written unsigned, see below).
//!
//! # Naming: `parse_<format>[_<zone>]` / `format_<format>[_<zone>]`
//!
//! Every parser and formatter names its **format** first, then — where the
//! format leaves the zone open — how the zone is resolved:
//!
//! - no zone suffix: the format carries its own zone (RFC 3339's offset, RFC
//!   2822's zone letters/offset). Nothing to resolve, nothing to choose.
//! - `_utc`: the value carries **no** zone; read/write it in UTC.
//! - `_local`: the value carries **no** zone; read/write it in the host's
//!   time zone, with that zone's DST rules at the instant in question.
//!   Host-only, since the host zone is ambient state.
//! - `_tz`: the value carries **no** zone; an extra `zone` argument names an
//!   IANA zone (`"Europe/Berlin"`) to read/write it in, DST-correct for the
//!   instant in question.
//!
//! A format only gets the suffixes that are *reasonable for it in the wild*.
//! RFC 3339 and RFC 2822 both carry their own zone syntactically, so each has
//! exactly one **parser** and needs no zone suffix there — a `_utc` variant
//! would just be a redundant assertion on a zone the string already names.
//! Their **formatters** do have to pick a zone: `format_rfc3339_{utc,tz,local}`
//! (`format_rfc2822`/`format_imf` are always UTC). `civil` (a bare
//! `"2026-08-10"` or `"2026-08-10T12:00:00"`, no offset in the grammar) and
//! `fmt` (a caller-supplied strftime pattern) get the full `_utc`/`_local`/
//! `_tz` family in both directions. When in doubt, a suffix is dropped rather
//! than added — see the per-format sections below for what's missing and why.
//! A `parse_fmt_*` pattern that reads an offset (`%z`) is rejected, since the
//! zone argument would answer for nothing.
//!
//! ```lua
//! time.parse_rfc3339("2026-08-10T12:00:00Z")           -- offset is in the string
//! time.parse_civil_utc("2026-08-10T12:00:00")          -- read as UTC
//! time.parse_civil_tz("2026-08-10T12:00:00", "Europe/Berlin")
//! time.parse_fmt_utc("10.08.2026", "%d.%m.%Y")
//! ```
//!
//! `_local` variants exist only under `ito` (see [`register_host_time`]):
//! the local offset is ambient state, so `ied` don't have
//! `parse_civil_local`/`format_fmt_local` at all — `if time.parse_civil_local
//! then` is a feature test.
//!
//! # RFC 2822, and HTTP dates
//!
//! Email headers (`Date:`) and HTTP ones (`Date`, `Last-Modified`, `Expires`)
//! carry the RFC 2822 spelling:
//!
//! ```lua
//! time.parse_rfc2822("Sat, 13 Jul 2024 15:09:59 GMT")  -- accepts obsolete letter zones too
//! time.format_rfc2822(t)  --> "Sat, 13 Jul 2024 15:09:59 -0000"
//! time.format_imf(t)      --> "Sat, 13 Jul 2024 15:09:59 GMT"
//! ```
//!
//! `parse_rfc2822` reads **both** zone spellings RFC 2822 allows — a numeric
//! offset and the obsolete letter zones (`GMT`, `UT`, the military letters)
//! — in one function rather than two, because a real header may use either
//! and a receiver has to accept both; splitting would just make the common
//! case call one parser, catch its failure, and try the other. There is no
//! matching split on the *format* side either, but for a different reason:
//! jiff's RFC 2822 printer only ever emits a numeric offset, so there is no
//! letter-zone printer to pair one with. `format_imf` is the IMF-fixdate of
//! RFC 9110 — always `GMT`, always a two-digit day, always 29 characters —
//! which is what an HTTP header must contain and is kept as its own function
//! rather than a suffix, there being no second IMF spelling to pick between.
//! There is no `parse_imf`: IMF-fixdate is a *subset* going in, and
//! `parse_rfc2822` already reads every header `format_imf` emits.
//!
//! These are **pure**, and so live in all three binaries: RFC 2822 requires a
//! zone just as RFC 3339 requires an offset, so neither direction consults
//! the host.
//!
//! # Deltas
//!
//! A delta is **a number of seconds**. `parse_delta`/`format_delta` are the
//! one coercion pair — every function that takes or produces a delta calls
//! them, so `time.add_utc`, `time.count` and `task.wait` cannot disagree about
//! what a duration string means. `add_*`/`sub_*` read the same grammar plus
//! months and years.
//!
//! ```lua
//! time.parse_delta("1h 30min")   -- 5400
//! time.parse_delta("1h30m")      -- 5400   spaces are optional
//! time.parse_delta("4d2h5m2s")   -- 353102
//! time.parse_delta(5400)         -- 5400   already seconds
//! time.parse_delta("1200")       -- error: a bare number in a string has no unit
//!
//! time.format_delta(5400)     -- "1h 30m"
//! time.format_delta(353102)   -- "4d 2h 5m 2s"
//! ```
//!
//! **Deltas are durations, so they do not carry a sign as input.** The parser
//! reads magnitudes only. Subtraction can still land below zero, and
//! [`format_delta`] renders such a value with a leading `-` rather than
//! refusing it — the arithmetic is the caller's, and printing it is not the
//! place to argue.
//!
//! **A day is a fixed 24 hours everywhere** — `"1d"`, `86400` and
//! `time.days(1)` are the same delta, including in `add_tz`/`add_local`. Only
//! months and years are calendar units: a month has no length until you say
//! which month, so [`parse_delta`] rejects them and `add_*`/`sub_*` accept
//! them (`"1M"`, `"1month"`, `"2y"`), where the timestamp anchors them and the
//! result is calendar-correct.
//!
//! # Units
//!
//! `time.minutes(n)`/`hours(n)`/`days(n)` build a delta from a count —
//! `time.hours(2)` is `7200`, the same number `parse_delta("2h")` gives, only
//! without a string in the middle. `time.truncate(secs, unit)` floors a delta
//! to whole units, in seconds; `time.count(secs, unit)` does the same division
//! but returns the unit count instead of the seconds it represents.
//!
//! ```lua
//! time.hours(2)                  -- 7200
//! time.truncate(7500, "h")       -- 7200   (2 whole hours, in seconds)
//! time.count(7500, "h")          -- 2      (2 whole hours)
//! ```
//!
//! # `till`/`ago`
//!
//! `time.till(t)` is `t - time.now()`, `time.ago(t)` is `time.now() - t` —
//! both host-only, existing purely for readability at the call site: a
//! reader does not have to recall a subtraction's argument order to know which
//! direction "3600" means.

use jiff::civil::DateTime;
use jiff::fmt::rfc2822;
use jiff::fmt::strtime;
use jiff::tz::TimeZone;
use jiff::{Span, Timestamp};
use mlua::{Lua, Result as LuaResult, Table, Value};

/// A timestamp argument, in whichever spelling the script used.
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

    /// Unix seconds, treating a string as already carrying its own zone
    /// (RFC 3339, or a compact/space-separated variant of it).
    fn to_secs_zoned(self, what: &str) -> LuaResult<f64> {
        match self {
            TimeArg::Unix(secs) => Ok(secs),
            // RFC 3339 first, then RFC 2822 (an HTTP/mail header date).
            TimeArg::Text(text) => text
                .parse::<Timestamp>()
                .or_else(|e| {
                    rfc2822::DateTimeParser::new()
                        .parse_timestamp(&text)
                        .map_err(|_| e)
                })
                .map(timestamp_to_secs)
                .map_err(|e| {
                    mlua::Error::runtime(format!(
                        "{what} could not read '{text}' as a timestamp with its own \
                         offset ({e}); expected RFC 3339 like \"2026-08-10T12:00:00Z\" \
                         or RFC 2822"
                    ))
                }),
        }
    }
}

/// Parse a **civil** (offset-less) date or date-time, and place it in `zone`.
///
/// A "civil" value is the wall-clock grammar with no zone in it at all: a
/// full date-time (`"2026-08-10T12:00:00"`) or a bare date
/// (`"2026-08-10"`, read as midnight). If the text *does* carry an offset,
/// that is a caller mistake this rejects rather than silently accepts and
/// ignores -- `zone` would then answer for nothing.
fn parse_civil(text: &str, zone: &TimeZone, what: &str) -> LuaResult<f64> {
    if text.parse::<Timestamp>().is_ok() {
        return Err(mlua::Error::runtime(format!(
            "{what} expects '{text}' to carry no UTC offset -- it already has one. \
             Use time.parse_rfc3339 to read a timestamp that names its own zone."
        )));
    }
    let dt = text
        .parse::<DateTime>()
        .or_else(|_| text.parse::<jiff::civil::Date>().map(|d| d.at(0, 0, 0, 0)))
        .map_err(|_| {
            mlua::Error::runtime(format!(
                "{what} could not read '{text}' as a date or date-time; expected \
                 something like \"2026-08-10T12:00:00\" or \"2026-08-10\""
            ))
        })?;
    let zoned = zone
        .to_ambiguous_zoned(dt)
        .compatible()
        .map_err(|e| mlua::Error::runtime(format!("{what} could not place '{text}': {e}")))?;
    Ok(timestamp_to_secs(zoned.timestamp()))
}

/// Unix seconds for an instant, keeping sub-second precision.
fn timestamp_to_secs(ts: Timestamp) -> f64 {
    ts.as_second() as f64 + f64::from(ts.subsec_nanosecond()) / 1e9
}

/// An instant from Unix seconds, splitting the fraction off so a negative
/// timestamp truncates toward the epoch rather than away from it.
fn secs_to_timestamp(secs: f64, what: &str) -> LuaResult<Timestamp> {
    let mut whole = secs.trunc() as i64;
    let mut nanos = ((secs - secs.trunc()) * 1e9).round() as i32;
    // Rounding can reach a whole second (0.9999999999); carry it instead of
    // handing jiff an out-of-range nanosecond part.
    if nanos.abs() >= 1_000_000_000 {
        let carry = i64::from(nanos.signum());
        whole += carry;
        nanos -= (carry * 1_000_000_000) as i32;
    }
    Timestamp::new(whole, nanos)
        .map_err(|e| mlua::Error::runtime(format!("{what}: {secs} is not a valid timestamp ({e})")))
}

/// Read a delta from a number of seconds or a duration string.
///
/// The *one* delta coercion: every function taking a delta calls this, so
/// `time.add_utc`, `time.truncate` and `task.wait` cannot disagree about what
/// a duration string means. A number is seconds; a string goes through
/// humantime's grammar, which has no notion of a month or year -- units with
/// no fixed length in seconds. `add_*`/`sub_*` take those separately, having
/// a timestamp to anchor them.
pub(crate) fn parse_delta_value(value: &Value, what: &str) -> LuaResult<f64> {
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

/// Split a duration string into whole calendar months (years count as 12) and
/// the remaining fixed-length text, for `add_*`/`sub_*`.
///
/// Anything it cannot tokenize comes back untouched with zero months, so the
/// fixed grammar (humantime) produces the error.
fn split_calendar(text: &str) -> (i64, String) {
    let mut months = 0i64;
    let mut rest = Vec::new();
    let mut chars = text.trim().chars().peekable();
    loop {
        while chars.next_if(|c| c.is_whitespace()).is_some() {}
        let mut num = String::new();
        while let Some(c) = chars.next_if(|c| c.is_ascii_digit() || *c == '.') {
            num.push(c);
        }
        let mut unit = String::new();
        while let Some(c) = chars.next_if(|c| c.is_ascii_alphabetic()) {
            unit.push(c);
        }
        if num.is_empty() && unit.is_empty() {
            return if chars.peek().is_none() {
                (months, rest.join(" "))
            } else {
                (0, text.trim().to_string())
            };
        }
        let per = match unit.as_str() {
            "M" | "month" | "months" => 1,
            "y" | "year" | "years" => 12,
            _ => 0,
        };
        if per == 0 {
            rest.push(format!("{num}{unit}"));
            continue;
        }
        match num.parse::<i64>().ok().and_then(|n| n.checked_mul(per)) {
            Some(n) => months = months.saturating_add(n),
            // A fractional or absurd count: leave it to the fixed grammar,
            // which rejects the calendar unit with its usual explanation.
            None => return (0, text.trim().to_string()),
        }
    }
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
/// neither has a length in seconds until a date anchors it. `add_*`/`sub_*`
/// take them instead, and say so in the error.
fn parse_delta_str(text: &str, what: &str) -> LuaResult<f64> {
    if let Some(unit) = calendar_unit(text) {
        return Err(mlua::Error::runtime(format!(
            "{what} cannot read '{text}': a {unit} has no fixed length in \
             seconds until it is anchored to a date. Use \
             time.add_utc(t, '{text}') / time.sub_utc(t, '{text}') (or the \
             _local/_tz variants), which anchor at the timestamp."
        )));
    }
    if text.starts_with('-') {
        return Err(mlua::Error::runtime(format!(
            "{what} cannot read '{text}': deltas are unsigned durations. Use \
             time.sub_utc(t, '{}') (or the _local/_tz variants) to go backwards.",
            text.trim_start_matches('-')
        )));
    }
    humantime::parse_duration(text)
        .map(|d| d.as_secs_f64())
        .map_err(|e| {
            // A bare number is the one failure worth naming: it is the mistake
            // a caller makes on purpose, expecting seconds.
            if !text.is_empty() && text.chars().all(|c| c.is_ascii_digit() || c == '.') {
                let fix = if text.contains('.') {
                    format!("pass the number {text}")
                } else {
                    format!("write \"{text}s\" for seconds, or pass the number {text}")
                };
                return mlua::Error::runtime(format!(
                    "{what} needs a unit on '{text}' -- {fix}."
                ));
            }
            mlua::Error::runtime(format!(
                "{what} could not read '{text}' as a delta ({e}); expected \
                 something like \"30s\", \"1h 30min\" or \"4d2h5m\""
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
    if text.starts_with('+') || text.starts_with('-') {
        return Err(mlua::Error::runtime(format!(
            "{what} expects an IANA time zone name like \"Europe/Berlin\", got the \
             fixed offset '{text}'."
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
/// returned verbatim, so `time.format_fmt_tz(t, "+05:30", zone)` (arguments
/// swapped) would yield the string `"+05:30"` rather than a timestamp. That
/// reads as a plausible answer, which makes it worse than an error.
fn check_pattern(pattern: &str, what: &str) -> LuaResult<()> {
    if pattern.contains('%') {
        return Ok(());
    }
    let hint = if pattern.starts_with('+')
        || (pattern.starts_with('-') && pattern.len() > 1)
        || pattern.eq_ignore_ascii_case("z")
        || pattern.eq_ignore_ascii_case("utc")
        || pattern.contains('/')
    {
        " -- check the argument order"
    } else {
        ""
    };
    Err(mlua::Error::runtime(format!(
        "{what} takes a strftime pattern with at least one % conversion, \
         not '{pattern}'{hint}."
    )))
}

/// Render an instant through a strftime pattern in `zone`.
fn format_pattern(ts: Timestamp, zone: TimeZone, pattern: &str, what: &str) -> LuaResult<String> {
    check_pattern(pattern, what)?;
    let zoned = ts.to_zoned(zone);
    strtime::format(pattern, &zoned).map_err(|e| {
        mlua::Error::runtime(format!(
            "{what} could not format with '{pattern}': {e}; expected strftime \
             conversions like %Y-%m-%d or %H:%M:%S"
        ))
    })
}

/// Render an instant as RFC 3339 in `zone`.
fn format_rfc3339_in(ts: Timestamp, zone: TimeZone, what: &str) -> LuaResult<String> {
    let zoned = ts.to_zoned(zone);
    // `%.f` keeps sub-second precision and prints nothing for a whole second.
    let text = strtime::format("%Y-%m-%dT%H:%M:%S%.f%:z", &zoned)
        .map_err(|e| mlua::Error::runtime(format!("{what} could not format: {e}")))?;
    Ok(match text.strip_suffix("+00:00") {
        Some(head) => format!("{head}Z"),
        None => text,
    })
}

/// Render an instant as a civil (offset-less) date-time in `zone`.
fn format_civil_in(ts: Timestamp, zone: TimeZone, what: &str) -> LuaResult<String> {
    let zoned = ts.to_zoned(zone);
    strtime::format("%Y-%m-%dT%H:%M:%S%.f", &zoned)
        .map_err(|e| mlua::Error::runtime(format!("{what} could not format: {e}")))
}

/// Format a delta in seconds as `"4d 2h 5m 2s"`.
///
/// **The second is this module's display resolution.** A delta arrives as an
/// `f64` and a computed one carries a fraction; rendering it to the last digit
/// gives `173h 44m 24s 737ms 129µs 927ns`, which is not a more precise answer
/// but an unreadable one. So the fraction is dropped here rather than left to
/// every call site to remember, and a script wanting a coarser unit than the
/// second says so with [`truncate`].
///
/// The decomposition is written out rather than delegated because neither
/// candidate renders the wanted spelling: jiff's `SignedDuration` is
/// calendar-less and stops at the hour (4 days prints as `96h`), and
/// humantime's formatter spells the large units in full (`4days 2h 5m 2s`) and
/// approximates years and months. Output stays readable by [`parse_delta_str`].
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

/// The unit named by `time.truncate`/`time.count`, as a number of seconds.
///
/// The ceiling is the day -- a week is offered as a shorthand nowhere here
/// because `minutes`/`hours`/`days` are the only fixed-length builders this
/// module exposes; a month or year has no length in seconds until a date
/// anchors it, the same line [`parse_delta_str`] draws.
fn unit_seconds(name: &str, what: &str) -> LuaResult<f64> {
    match name.trim() {
        "s" | "sec" | "secs" | "second" | "seconds" => Ok(1.0),
        "m" | "min" | "mins" | "minute" | "minutes" => Ok(60.0),
        "h" | "hr" | "hrs" | "hour" | "hours" => Ok(3600.0),
        "d" | "day" | "days" => Ok(86_400.0),
        other => Err(mlua::Error::runtime(format!(
            "{what} does not know the unit '{other}'; expected one of \"s\", \
             \"m\", \"h\", \"d\"."
        ))),
    }
}

/// The host's time zone, so `_local` follows its DST rules at the instant in
/// question rather than pinning today's offset.
fn local_zone() -> TimeZone {
    TimeZone::system()
}

// ---------------------------------------------------------------------------
// Engine registration
// ---------------------------------------------------------------------------

/// Register the `time` module on `lua`: the value layer, available everywhere.
///
/// Everything here is a pure conversion between values the script already has.
/// Nothing reads the clock and nothing reads the host's zone, which is why all
/// three binaries get it — and why the `_local` variants live only in
/// [`register_host_time`].
pub fn register(lua: &Lua) -> LuaResult<()> {
    let module = lua.create_table()?;

    // time.parse_rfc3339(str) -> number
    // The offset is in the string; RFC 3339 requires one, so there is no zone
    // suffix to choose between.
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
    // The zone is in the string, either as a numeric offset or one of RFC
    // 2822's obsolete letter zones (`GMT`, `UT`, the military letters) -- both
    // in the same function, since a real header may use either and a receiver
    // must accept both.
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

    // time.parse_civil_utc(str) -> number
    // A date or date-time with no zone in it, read as UTC.
    module.set(
        "parse_civil_utc",
        lua.create_function(|_, text: String| {
            parse_civil(text.trim(), &TimeZone::UTC, "time.parse_civil_utc")
        })?,
    )?;

    // time.parse_civil_tz(str, zone) -> number
    // As `parse_civil_utc`, but in a named IANA zone -- DST edges resolve
    // rather than raise (jiff's "compatible" strategy): the repeated hour of
    // a fall-back transition takes its first occurrence, and a time that
    // never happened (a spring-forward gap) shifts forward past it.
    module.set(
        "parse_civil_tz",
        lua.create_function(|_, (text, zone): (String, String)| {
            let zone = zone_from(&zone, "time.parse_civil_tz")?;
            parse_civil(text.trim(), &zone, "time.parse_civil_tz")
        })?,
    )?;

    // time.parse_fmt_utc(str, pattern) -> number
    // Reads exactly `pattern`; a pattern with no time component reads as
    // midnight. If the pattern itself resolves an offset (e.g. `%z`), that
    // would conflict with "read as UTC" -- rejected, since the two say
    // different things about the same string.
    module.set(
        "parse_fmt_utc",
        lua.create_function(|_, (text, pattern): (String, String)| {
            parse_fmt(text.trim(), &pattern, TimeZone::UTC, "time.parse_fmt_utc")
        })?,
    )?;

    // time.parse_fmt_tz(str, pattern, zone) -> number
    // As `parse_fmt_utc`, but any offset-less remainder is placed in a named
    // IANA zone instead of UTC.
    module.set(
        "parse_fmt_tz",
        lua.create_function(|_, (text, pattern, zone): (String, String, String)| {
            let zone = zone_from(&zone, "time.parse_fmt_tz")?;
            parse_fmt(text.trim(), &pattern, zone, "time.parse_fmt_tz")
        })?,
    )?;

    // time.format_rfc3339_utc(t) -> string
    module.set(
        "format_rfc3339_utc",
        lua.create_function(|_, t: Value| {
            let secs = TimeArg::from_value(&t, "time.format_rfc3339_utc")?
                .to_secs_zoned("time.format_rfc3339_utc")?;
            let ts = secs_to_timestamp(secs, "time.format_rfc3339_utc")?;
            format_rfc3339_in(ts, TimeZone::UTC, "time.format_rfc3339_utc")
        })?,
    )?;

    // time.format_rfc3339_tz(t, zone) -> string
    // The offset follows `zone`'s DST rules at `t`: Berlin renders +02:00 in
    // summer and +01:00 in winter.
    module.set(
        "format_rfc3339_tz",
        lua.create_function(|_, (t, zone): (Value, String)| {
            let secs = TimeArg::from_value(&t, "time.format_rfc3339_tz")?
                .to_secs_zoned("time.format_rfc3339_tz")?;
            let ts = secs_to_timestamp(secs, "time.format_rfc3339_tz")?;
            let zone = zone_from(&zone, "time.format_rfc3339_tz")?;
            format_rfc3339_in(ts, zone, "time.format_rfc3339_tz")
        })?,
    )?;

    // time.format_rfc2822(t) -> string   "Sat, 13 Jul 2024 15:09:59 -0000"
    // Always UTC, always the numeric `-0000` spelling -- RFC 2822 §3.3 gives
    // that the meaning "this is UTC, and the sender's own zone is unknown",
    // which is exactly true of a timestamp here: the offset it was parsed
    // with was absorbed into the instant and is not recoverable. `+0000`
    // would claim the sender was *in* UTC. Receivers treat both as the same
    // instant. jiff's printer has no other zone spelling to offer, so there
    // is no suffix family here.
    module.set(
        "format_rfc2822",
        lua.create_function(|_, t: Value| {
            let secs = TimeArg::from_value(&t, "time.format_rfc2822")?
                .to_secs_zoned("time.format_rfc2822")?;
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
    // characters.
    module.set(
        "format_imf",
        lua.create_function(|_, t: Value| {
            let secs =
                TimeArg::from_value(&t, "time.format_imf")?.to_secs_zoned("time.format_imf")?;
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

    // time.format_fmt_utc(t, pattern) -> string
    module.set(
        "format_fmt_utc",
        lua.create_function(|_, (t, pattern): (Value, String)| {
            let secs = TimeArg::from_value(&t, "time.format_fmt_utc")?
                .to_secs_zoned("time.format_fmt_utc")?;
            let ts = secs_to_timestamp(secs, "time.format_fmt_utc")?;
            format_pattern(ts, TimeZone::UTC, &pattern, "time.format_fmt_utc")
        })?,
    )?;

    // time.format_fmt_tz(t, pattern, zone) -> string
    module.set(
        "format_fmt_tz",
        lua.create_function(|_, (t, pattern, zone): (Value, String, String)| {
            let secs = TimeArg::from_value(&t, "time.format_fmt_tz")?
                .to_secs_zoned("time.format_fmt_tz")?;
            let ts = secs_to_timestamp(secs, "time.format_fmt_tz")?;
            let zone = zone_from(&zone, "time.format_fmt_tz")?;
            format_pattern(ts, zone, &pattern, "time.format_fmt_tz")
        })?,
    )?;

    // time.parse_delta(value) -> number
    // A number is seconds already, a string is read by humantime's grammar --
    // the same reading every delta-taking function in this module does, so
    // there is one answer to "what does '1h30m' mean" rather than one per
    // call site.
    module.set(
        "parse_delta",
        lua.create_function(|_, value: Value| parse_delta_value(&value, "time.parse_delta"))?,
    )?;

    // time.format_delta(value) -> string   "4d 2h 5m 2s"
    // The inverse of `time.parse_delta`. Takes the same coercion as
    // everything else, so formatting a duration *string* is a normalisation
    // rather than an error.
    module.set(
        "format_delta",
        lua.create_function(|_, value: Value| {
            Ok(format_delta(parse_delta_value(
                &value,
                "time.format_delta",
            )?))
        })?,
    )?;

    // time.minutes(n) / time.hours(n) / time.days(n) -> number
    // A count of the unit, as a delta in seconds -- `time.hours(2)` is the
    // same 7200 as `time.parse_delta("2h")`, without a string in between.
    for (name, secs) in [("minutes", 60.0), ("hours", 3600.0), ("days", 86_400.0)] {
        module.set(
            name,
            lua.create_function(move |_, n: Value| {
                let what = format!("time.{name}");
                let n = match n {
                    Value::Integer(n) => n as f64,
                    Value::Number(n) if n.is_finite() => n,
                    other => {
                        return Err(mlua::Error::runtime(format!(
                            "{what} expects a finite number of {name}, got {}",
                            other.type_name()
                        )));
                    }
                };
                Ok(n * secs)
            })?,
        )?;
    }

    // time.truncate(secs, unit) -> number
    // Floor a delta to whole units, in seconds: `time.truncate(7500, "h")` is
    // `7200`, the two whole hours in it. Floors toward zero, matching
    // `parse_delta`'s magnitude-only reading -- a negative delta floors
    // toward zero from that side too.
    module.set(
        "truncate",
        lua.create_function(|_, (value, unit): (Value, String)| {
            let secs = parse_delta_value(&value, "time.truncate")?;
            let unit = unit_seconds(&unit, "time.truncate")?;
            Ok((secs / unit).trunc() * unit)
        })?,
    )?;

    // time.count(secs, unit) -> number
    // As `time.truncate`, but returns the unit count instead of the seconds
    // it represents: `time.count(7500, "h")` is `2`.
    module.set(
        "count",
        lua.create_function(|_, (value, unit): (Value, String)| {
            let secs = parse_delta_value(&value, "time.count")?;
            let unit = unit_seconds(&unit, "time.count")?;
            Ok((secs / unit).trunc())
        })?,
    )?;

    // time.add_utc(t, delta) / time.sub_utc(t, delta) -> number
    // Calendar-aware: a month or year anchors at `t` in UTC, so adding "1M"
    // to the 31st lands where the calendar says rather than 30 days later.
    for (name, negate) in [("add_utc", false), ("sub_utc", true)] {
        let what = format!("time.{name}");
        module.set(
            name,
            lua.create_function(move |_, (t, delta): (Value, Value)| {
                let secs = TimeArg::from_value(&t, &what)?.to_secs_zoned(&what)?;
                shift(secs, &delta, TimeZone::UTC, negate, &what)
            })?,
        )?;
    }

    // time.format_civil_utc(t) / time.format_civil_tz(t, zone) -> string
    // The inverse of `parse_civil_*`: a wall-clock date-time with no offset.
    module.set(
        "format_civil_utc",
        lua.create_function(|_, t: Value| {
            let what = "time.format_civil_utc";
            let secs = TimeArg::from_value(&t, what)?.to_secs_zoned(what)?;
            format_civil_in(secs_to_timestamp(secs, what)?, TimeZone::UTC, what)
        })?,
    )?;
    module.set(
        "format_civil_tz",
        lua.create_function(|_, (t, zone): (Value, String)| {
            let what = "time.format_civil_tz";
            let secs = TimeArg::from_value(&t, what)?.to_secs_zoned(what)?;
            let zone = zone_from(&zone, what)?;
            format_civil_in(secs_to_timestamp(secs, what)?, zone, what)
        })?,
    )?;

    // time.add_tz(t, delta, zone) / time.sub_tz(t, delta, zone) -> number
    // As `add_utc`/`sub_utc`, anchoring a calendar-unit delta in `zone`
    // instead of UTC.
    for (name, negate) in [("add_tz", false), ("sub_tz", true)] {
        let what = format!("time.{name}");
        module.set(
            name,
            lua.create_function(move |_, (t, delta, zone): (Value, Value, String)| {
                let secs = TimeArg::from_value(&t, &what)?.to_secs_zoned(&what)?;
                let zone = zone_from(&zone, &what)?;
                shift(secs, &delta, zone, negate, &what)
            })?,
        )?;
    }

    lua.globals().set("time", module)?;
    Ok(())
}

/// Parse `text` against `pattern`, placing an offset-less result in `zone`.
///
/// Shared by `parse_fmt_utc`/`parse_fmt_tz`; the host-only `parse_fmt_local`
/// calls it with the host zone. A pattern that itself reads an offset (`%z`)
/// is rejected: the string would name an instant and `zone` would answer for
/// nothing -- use `parse_rfc3339`/`parse_rfc2822` for those.
fn parse_fmt(text: &str, pattern: &str, zone: TimeZone, what: &str) -> LuaResult<f64> {
    check_pattern(pattern, what)?;
    let parsed = strtime::parse(pattern, text).map_err(|e| {
        mlua::Error::runtime(format!(
            "{what} could not read '{text}' with '{pattern}': {e}"
        ))
    })?;
    if parsed.offset().is_some() {
        return Err(mlua::Error::runtime(format!(
            "{what} read an offset or zone from '{text}' with '{pattern}', which \
             would override the zone argument; drop %z/%Z/%Q from the pattern, or \
             read a self-zoned timestamp with time.parse_rfc3339."
        )));
    }
    let dt = parsed.to_datetime().or_else(|_| {
        parsed.to_date().map(|d| d.at(0, 0, 0, 0)).map_err(|e| {
            mlua::Error::runtime(format!(
                "{what} read '{text}' with '{pattern}', but it yields no date: {e}"
            ))
        })
    })?;
    let zoned = zone
        .to_ambiguous_zoned(dt)
        .compatible()
        .map_err(|e| mlua::Error::runtime(format!("{what} could not place '{text}': {e}")))?;
    Ok(timestamp_to_secs(zoned.timestamp()))
}

/// Shift a timestamp by a delta, anchoring calendar units at the timestamp
/// in `zone`.
///
/// Shared by `add_*`/`sub_*` everywhere the zone differs, so the calendar
/// behaviour cannot differ between the UTC, local and named-zone forms.
fn shift(secs: f64, delta: &Value, zone: TimeZone, negate: bool, what: &str) -> LuaResult<f64> {
    // A delta is `parse_delta`'s grammar (a day is a fixed 24h) plus months
    // and years, which only a timestamp can anchor.
    let (months, fixed) = match delta {
        Value::Integer(_) | Value::Number(_) => (0, parse_delta_value(delta, what)?),
        Value::String(s) => {
            let text = s.to_str()?;
            let text = text.trim();
            let (months, rest) = split_calendar(text);
            let fixed = if months == 0 || !rest.is_empty() {
                parse_delta_str(&rest_or(text, months, &rest), what)?
            } else {
                0.0
            };
            (months, fixed)
        }
        other => {
            return Err(mlua::Error::runtime(format!(
                "{what} expects a delta -- seconds as a number, or a string like \
                 \"30s\", \"1h 30min\" or \"1M\" -- got {}",
                other.type_name()
            )));
        }
    };
    let sign = if negate { -1.0 } else { 1.0 };
    let mut out = secs;
    if months != 0 {
        // Anchored at `secs` in `zone`: this is the whole reason `add_*` takes
        // the calendar units the delta parsers reject.
        let span = Span::new()
            .try_months(if negate { -months } else { months })
            .map_err(|e| mlua::Error::runtime(format!("{what}: {months} months: {e}")))?;
        let zoned = secs_to_timestamp(secs, what)?.to_zoned(zone);
        let shifted = zoned
            .checked_add(span)
            .map_err(|e| mlua::Error::runtime(format!("{what} could not shift: {e}")))?;
        out = timestamp_to_secs(shifted.timestamp());
    }
    let out = out + sign * fixed;
    // Reject a result no timestamp can represent now, not at the next use.
    secs_to_timestamp(out, what)?;
    Ok(out)
}

/// The text to hand the fixed grammar: the calendar-free remainder, or the
/// original when there was nothing to split (so its error names what was typed).
fn rest_or(text: &str, months: i64, rest: &str) -> String {
    if months == 0 { text.to_string() } else { rest.to_string() }
}

/// Add the clock-reading and local-zone half of `time` (`ito` only).
///
/// Host-only for the reason `os.getenv` and `fs.cwd()` are: each answer depends
/// on ambient state the caller never passed and cannot see, so two runs of one
/// script can disagree. Where the output is a *preview* of what a run would do
/// that is a correctness problem rather than a taste one — an `ied --diff` that
/// read the clock could differ from the `--write` after it.
///
/// Three kinds of thing land here:
///
/// - `time.now()` — Unix seconds, fractional.
/// - `time.until(t)`/`time.ago(t)` — `t - time.now()` and `time.now() - t`,
///   spelled out under a name so the reader doesn't have to recall a
///   subtraction's argument order to know which direction it measures.
/// - the `_local` half of the parse/format suffix family
///   (`parse_civil_local`, `parse_fmt_local`, `format_rfc3339_local`,
///   `format_fmt_local`) and `add_local`/`sub_local` — every function that
///   may consult the host's **offset**. `if time.now then` (or any of these
///   names) tells a script which half it got.
///
/// What is *not* here is the named-zone surface — `_tz` variants are in
/// [`register`], because their zone arrives as an **argument** and resolves
/// against the *bundled* tzdb: same input, same answer, on every machine and
/// every run. The line is where the answer comes from, not whether dates are
/// involved.
///
/// Like the other reach-into-the-stdlib steps in `Engine::build`, this must
/// run before `sandbox(true)` freezes the `time` table.
pub fn register_host_time(lua: &Lua) -> LuaResult<()> {
    let module: Table = lua.globals().get("time")?;

    // time.now() -> number
    module.set(
        "now",
        lua.create_function(|_, ()| Ok(timestamp_to_secs(Timestamp::now())))?,
    )?;

    // time.till(t) -> number
    // time.ago(t) -> number
    // Spelled-out subtractions against `now()`, named for the direction each
    // reads: "how long till t" vs. "how long ago was t". Named `till` rather
    // than `until` -- `until` is a reserved word in Luau, so `time.until(t)`
    // could never be written with dot-call syntax at all.
    module.set(
        "till",
        lua.create_function(|_, t: Value| {
            let secs = TimeArg::from_value(&t, "time.till")?.to_secs_zoned("time.till")?;
            Ok(secs - timestamp_to_secs(Timestamp::now()))
        })?,
    )?;
    module.set(
        "ago",
        lua.create_function(|_, t: Value| {
            let secs = TimeArg::from_value(&t, "time.ago")?.to_secs_zoned("time.ago")?;
            Ok(timestamp_to_secs(Timestamp::now()) - secs)
        })?,
    )?;

    // time.parse_civil_local(str) -> number
    module.set(
        "parse_civil_local",
        lua.create_function(|_, text: String| {
            let zone = local_zone();
            parse_civil(text.trim(), &zone, "time.parse_civil_local")
        })?,
    )?;

    // time.parse_fmt_local(str, pattern) -> number
    module.set(
        "parse_fmt_local",
        lua.create_function(|_, (text, pattern): (String, String)| {
            let zone = local_zone();
            parse_fmt(text.trim(), &pattern, zone, "time.parse_fmt_local")
        })?,
    )?;

    // time.format_rfc3339_local(t) -> string
    module.set(
        "format_rfc3339_local",
        lua.create_function(|_, t: Value| {
            let secs = TimeArg::from_value(&t, "time.format_rfc3339_local")?
                .to_secs_zoned("time.format_rfc3339_local")?;
            let ts = secs_to_timestamp(secs, "time.format_rfc3339_local")?;
            format_rfc3339_in(
                ts,
                local_zone(),
                "time.format_rfc3339_local",
            )
        })?,
    )?;

    // time.format_fmt_local(t, pattern) -> string
    module.set(
        "format_fmt_local",
        lua.create_function(|_, (t, pattern): (Value, String)| {
            let secs = TimeArg::from_value(&t, "time.format_fmt_local")?
                .to_secs_zoned("time.format_fmt_local")?;
            let ts = secs_to_timestamp(secs, "time.format_fmt_local")?;
            format_pattern(
                ts,
                local_zone(),
                &pattern,
                "time.format_fmt_local",
            )
        })?,
    )?;

    // time.add_local(t, delta) / time.sub_local(t, delta) -> number
    for (name, negate) in [("add_local", false), ("sub_local", true)] {
        let what = format!("time.{name}");
        module.set(
            name,
            lua.create_function(move |_, (t, delta): (Value, Value)| {
                let secs = TimeArg::from_value(&t, &what)?.to_secs_zoned(&what)?;
                shift(secs, &delta, local_zone(), negate, &what)
            })?,
        )?;
    }

    // time.format_civil_local(t) -> string
    module.set(
        "format_civil_local",
        lua.create_function(|_, t: Value| {
            let what = "time.format_civil_local";
            let secs = TimeArg::from_value(&t, what)?.to_secs_zoned(what)?;
            format_civil_in(secs_to_timestamp(secs, what)?, local_zone(), what)
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `Lua` with the shared layer only, as `ied` see it.
    fn value_only() -> Lua {
        let lua = Lua::new();
        register(&lua).unwrap();
        lua
    }

    /// A `Lua` with both layers, as `ito` see it.
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
    fn format_rfc3339_round_trips_through_utc() {
        let lua = value_only();
        let s: String = lua
            .load(
                r#"return time.format_rfc3339_utc(time.parse_rfc3339("2026-08-10T14:00:00+02:00"))"#,
            )
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
        let iso: String = lua
            .load(format!("return time.format_rfc3339_utc({t})"))
            .eval()
            .unwrap();
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
        let iso: String = lua
            .load(format!("return time.format_rfc3339_utc({t})"))
            .eval()
            .unwrap();
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
    fn format_fmt_takes_a_strftime_pattern() {
        let lua = value_only();
        let s: String = lua
            .load(r#"return time.format_fmt_utc(1786363200, "%Y-%m-%d %H:%M")"#)
            .eval()
            .unwrap();
        assert_eq!(s, "2026-08-10 12:00");
    }

    #[test]
    fn parse_fmt_reads_exactly_its_pattern() {
        let lua = value_only();
        let t: f64 = lua
            .load(r#"return time.parse_fmt_utc("10.08.2026", "%d.%m.%Y")"#)
            .eval()
            .unwrap();
        assert_eq!(t, 1786320000.0, "midnight UTC");

        let err = lua
            .load(r#"return time.parse_fmt_utc("2026-08-10", "%d.%m.%Y")"#)
            .eval::<f64>()
            .unwrap_err()
            .to_string();
        assert!(err.contains("could not read"), "{err}");
    }

    #[test]
    fn parse_civil_rejects_a_value_that_already_carries_an_offset() {
        let lua = value_only();
        let err = lua
            .load(r#"return time.parse_civil_utc("2026-08-10T12:00:00Z")"#)
            .eval::<f64>()
            .unwrap_err()
            .to_string();
        assert!(err.contains("carry no UTC offset"), "{err}");
    }

    #[test]
    fn parse_civil_utc_and_tz_agree_on_an_offset_less_reading() {
        let lua = value_only();
        let utc: f64 = lua
            .load(r#"return time.parse_civil_utc("2026-08-10T12:00:00")"#)
            .eval()
            .unwrap();
        assert_eq!(utc, 1786363200.0);
        // Berlin is +02:00 in August, so the same wall-clock reading there is
        // two hours earlier in UTC.
        let berlin: f64 = lua
            .load(r#"return time.parse_civil_tz("2026-08-10T12:00:00", "Europe/Berlin")"#)
            .eval()
            .unwrap();
        assert_eq!(berlin, utc - 7200.0);
    }

    #[test]
    fn parse_civil_resolves_the_dst_edges() {
        let lua = value_only();
        let (fall, spring): (String, String) = lua
            .load(
                r#"local f = time.parse_civil_tz("2026-10-25T02:30:00", "Europe/Berlin")
                   local s = time.parse_civil_tz("2026-03-29T02:30:00", "Europe/Berlin")
                   return time.format_rfc3339_tz(f, "Europe/Berlin"),
                          time.format_rfc3339_tz(s, "Europe/Berlin")"#,
            )
            .eval()
            .unwrap();
        assert_eq!(fall, "2026-10-25T02:30:00+02:00", "the earlier of the two");
        assert_eq!(spring, "2026-03-29T03:30:00+02:00", "shifted past the gap");
    }

    #[test]
    fn format_rfc3339_tz_follows_dst() {
        let lua = value_only();
        let summer: String = lua
            .load(r#"return time.format_rfc3339_tz(1786363200, "Europe/Berlin")"#)
            .eval()
            .unwrap();
        let winter: String = lua
            .load(
                r#"return time.format_rfc3339_tz(time.parse_rfc3339("2026-01-10T12:00:00Z"),
                                                   "Europe/Berlin")"#,
            )
            .eval()
            .unwrap();
        assert_eq!(summer, "2026-08-10T14:00:00+02:00");
        assert_eq!(winter, "2026-01-10T13:00:00+01:00");
    }

    #[test]
    fn offsets_and_zone_names_are_not_confused_for_each_other() {
        let lua = value_only();
        let err = lua
            .load(r#"return time.format_rfc3339_tz(1, "+02:00")"#)
            .eval::<String>()
            .unwrap_err()
            .to_string();
        assert!(err.contains("Europe/Berlin"), "{err}");

        let err = lua
            .load(r#"return time.format_rfc3339_tz(1, "Mars/Olympus")"#)
            .eval::<String>()
            .unwrap_err()
            .to_string();
        assert!(err.contains("does not know the time zone"), "{err}");
    }

    #[test]
    fn an_offset_where_a_pattern_belongs_is_rejected() {
        let lua = value_only();
        let err = lua
            .load(r#"return time.format_fmt_tz(1, "+05:30", "Europe/Berlin")"#)
            .eval::<String>()
            .unwrap_err()
            .to_string();
        assert!(err.contains("check the argument order"), "{err}");
        // A real pattern still works, including one with no `%` conversions
        // that could not be mistaken for an offset or a zone.
        let s: String = lua
            .load(r#"return time.format_fmt_utc(1786363200, "%Y")"#)
            .eval()
            .unwrap();
        assert_eq!(s, "2026");
    }

    #[test]
    fn one_grammar_reads_every_delta_spelling() {
        let lua = value_only();
        for src in [r#""1h 30min""#, r#""1h30m""#, r#""90m""#, "5400"] {
            let secs: f64 = lua
                .load(format!("return time.parse_delta({src})"))
                .eval()
                .unwrap();
            assert_eq!(secs, 5400.0, "{src}");
        }
    }

    /// The delta surface is the *only* one: `add_utc`/`truncate`/
    /// `format_delta` all read a duration string through `time.parse_delta`,
    /// so none can disagree.
    #[test]
    fn every_delta_taker_shares_the_one_coercion() {
        let lua = value_only();
        let same: bool = lua
            .load(
                r#"return time.truncate("1h30m", "m") == time.parse_delta("1h30m")
                   and time.add_utc(0, "1h30m") == time.parse_delta("1h30m")
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
            .load(r#"return time.parse_delta("1200")"#)
            .eval::<f64>()
            .unwrap_err()
            .to_string();
        assert!(err.contains("needs a unit"), "{err}");
        // ...while the number itself means seconds.
        let secs: f64 = lua.load("return time.parse_delta(1200)").eval().unwrap();
        assert_eq!(secs, 1200.0);
    }

    #[test]
    fn fixed_units_are_deltas_but_calendar_units_are_not() {
        let lua = value_only();
        let day: f64 = lua.load(r#"return time.parse_delta("1d")"#).eval().unwrap();
        assert_eq!(day, 86400.0, "a day is 24h here");
        let week: f64 = lua.load(r#"return time.parse_delta("1w")"#).eval().unwrap();
        assert_eq!(week, 604800.0);

        // A month or year has no length until a date anchors it, so the delta
        // reader refuses; `add_*`/`sub_*` take them, having a timestamp.
        for src in [r#""1month""#, r#""1year""#] {
            let err = lua
                .load(format!("return time.parse_delta({src})"))
                .eval::<f64>()
                .unwrap_err()
                .to_string();
            assert!(err.contains("time.add_utc"), "{src}: {err}");
        }
    }

    #[test]
    fn add_anchors_calendar_units_at_the_timestamp() {
        let lua = value_only();
        // January 31 + 1M is the end of February, not 30 days later.
        let s: String = lua
            .load(
                r#"return time.format_rfc3339_utc(time.add_utc(
                       time.parse_rfc3339("2026-01-31T00:00:00Z"), "1M"))"#,
            )
            .eval()
            .unwrap();
        assert_eq!(s, "2026-02-28T00:00:00Z");
    }

    #[test]
    fn add_and_sub_are_inverses_on_fixed_units() {
        let lua = value_only();
        let round: f64 = lua
            .load(r#"return time.sub_utc(time.add_utc(1786363200, "1h"), "1h")"#)
            .eval()
            .unwrap();
        assert_eq!(round, 1786363200.0);
    }

    #[test]
    fn add_tz_anchors_in_the_named_zone() {
        let lua = value_only();
        // Adding one calendar month across a DST boundary should track the
        // zone's local calendar, not UTC's.
        let s: String = lua
            .load(
                r#"local t = time.parse_rfc3339("2026-01-31T12:00:00Z")
                   return time.format_rfc3339_tz(time.add_tz(t, "1M", "Europe/Berlin"),
                                                  "Europe/Berlin")"#,
            )
            .eval()
            .unwrap();
        assert_eq!(s, "2026-02-28T13:00:00+01:00");
    }

    #[test]
    fn units_build_deltas_from_a_count() {
        let lua = value_only();
        let (m, h, d): (f64, f64, f64) = lua
            .load("return time.minutes(2), time.hours(2), time.days(2)")
            .eval()
            .unwrap();
        assert_eq!((m, h, d), (120.0, 7200.0, 172_800.0));
    }

    #[test]
    fn truncate_floors_to_whole_units() {
        let lua = value_only();
        for (src, want) in [
            ("time.truncate(7500, \"h\")", 7200.0),
            ("time.truncate(90, \"m\")", 60.0),
            ("time.truncate(180000, \"d\")", 172800.0),
        ] {
            let got: f64 = lua.load(format!("return {src}")).eval().unwrap();
            assert_eq!(got, want, "{src}");
        }
    }

    #[test]
    fn count_returns_the_unit_count_not_seconds() {
        let lua = value_only();
        for (src, want) in [
            ("time.count(7500, \"h\")", 2.0),
            ("time.count(90, \"m\")", 1.0),
        ] {
            let got: f64 = lua.load(format!("return {src}")).eval().unwrap();
            assert_eq!(got, want, "{src}");
        }
    }

    #[test]
    fn truncate_and_count_reject_calendar_units() {
        let lua = value_only();
        for unit in ["ms", "month", "y", "w"] {
            let err = lua
                .load(format!(r#"return time.truncate(5400, "{unit}")"#))
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
                "return time.now == nil and time.parse_civil_local == nil \
                 and time.format_rfc3339_local == nil and time.till == nil \
                 and time.ago == nil",
            )
            .eval()
            .unwrap();
        assert!(missing, "the ambient half must not leak into ied");
        // ...and the value half is all there.
        let present: bool = lua
            .load(
                "return time.parse_rfc3339 ~= nil and time.parse_delta ~= nil \
                 and time.parse_rfc2822 ~= nil and time.format_rfc2822 ~= nil \
                 and time.format_imf ~= nil and time.parse_civil_tz ~= nil \
                 and time.format_rfc3339_tz ~= nil",
            )
            .eval()
            .unwrap();
        assert!(present);
    }

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

    #[test]
    fn format_delta_round_trips_through_parse_delta() {
        let lua = value_only();
        for secs in [5400.0, 353102.0, 345600.0, 90.0, 1.0] {
            let back: f64 = lua
                .load(format!(
                    "return time.parse_delta(time.format_delta({secs}))"
                ))
                .eval()
                .unwrap();
            assert_eq!(back, secs, "{secs}");
        }
    }

    #[test]
    fn a_negative_delta_prints_with_a_leading_minus() {
        let lua = value_only();
        let s: String = lua.load("return time.format_delta(-90)").eval().unwrap();
        assert_eq!(s, "-1m 30s");
        // ...but it is not a spelling anyone can write.
        assert!(
            lua.load(r#"return time.parse_delta("-90s")"#)
                .eval::<f64>()
                .is_err()
        );
    }

    #[test]
    fn parse_guesses_are_gone_but_civil_local_exists_under_host_io() {
        let lua = with_host();
        let naive: f64 = lua
            .load(r#"return time.parse_civil_local("2026-08-10T12:00:00")"#)
            .eval()
            .unwrap();
        // Offset of the host zone *at that instant*, not today's.
        let at = Timestamp::from_second(1786363200).unwrap();
        let expected = 1786363200.0 - f64::from(local_zone().to_offset(at).seconds());
        assert_eq!(naive, expected);
    }

    #[test]
    fn a_day_is_24h_in_every_zone() {
        let lua = value_only();
        // Across the 2026-03-29 spring-forward in Berlin, "1d" == 86400.
        let same: bool = lua
            .load(
                r#"local t = time.parse_rfc3339("2026-03-28T12:00:00Z")
                   local z = "Europe/Berlin"
                   return time.add_tz(t, "1d", z) == time.add_tz(t, 86400, z)
                      and time.add_tz(t, "1d", z) == time.add_tz(t, time.days(1), z)"#,
            )
            .eval()
            .unwrap();
        assert!(same);
    }

    #[test]
    fn add_reads_parse_delta_grammar_plus_months_and_years() {
        let lua = value_only();
        let (a, b, c): (String, String, String) = lua
            .load(
                r#"local t = time.parse_rfc3339("2026-01-31T00:00:00Z")
                   return time.format_rfc3339_utc(time.add_utc(t, "1M 1d")),
                          time.format_rfc3339_utc(time.add_utc(t, "1y")),
                          time.format_rfc3339_utc(time.sub_utc(t, "1month 2h"))"#,
            )
            .eval()
            .unwrap();
        assert_eq!(a, "2026-03-01T00:00:00Z");
        assert_eq!(b, "2027-01-31T00:00:00Z");
        assert_eq!(c, "2025-12-30T22:00:00Z");
        // The jiff Span spellings are gone.
        for bad in ["PT1H", "-1h", "2h ago"] {
            assert!(
                lua.load(format!(r#"return time.add_utc(0, "{bad}")"#))
                    .eval::<f64>()
                    .is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn errors_carry_the_time_prefix() {
        let lua = value_only();
        let err = lua
            .load(r#"return time.add_utc(0, "nonsense")"#)
            .eval::<f64>()
            .unwrap_err()
            .to_string();
        assert!(err.contains("time.add_utc"), "{err}");
        let err = lua
            .load(r#"return time.hours("2h")"#)
            .eval::<f64>()
            .unwrap_err()
            .to_string();
        assert!(err.contains("time.hours"), "{err}");
    }

    #[test]
    fn delta_error_hints_name_the_actual_problem() {
        let lua = value_only();
        let err = lua
            .load(r#"return time.parse_delta("-90s")"#)
            .eval::<f64>()
            .unwrap_err()
            .to_string();
        assert!(err.contains("unsigned") && err.contains("sub_utc"), "{err}");
        let err = lua
            .load(r#"return time.parse_delta("1.5")"#)
            .eval::<f64>()
            .unwrap_err()
            .to_string();
        assert!(err.contains("pass the number 1.5"), "{err}");
    }

    #[test]
    fn rfc3339_keeps_subseconds_and_rounding_never_overflows() {
        let lua = value_only();
        let s: String = lua
            .load("return time.format_rfc3339_utc(1786363200.5)")
            .eval()
            .unwrap();
        assert_eq!(s, "2026-08-10T12:00:00.5Z");
        let s: String = lua
            .load("return time.format_rfc3339_utc(1786363200.9999999999)")
            .eval()
            .unwrap();
        assert_eq!(s, "2026-08-10T12:00:01Z");
    }

    #[test]
    fn civil_formatters_invert_the_parsers() {
        let lua = value_only();
        let (u, tz): (String, String) = lua
            .load(
                r#"local t = time.parse_civil_tz("2026-08-10T12:00:00", "Europe/Berlin")
                   return time.format_civil_utc(t), time.format_civil_tz(t, "Europe/Berlin")"#,
            )
            .eval()
            .unwrap();
        assert_eq!(u, "2026-08-10T10:00:00");
        assert_eq!(tz, "2026-08-10T12:00:00");
    }

    #[test]
    fn parse_fmt_rejects_an_offset_and_a_pattern_without_conversions() {
        let lua = value_only();
        let err = lua
            .load(r#"return time.parse_fmt_utc("2026-08-10 +0200", "%Y-%m-%d %z")"#)
            .eval::<f64>()
            .unwrap_err()
            .to_string();
        assert!(err.contains("override the zone"), "{err}");
        assert!(
            lua.load(r#"return time.format_fmt_utc(0, "plain")"#)
                .eval::<String>()
                .is_err()
        );
    }

    #[test]
    fn a_timestamp_string_may_be_rfc2822() {
        let lua = value_only();
        let s: String = lua
            .load(r#"return time.format_rfc3339_utc("Sat, 13 Jul 2024 15:09:59 GMT")"#)
            .eval()
            .unwrap();
        assert_eq!(s, "2024-07-13T15:09:59Z");
    }

    #[test]
    fn a_bare_date_is_midnight() {
        let lua = with_host();
        let date: f64 = lua
            .load(r#"return time.parse_civil_local("2026-08-10")"#)
            .eval()
            .unwrap();
        let noon: f64 = lua
            .load(r#"return time.parse_civil_local("2026-08-10T12:00:00")"#)
            .eval()
            .unwrap();
        assert_eq!(noon - date, 43200.0, "midnight is 12h before noon");
    }

    #[test]
    fn till_and_ago_are_now_relative_and_opposite_signed() {
        let lua = with_host();
        let (till, ago): (f64, f64) = lua
            .load(
                "local t = time.now() - 60 \
                 return time.till(t), time.ago(t)",
            )
            .eval()
            .unwrap();
        assert!((till - -60.0).abs() < 5.0, "{till}");
        assert!((ago - 60.0).abs() < 5.0, "{ago}");
    }

    #[test]
    fn format_rfc3339_local_carries_a_real_offset() {
        let lua = with_host();
        let s: String = lua
            .load("return time.format_rfc3339_local(1786363200)")
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
