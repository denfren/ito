//! Luau bindings for AWS CloudWatch, registered as `aws.cloudwatch`.
//!
//! Follows the conventions documented on the parent module: a request is
//! configured through a builder finalized by `:send()`, and every `:send()`
//! either raises or returns `(envelope, data)`.
//!
//! Metrics only, and only the read path: this module answers "what was this
//! metric doing" and "what metrics exist", which is what an investigation
//! needs. Alarms are deliberately absent.
//!
//! # `get_metric_statistics`
//!
//! One metric, aggregated into time buckets:
//!
//! ```lua
//! local r, points = aws.cloudwatch.get_metric_statistics("AWS/EC2", "CPUUtilization", "5m")
//!     :dimension("InstanceId", "i-0abc123")
//!     :start_time(time.now() - time.hours(3))
//!     :average()
//!     :send()
//!
//! for _, p in ipairs(points) do
//!     print(p.timestamp, p.average)
//! end
//! ```
//!
//! **The period is an entry-function argument**, alongside the namespace and
//! metric name, because the API requires it and because it is not a knob with
//! a sensible default: it sets the resolution of the answer, and guessing it
//! silently decides how many datapoints a range produces. It accepts anything
//! `time.parse_delta` does, so `300`, `"5m"` and `time.minutes(5)` are one
//! value spelled three ways.
//!
//! `:end_time()` *does* default — to now — since a start time alone is the
//! overwhelmingly common shape and "up to the present" is the only thing an
//! absent end could have meant.
//!
//! # Dimensions are an exact match, not a filter
//!
//! CloudWatch stores a metric under the *full* set of dimensions it was
//! published with, and `GetMetricStatistics` matches that set exactly. Asking
//! for `InstanceId` alone, when the metric was published with `InstanceId` **and**
//! `ImageId` **and** `InstanceType`, returns **no datapoints and no error** —
//! the single sharpest edge in this API.
//!
//! This module does not paper over it: detecting the case would mean a
//! `ListMetrics` call behind every read, and inventing an error for an answer
//! the service considers valid. `list_metrics` is the tool for finding out
//! which sets exist, which is why both live here.
//!
//! # `:dimension()` accumulates
//!
//! Unlike `aws.cloudtrail`'s single-slot `:filter()`, repeated `:dimension()`
//! calls build up the set — the API takes a list, and an exact match is the
//! whole point. `:dimensions({[name] = value})` sets several at once from the
//! `{[key] = value}` map shape the rest of the `aws` surface uses for tags.
//!
//! # Statistics are named methods
//!
//! `:average()`, `:sum()`, `:minimum()`, `:maximum()`, `:sample_count()` —
//! the set is closed and five items long, so a typo is a load-time error here
//! rather than a round trip to AWS. They accumulate; each requested statistic
//! becomes a field on every datapoint. Percentiles are the open-ended part and
//! go through `:percentile("p99")`, a different field upstream
//! (`ExtendedStatistics`) that AWS refuses to combine with the five, so mixing
//! the two raises.
//!
//! Requesting nothing means `:average()`: unlike the period, there is an
//! obvious right answer.
//!
//! # Datapoints come back sorted
//!
//! The API returns them in no particular order — genuinely unordered, not
//! merely undocumented. They are sorted by timestamp, oldest first, before
//! reaching the script: every caller either wants that or does not care.
//!
//! A statistic that was not requested is **absent** rather than zero, since
//! `0` is a real reading.

use aws_sdk_cloudwatch::Client;
use aws_sdk_cloudwatch::types::{
    Datapoint, Dimension, DimensionFilter, Metric, StandardUnit, Statistic,
};
use mlua::{
    AnyUserData, Lua, Result as LuaResult, Table, UserData, UserDataMethods, Value, Variadic,
};
use std::sync::{Arc, Mutex};

use super::{BoxError, ClientSource, date_to_string, lua_err, respond};

/// The five closed statistics, paired with their builder method names.
///
/// One list rather than five method registrations, so a statistic cannot be
/// added to the module without also being convertible back out of a datapoint.
const STATISTICS: [(&str, Statistic); 5] = [
    ("average", Statistic::Average),
    ("sum", Statistic::Sum),
    ("minimum", Statistic::Minimum),
    ("maximum", Statistic::Maximum),
    ("sample_count", Statistic::SampleCount),
];

/// Shared request state, mutated in place by the builder methods.
#[derive(Default, Clone)]
struct StatisticsState {
    dimensions: Vec<Dimension>,
    start_time: Option<aws_smithy_types::DateTime>,
    /// Absent means "now", resolved at `:send()` rather than at construction
    /// so a builder held across an await still ends at the send.
    end_time: Option<aws_smithy_types::DateTime>,
    /// Kept in request order, deduplicated: the API rejects a repeat.
    statistics: Vec<Statistic>,
    percentiles: Vec<String>,
    unit: Option<StandardUnit>,
}

/// Builder for `GetMetricStatistics`, finalized with `:send()`.
struct GetMetricStatistics {
    client: Client,
    namespace: String,
    metric_name: String,
    /// Seconds, already validated as a positive multiple of 60 (or one of the
    /// high-resolution values) at the entry call.
    period: i32,
    state: Arc<Mutex<StatisticsState>>,
}

/// Shared request state for a metric listing.
#[derive(Default, Clone)]
struct ListMetricsState {
    namespace: Option<String>,
    metric_name: Option<String>,
    dimensions: Vec<DimensionFilter>,
    recently_active: bool,
    max_metrics: Option<usize>,
}

/// Builder for `ListMetrics`, finalized with `:send()`.
struct ListMetrics {
    client: Client,
    state: Arc<Mutex<ListMetricsState>>,
}

/// Add a dimension to the set: an exact repeat is a no-op, the same name with
/// another value raises (a metric has one value per dimension).
fn push_dimension(ud: &AnyUserData, d: Dimension) -> LuaResult<()> {
    let this = ud.borrow::<GetMetricStatistics>()?;
    let mut state = this.state.lock().unwrap();
    match state.dimensions.iter().find(|e| e.name == d.name) {
        Some(e) if e.value == d.value => Ok(()),
        Some(e) => Err(mlua::Error::runtime(format!(
            "get_metric_statistics: dimension '{}' is already set to '{}', not '{}'",
            d.name.as_deref().unwrap_or(""),
            e.value.as_deref().unwrap_or(""),
            d.value.as_deref().unwrap_or("")
        ))),
        None => {
            state.dimensions.push(d);
            Ok(())
        }
    }
}

/// The API's own cap on datapoints per `GetMetricStatistics` call.
const MAX_DATAPOINTS: f64 = 1440.0;

/// The periods CloudWatch accepts below a minute, for high-resolution metrics.
const HIGH_RESOLUTION_PERIODS: [i64; 4] = [1, 5, 10, 30];

/// Validate a period in whole seconds against what the API accepts: 1, 5, 10
/// or 30 for high-resolution metrics, any multiple of 60 above that.
///
/// Checked here rather than left to AWS because it is the script author's
/// mistake and the service's own message for it does not name the rule.
fn period_secs(secs: i64) -> LuaResult<i32> {
    let ok = HIGH_RESOLUTION_PERIODS.contains(&secs) || (secs >= 60 && secs % 60 == 0);
    if !ok {
        return Err(mlua::Error::runtime(format!(
            "get_metric_statistics: 'period' must be 1, 5, 10, 30, or a multiple of 60 seconds, got {secs}"
        )));
    }
    i32::try_from(secs).map_err(|_| {
        mlua::Error::runtime(format!(
            "get_metric_statistics: 'period' is too large, got {secs} seconds"
        ))
    })
}

const MIXED_STATS: &str = "get_metric_statistics: statistics (':average()', …) and ':percentile()' cannot be combined; AWS rejects the request";

/// Raise on an inverted range or one the API would refuse for holding more
/// than 1440 datapoints.
fn check_range(
    start: aws_smithy_types::DateTime,
    end: aws_smithy_types::DateTime,
    period: i32,
) -> LuaResult<()> {
    let span = end.as_secs_f64() - start.as_secs_f64();
    if span <= 0.0 {
        return Err(mlua::Error::runtime(
            "get_metric_statistics: ':start_time()' must be before ':end_time()'",
        ));
    }
    let points = (span / f64::from(period)).ceil();
    if points > MAX_DATAPOINTS {
        return Err(mlua::Error::runtime(format!(
            "get_metric_statistics: {points} datapoints exceeds the API's cap of 1440; widen the period or narrow the range"
        )));
    }
    Ok(())
}

/// Build a `Dimension`. Infallible upstream: both fields are plain options,
/// so there is nothing for the builder to reject.
fn dimension(name: String, value: String) -> Dimension {
    Dimension::builder().name(name).value(value).build()
}

/// Read a `{[name] = value}` map into a list, sorted by name.
///
/// Sorted because a Luau table has no order of its own and an arbitrary one
/// would make the same script send a different request run to run; the request
/// is a set either way, so any stable order will do.
fn dimension_pairs(table: &Table) -> LuaResult<Vec<(String, String)>> {
    let mut pairs = Vec::new();
    for entry in table.pairs::<String, String>() {
        let (name, value) = entry?;
        pairs.push((name, value));
    }
    pairs.sort();
    Ok(pairs)
}

impl UserData for GetMetricStatistics {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        // --- dimensions ------------------------------------------------------
        // These accumulate rather than replace: CloudWatch matches the full
        // published set exactly, so naming one at a time is how a set is built.

        methods.add_function(
            "dimension",
            |_, (ud, name, value): (AnyUserData, String, String)| {
                push_dimension(&ud, dimension(name, value))?;
                Ok(ud)
            },
        );

        // The `{[name] = value}` map, the same shape the rest of the `aws`
        // surface uses for tags -- so a set read from elsewhere needs no
        // reshaping. Adds to whatever `:dimension()` already named.
        methods.add_function("dimensions", |_, (ud, table): (AnyUserData, Table)| {
            for (name, value) in dimension_pairs(&table)? {
                push_dimension(&ud, dimension(name, value))?;
            }
            Ok(ud)
        });

        // --- time bounds -----------------------------------------------------

        methods.add_function("start_time", |_, (ud, t): (AnyUserData, Value)| {
            let dt = super::time_arg("get_metric_statistics", "start_time", &t)?;
            ud.borrow::<Self>()?.state.lock().unwrap().start_time = Some(dt);
            Ok(ud)
        });

        // Defaults to now when never called; see the module docs.
        methods.add_function("end_time", |_, (ud, t): (AnyUserData, Value)| {
            let dt = super::time_arg("get_metric_statistics", "end_time", &t)?;
            ud.borrow::<Self>()?.state.lock().unwrap().end_time = Some(dt);
            Ok(ud)
        });

        // --- statistics ------------------------------------------------------
        // One method per closed-set statistic, so a typo fails here rather
        // than at AWS. They accumulate, and a repeat is a no-op rather than a
        // duplicate the API would reject.

        for (name, statistic) in STATISTICS {
            methods.add_function(name, move |_, ud: AnyUserData| {
                let this = ud.borrow::<Self>()?;
                let mut state = this.state.lock().unwrap();
                if !state.percentiles.is_empty() {
                    return Err(mlua::Error::runtime(MIXED_STATS));
                }
                if !state.statistics.contains(&statistic) {
                    state.statistics.push(statistic.clone());
                }
                drop(state);
                drop(this);
                Ok(ud)
            });
        }

        // The open-ended half: `p99`, `p99.9`, `tm90`. A separate field
        // upstream (`ExtendedStatistics`), which AWS refuses to combine with
        // the five above, so mixing them raises here.
        methods.add_function("percentile", |_, (ud, p): (AnyUserData, String)| {
            let this = ud.borrow::<Self>()?;
            let mut state = this.state.lock().unwrap();
            if !state.statistics.is_empty() {
                return Err(mlua::Error::runtime(MIXED_STATS));
            }
            if !state.percentiles.contains(&p) {
                state.percentiles.push(p);
            }
            drop(state);
            drop(this);
            Ok(ud)
        });

        // Disambiguates a metric published under more than one unit. A unit
        // that does not match returns no datapoints, the same silence a wrong
        // dimension set produces.
        methods.add_function("unit", |_, (ud, unit): (AnyUserData, String)| {
            if !StandardUnit::values().contains(&unit.as_str()) {
                return Err(mlua::Error::runtime(format!(
                    "get_metric_statistics: unknown unit '{unit}', expected one of {}",
                    StandardUnit::values().join(", ")
                )));
            }
            ud.borrow::<Self>()?.state.lock().unwrap().unit = Some(StandardUnit::from(unit.as_str()));
            Ok(ud)
        });

        methods.add_async_function("send", |lua, ud: AnyUserData| async move {
            // The borrow is scoped: it must not be held across the await.
            let (client, namespace, metric_name, period, state) = {
                let this = ud.borrow::<Self>()?;
                (
                    this.client.clone(),
                    this.namespace.clone(),
                    this.metric_name.clone(),
                    this.period,
                    this.state.lock().unwrap().clone(),
                )
            };

            let Some(start_time) = state.start_time else {
                return Err(mlua::Error::runtime(
                    "get_metric_statistics: ':start_time()' is required",
                ));
            };
            // Resolved here rather than at construction, so a builder held
            // across an await still ends at the send.
            let end_time = state
                .end_time
                .unwrap_or_else(|| aws_smithy_types::DateTime::from(std::time::SystemTime::now()));
            check_range(start_time, end_time, period)?;

            let mut req = client
                .get_metric_statistics()
                .namespace(namespace)
                .metric_name(metric_name)
                .period(period)
                .start_time(start_time)
                .end_time(end_time);

            for d in state.dimensions {
                req = req.dimensions(d);
            }
            // Nothing asked for means the average -- unlike the period, there
            // is an obvious right answer.
            let statistics = if state.statistics.is_empty() && state.percentiles.is_empty() {
                vec![Statistic::Average]
            } else {
                state.statistics
            };
            for s in statistics {
                req = req.statistics(s);
            }
            for p in state.percentiles {
                req = req.extended_statistics(p);
            }
            if let Some(unit) = state.unit {
                req = req.unit(unit);
            }

            let resp = req.send().await.map_err(|e| lua_err(BoxError::from(e)))?;

            // The API returns datapoints unordered; sort oldest-first before
            // the script sees them.
            let mut datapoints = resp.datapoints.unwrap_or_default();
            datapoints.sort_by_key(|d| d.timestamp.map(|t| (t.secs(), t.subsec_nanos())));

            let out = lua.create_table()?;
            for (idx, point) in datapoints.into_iter().enumerate() {
                out.raw_set(idx + 1, datapoint_to_table(&lua, point)?)?;
            }

            respond(&lua, true, Value::Table(out))
        });
    }
}

impl UserData for ListMetrics {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_function("namespace", |_, (ud, v): (AnyUserData, String)| {
            ud.borrow::<Self>()?.state.lock().unwrap().namespace = Some(v);
            Ok(ud)
        });

        methods.add_function("metric_name", |_, (ud, v): (AnyUserData, String)| {
            ud.borrow::<Self>()?.state.lock().unwrap().metric_name = Some(v);
            Ok(ud)
        });

        // Here a dimension genuinely *is* a filter, unlike in
        // `get_metric_statistics`: naming one narrows the listing to metrics
        // carrying it, rather than demanding the set match exactly. The value
        // is optional -- `:dimension("InstanceId")` asks which metrics have
        // that dimension at all, whatever its value.
        methods.add_function(
            "dimension",
            |_, (ud, name, value): (AnyUserData, String, Option<String>)| {
                let mut f = DimensionFilter::builder().name(name);
                if let Some(value) = value {
                    f = f.value(value);
                }
                let f = f.build();
                ud.borrow::<Self>()?
                    .state
                    .lock()
                    .unwrap()
                    .dimensions
                    .push(f);
                Ok(ud)
            },
        );

        methods.add_function("dimensions", |_, (ud, table): (AnyUserData, Table)| {
            let mut built = Vec::new();
            for (name, value) in dimension_pairs(&table)? {
                built.push(DimensionFilter::builder().name(name).value(value).build());
            }
            ud.borrow::<Self>()?
                .state
                .lock()
                .unwrap()
                .dimensions
                .extend(built);
            Ok(ud)
        });

        // Metrics that published a datapoint in the last three hours -- the
        // only window the API offers, so this is a flag rather than a value.
        // Without it a listing includes metrics dead for up to two weeks.
        methods.add_function("recently_active", |_, ud: AnyUserData| {
            ud.borrow::<Self>()?.state.lock().unwrap().recently_active = true;
            Ok(ud)
        });

        // A total across pages; `envelope.truncated` says it cut the list.
        methods.add_function("max_metrics", |_, (ud, n): (AnyUserData, i64)| {
            if n <= 0 {
                return Err(mlua::Error::runtime(format!(
                    "list_metrics: ':max_metrics()' must be positive, got {n}"
                )));
            }
            ud.borrow::<Self>()?.state.lock().unwrap().max_metrics = Some(n as usize);
            Ok(ud)
        });

        methods.add_async_function("send", |lua, ud: AnyUserData| async move {
            let (client, state) = {
                let this = ud.borrow::<Self>()?;
                (this.client.clone(), this.state.lock().unwrap().clone())
            };

            let mut req = client.list_metrics();
            if let Some(v) = state.namespace {
                req = req.namespace(v);
            }
            if let Some(v) = state.metric_name {
                req = req.metric_name(v);
            }
            for d in state.dimensions {
                req = req.dimensions(d);
            }
            if state.recently_active {
                req = req.recently_active(aws_sdk_cloudwatch::types::RecentlyActive::Pt3H);
            }

            let out = lua.create_table()?;
            let mut idx = 0;
            // Page-level iteration: this paginator has no `.items()`.
            let mut truncated = false;
            let mut stream = req.into_paginator().send();
            'pages: while let Some(page) = stream.next().await {
                let page = page.map_err(|e| lua_err(BoxError::from(e)))?;
                for metric in page.metrics.unwrap_or_default() {
                    if state.max_metrics.is_some_and(|max| idx >= max) {
                        truncated = true;
                        break 'pages;
                    }
                    idx += 1;
                    out.raw_set(idx, metric_to_table(&lua, metric)?)?;
                }
                if state.max_metrics.is_some_and(|max| idx >= max) && page.next_token.is_some() {
                    truncated = true;
                    break;
                }
            }

            let resp = respond(&lua, true, Value::Table(out))?;
            if let Some(Value::Table(env)) = resp.iter().next() {
                env.raw_set("truncated", truncated)?;
            }
            Ok(resp)
        });
    }
}

/// Convert a datapoint, omitting every statistic that was not requested.
///
/// Absent rather than zero: `0` is a real reading, so conflating "not asked
/// for" with "measured zero" would make a script's own arithmetic wrong.
fn datapoint_to_table(lua: &Lua, point: Datapoint) -> LuaResult<Table> {
    let t = lua.create_table()?;

    if let Some(v) = point.timestamp.as_ref().and_then(date_to_string) {
        t.raw_set("timestamp", v)?;
    }
    if let Some(v) = point.average {
        t.raw_set("average", v)?;
    }
    if let Some(v) = point.sum {
        t.raw_set("sum", v)?;
    }
    if let Some(v) = point.minimum {
        t.raw_set("minimum", v)?;
    }
    if let Some(v) = point.maximum {
        t.raw_set("maximum", v)?;
    }
    if let Some(v) = point.sample_count {
        t.raw_set("sample_count", v)?;
    }
    if let Some(v) = point.unit {
        t.raw_set("unit", v.as_str().to_string())?;
    }

    // Keyed by the spelling that asked for them (`p99`), so a script reads
    // back what it wrote. Always a table, empty when none were requested --
    // the same "a collection is never nil" rule the parent module states.
    let percentiles = lua.create_table()?;
    for (name, value) in point.extended_statistics.unwrap_or_default() {
        percentiles.raw_set(name, value)?;
    }
    t.raw_set("percentiles", percentiles)?;

    Ok(t)
}

/// Convert a listed metric. `dimensions` comes back both as the `{[name] =
/// value}` map (the shape `get_metric_statistics:dimensions()` takes, so a
/// listing feeds a read directly) and, under `dimension_names`, as the names
/// alone in order — which is what tells you the *set* a metric is published
/// under, the thing an exact-match read has to reproduce.
fn metric_to_table(lua: &Lua, metric: Metric) -> LuaResult<Table> {
    let t = lua.create_table()?;

    if let Some(v) = metric.namespace {
        t.raw_set("namespace", v)?;
    }
    if let Some(v) = metric.metric_name {
        t.raw_set("metric_name", v)?;
    }

    let dimensions = lua.create_table()?;
    let names = lua.create_table()?;
    for (idx, d) in metric.dimensions.unwrap_or_default().into_iter().enumerate() {
        if let (Some(name), Some(value)) = (d.name, d.value) {
            names.raw_set(idx + 1, name.clone())?;
            dimensions.raw_set(name, value)?;
        }
    }
    t.raw_set("dimensions", dimensions)?;
    t.raw_set("dimension_names", names)?;

    Ok(t)
}

/// Build the `aws.cloudwatch` module table over `source`.
///
/// Called once for the `aws.cloudwatch` global (ambient configuration) and
/// again per `aws.config():build()`, so the surface cannot differ between
/// them.
pub(crate) fn module(lua: &Lua, source: ClientSource) -> LuaResult<Table> {
    let exports = lua.create_table()?;

    // The period joins the namespace and metric name as an argument rather
    // than a builder knob: the API requires it, and it decides the resolution
    // of the answer, so there is nothing safe to default it to.
    let c = source.clone();
    exports.raw_set(
        "get_metric_statistics",
        lua.create_async_function(
            move |_, (namespace, metric_name, period): (String, String, Value)| {
                let c = c.clone();
                async move {
                    // The one delta coercion, shared with `time` -- so `300`,
                    // `"5m"` and `time.minutes(5)` cannot disagree.
                    let secs = crate::scripting::time::parse_delta_value(
                        &period,
                        "get_metric_statistics: 'period'",
                    )?;
                    if secs.fract() != 0.0 {
                        return Err(mlua::Error::runtime(format!(
                            "get_metric_statistics: 'period' must be a whole number of seconds, got {secs}"
                        )));
                    }
                    // Validated before the client resolves credentials.
                    let period = period_secs(secs as i64)?;
                    Ok(GetMetricStatistics {
                        client: c.cloudwatch().await,
                        namespace,
                        metric_name,
                        period,
                        state: Arc::new(Mutex::new(StatisticsState::default())),
                    })
                }
            },
        )?,
    )?;

    // Every argument is optional and each only narrows the listing, so they
    // all live on the builder; a bare call is the account-wide read.
    let c = source.clone();
    exports.raw_set(
        "list_metrics",
        lua.create_async_function(move |_, args: Variadic<Value>| {
            let c = c.clone();
            async move {
                let mut state = ListMetricsState::default();
                // Sugar for the two-thirds of listings that name a namespace,
                // and occasionally a metric: `list_metrics("AWS/EC2")` reads
                // better than a builder call for the one argument that is
                // almost always present.
                let mut args = args.into_iter();
                if let Some(v) = args.next() {
                    state.namespace = from_optional_string("namespace", v)?;
                }
                if let Some(v) = args.next() {
                    state.metric_name = from_optional_string("metric_name", v)?;
                }
                Ok(ListMetrics {
                    client: c.cloudwatch().await,
                    state: Arc::new(Mutex::new(state)),
                })
            }
        })?,
    )?;

    Ok(exports)
}

/// Read a positional string argument (nil means unset), naming it in the error.
fn from_optional_string(name: &str, value: Value) -> LuaResult<Option<String>> {
    match value {
        Value::Nil => Ok(None),
        Value::String(s) => Ok(Some(s.to_str()?.to_string())),
        other => Err(mlua::Error::runtime(format!(
            "list_metrics: '{name}' must be a string, got {}",
            other.type_name()
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal config carrying only the behavior version the SDK requires
    /// at construction — enough to build a client for a test that never
    /// calls `:send()`.
    fn test_sdk_config() -> aws_config::SdkConfig {
        aws_config::SdkConfig::builder()
            .behavior_version(aws_config::BehaviorVersion::latest())
            .build()
    }

    fn statistics_builder(lua: &Lua) -> AnyUserData {
        let ud = lua
            .create_userdata(GetMetricStatistics {
                client: Client::new(&test_sdk_config()),
                namespace: "AWS/EC2".into(),
                metric_name: "CPUUtilization".into(),
                period: 300,
                state: Arc::new(Mutex::new(StatisticsState::default())),
            })
            .unwrap();
        lua.globals().raw_set("m", ud.clone()).unwrap();
        ud
    }

    fn state_of(ud: &AnyUserData) -> StatisticsState {
        ud.borrow::<GetMetricStatistics>()
            .unwrap()
            .state
            .lock()
            .unwrap()
            .clone()
    }

    #[test]
    fn accepts_the_periods_the_api_does() {
        for good in [1, 5, 10, 30, 60, 300, 3600] {
            assert!(period_secs(good).is_ok(), "{good} must be accepted");
        }
    }

    /// A period the API would reject is the script author's mistake, so it
    /// raises at the call with the rule named rather than after a round trip.
    #[test]
    fn rejects_periods_the_api_would_not() {
        for bad in [0, -60, 7, 45, 90, 61] {
            let err = period_secs(bad).unwrap_err().to_string();
            assert!(err.contains("must be 1, 5, 10, 30"), "{bad}: {err}");
        }
    }

    /// Dimensions accumulate rather than replacing, because CloudWatch matches
    /// the published set exactly -- one call per dimension is how a set is
    /// built.
    #[test]
    fn dimensions_accumulate() {
        let lua = Lua::new();
        let ud = statistics_builder(&lua);

        lua.load(r#"m:dimension("InstanceId", "i-1"):dimension("ImageId", "ami-2")"#)
            .exec()
            .unwrap();

        let dims = state_of(&ud).dimensions;
        assert_eq!(dims.len(), 2);
        assert_eq!(dims[0].name.as_deref(), Some("InstanceId"));
        assert_eq!(dims[0].value.as_deref(), Some("i-1"));
        assert_eq!(dims[1].name.as_deref(), Some("ImageId"));
    }

    /// The map form adds to whatever `:dimension()` already named, and sorts
    /// by name so one script does not send a different request each run.
    #[test]
    fn dimension_map_is_sorted_and_additive() {
        let lua = Lua::new();
        let ud = statistics_builder(&lua);

        lua.load(r#"m:dimension("Zzz", "last"):dimensions({ImageId = "ami-2", AutoScalingGroupName = "web"})"#)
            .exec()
            .unwrap();

        let names: Vec<_> = state_of(&ud)
            .dimensions
            .iter()
            .map(|d| d.name.clone().unwrap())
            .collect();
        assert_eq!(names, ["Zzz", "AutoScalingGroupName", "ImageId"]);
    }

    /// Each statistic is its own method, they accumulate, and a repeat is a
    /// no-op rather than a duplicate the API rejects.
    #[test]
    fn statistics_accumulate_without_duplicates() {
        let lua = Lua::new();
        let ud = statistics_builder(&lua);

        lua.load("m:average():maximum():average()").exec().unwrap();

        assert_eq!(
            state_of(&ud).statistics,
            vec![Statistic::Average, Statistic::Maximum]
        );
    }

    /// AWS rejects statistics combined with percentiles, so mixing raises
    /// (in either order); percentiles alone dedupe.
    #[test]
    fn percentiles_cannot_mix_with_statistics() {
        let lua = Lua::new();
        let ud = statistics_builder(&lua);
        lua.load(r#"m:percentile("p99"):percentile("p99")"#)
            .exec()
            .unwrap();
        assert_eq!(state_of(&ud).percentiles, vec!["p99"]);
        let err = lua.load("m:average()").exec().unwrap_err().to_string();
        assert!(err.contains("cannot be combined"), "{err}");

        let ud = statistics_builder(&lua);
        lua.load("m:average()").exec().unwrap();
        assert!(lua.load(r#"m:percentile("p99")"#).exec().is_err());
        assert!(state_of(&ud).percentiles.is_empty());
    }

    #[test]
    fn dimensions_dedupe_and_conflict() {
        let lua = Lua::new();
        let ud = statistics_builder(&lua);
        lua.load(r#"m:dimension("A", "1"):dimension("A", "1")"#)
            .exec()
            .unwrap();
        assert_eq!(state_of(&ud).dimensions.len(), 1);
        assert!(lua.load(r#"m:dimension("A", "2")"#).exec().is_err());
    }

    #[test]
    fn unit_is_validated() {
        let lua = Lua::new();
        statistics_builder(&lua);
        lua.load(r#"m:unit("Percent")"#).exec().unwrap();
        let err = lua.load(r#"m:unit("Nope")"#).exec().unwrap_err().to_string();
        assert!(err.contains("unknown unit"), "{err}");
    }

    #[test]
    fn range_is_checked() {
        use aws_smithy_types::DateTime;
        let t = DateTime::from_secs;
        assert!(check_range(t(0), t(3600), 300).is_ok());
        assert!(check_range(t(100), t(0), 300).is_err());
        assert!(check_range(t(0), t(0), 300).is_err());
        let err = check_range(t(0), t(30 * 86400), 300).unwrap_err().to_string();
        assert!(err.contains("1440"), "{err}");
    }

    #[test]
    fn times_accept_numbers_and_rfc3339() {
        let lua = Lua::new();
        let s = Value::String(lua.create_string("2023-11-14T22:13:20Z").unwrap());
        assert_eq!(crate::scripting::aws::time_arg("f", "t", &s).unwrap().secs(), 1_700_000_000);
        assert_eq!(crate::scripting::aws::time_arg("f", "t", &Value::Integer(5)).unwrap().secs(), 5);
        assert!(crate::scripting::aws::time_arg("f", "t", &Value::Nil).is_err());
    }

    #[test]
    fn list_metrics_nil_positional_is_unset() {
        assert_eq!(from_optional_string("n", Value::Nil).unwrap(), None);
        assert!(from_optional_string("n", Value::Boolean(true)).is_err());
    }

    /// A statistic that was not requested is absent, not zero -- `0` is a real
    /// reading and a script's arithmetic must be able to tell them apart.
    #[test]
    fn unrequested_statistics_are_absent() {
        let lua = Lua::new();
        let point = Datapoint::builder()
            .timestamp(aws_smithy_types::DateTime::from_secs(1_700_000_000))
            .average(42.5)
            .build();

        let t = datapoint_to_table(&lua, point).unwrap();

        assert_eq!(t.get::<String>("timestamp").unwrap(), "2023-11-14T22:13:20Z");
        assert_eq!(t.get::<f64>("average").unwrap(), 42.5);
        assert_eq!(t.get::<Value>("sum").unwrap(), Value::Nil);
        assert_eq!(t.get::<Value>("maximum").unwrap(), Value::Nil);
    }

    /// A zero reading survives as zero rather than being dropped as "absent".
    #[test]
    fn a_zero_reading_is_kept() {
        let lua = Lua::new();
        let point = Datapoint::builder().sum(0.0).build();

        let t = datapoint_to_table(&lua, point).unwrap();

        assert_eq!(t.get::<f64>("sum").unwrap(), 0.0);
    }

    /// Percentiles come back keyed by the spelling that asked for them, and
    /// the table is present even when empty.
    #[test]
    fn percentiles_round_trip_by_name() {
        let lua = Lua::new();
        let point = Datapoint::builder()
            .extended_statistics("p99", 12.5)
            .build();

        let t = datapoint_to_table(&lua, point).unwrap();
        assert_eq!(
            t.get::<Table>("percentiles")
                .unwrap()
                .get::<f64>("p99")
                .unwrap(),
            12.5
        );

        let empty = datapoint_to_table(&lua, Datapoint::builder().build()).unwrap();
        assert_eq!(empty.get::<Table>("percentiles").unwrap().raw_len(), 0);
    }

    /// A listed metric's dimensions arrive as the map a read takes, plus the
    /// names in order -- the set an exact-match read has to reproduce.
    #[test]
    fn listed_metric_exposes_map_and_names() {
        let lua = Lua::new();
        let metric = Metric::builder()
            .namespace("CWAgent")
            .metric_name("mem_used_percent")
            .dimensions(Dimension::builder().name("InstanceId").value("i-1").build())
            .dimensions(Dimension::builder().name("ImageId").value("ami-2").build())
            .build();

        let t = metric_to_table(&lua, metric).unwrap();

        assert_eq!(t.get::<String>("namespace").unwrap(), "CWAgent");
        assert_eq!(t.get::<String>("metric_name").unwrap(), "mem_used_percent");

        let dims = t.get::<Table>("dimensions").unwrap();
        assert_eq!(dims.get::<String>("InstanceId").unwrap(), "i-1");
        assert_eq!(dims.get::<String>("ImageId").unwrap(), "ami-2");

        let names = t.get::<Table>("dimension_names").unwrap();
        assert_eq!(names.raw_len(), 2);
        assert_eq!(names.get::<String>(1).unwrap(), "InstanceId");
        assert_eq!(names.get::<String>(2).unwrap(), "ImageId");
    }

    /// A metric with no dimensions still exposes both tables, never nil.
    #[test]
    fn metric_without_dimensions_has_empty_tables() {
        let lua = Lua::new();
        let t = metric_to_table(&lua, Metric::builder().metric_name("m").build()).unwrap();

        assert_eq!(t.get::<Table>("dimension_names").unwrap().raw_len(), 0);
        assert!(t.get::<Table>("dimensions").is_ok());
    }
}
