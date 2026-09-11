# `aws.cloudwatch` — AWS CloudWatch

| Function | Effect |
| --- | --- |
| `aws.cloudwatch.get_metric_statistics(<namespace>, <metric>, <period>)` | new `GetMetricStatistics` builder |
| `:dimension(<name>, <value>)` | add one dimension to the match set |
| `:dimensions({[<name>] = <value>})` | add several dimensions at once |
| `:start_time(<time>)` | the start of the range (required) |
| `:end_time(<time>)` | the end of the range (defaults to now) |
| `:average()`, `:sum()`, `:minimum()`, `:maximum()`, `:sample_count()` | request a statistic |
| `:percentile(<spec>)` | request a percentile (`"p99"`); cannot be combined with statistics |
| `:unit(<unit>)` | only datapoints published in this unit |
| `:send()` | finalize the builder, run the request |
| `aws.cloudwatch.list_metrics([<namespace>[, <metric>]])` | new `ListMetrics` builder |
| `:namespace(<value>)` | restrict to a namespace |
| `:metric_name(<value>)` | restrict to a metric name |
| `:dimension(<name>[, <value>])` | restrict to metrics carrying this dimension |
| `:dimensions({[<name>] = <value>})` | restrict by several dimensions at once |
| `:recently_active()` | only metrics with data in the last 3 hours |
| `:max_metrics(<n>)` | stop after `n` metrics in total; sets `envelope.truncated` |
| `:send()` | finalize the builder, run the request |

## Example

```lua
local r, points = aws.cloudwatch.get_metric_statistics("AWS/EC2", "CPUUtilization", "5m")
    :dimension("InstanceId", "i-0abc123")
    :start_time(time.now() - time.hours(3))
    :average()
    :send()

for _, p in ipairs(points) do
    print(p.timestamp, p.average)
end
```

## Conventions

`aws.cloudwatch` covers **metrics, read-only**: what a metric was doing
(`get_metric_statistics`) and which metrics exist (`list_metrics`).
Alarms are deliberately not exposed.

**Builder + `:send()`, two return values**, as documented for the whole
`aws` surface: `:send()` either raises or returns `(envelope, data)`.
`envelope.ok` is always `true` — a query that matched nothing is an empty
table, not a failure. The entry call returns a mutable builder; each
configuration method mutates it in place and returns the same handle, and
validates its argument at the call. `:send()` is async and is the only
place a request (or an AWS error) happens.

**Dimensions are an exact match, not a filter.** This is the sharpest
edge in the whole service, so it is worth stating plainly: CloudWatch
stores a metric under the *full* set of dimensions it was published with,
and `get_metric_statistics` matches that set **exactly**. A metric
published with `InstanceId` *and* `ImageId` *and* `InstanceType` is not
found by asking for `InstanceId` alone — the result is **no datapoints
and no error**, which reads exactly like "the metric was flat".

Nothing here papers over that: detecting it would mean a `list_metrics`
call behind every read, and inventing an error for an answer the service
considers valid. Use `list_metrics` to find out which sets exist — the
`dimension_names` it returns is precisely the set a read has to
reproduce. Note that `list_metrics`'s own `:dimension()` genuinely *is* a
filter, so the two calls differ here on purpose.

**The period is an argument, not a builder knob.** It sits beside the
namespace and metric name because the API requires it and because it
decides the *resolution* of the answer — there is nothing safe to default
it to. It accepts anything [`time`](time.md)'s delta grammar does, so
`300`, `"5m"` and `time.minutes(5)` are one value spelled three ways. It
must be 1, 5, 10 or 30 seconds (high-resolution metrics only) or a
multiple of 60; anything else raises at the call rather than after a
round trip.

**`:end_time()` defaults to now**, resolved when `:send()` runs. A start
time alone is the overwhelmingly common shape and "up to the present" is
the only thing an absent end could have meant. `:start_time()` has no
default and `:send()` raises without it.

**The 1440-datapoint cap.** `GetMetricStatistics` does not paginate: it
returns at most 1440 datapoints and errors beyond that, so a range
divided by too small a period fails rather than truncating — `:send()`
raises locally before calling AWS. There is consequently no
`:max_points()` here. Widen the period to fit: 3 hours at `"5m"` is 36
points, 30 days at `"5m"` is 8640 and will fail.

**Retention vs period.** CloudWatch keeps data at a resolution that
coarsens with age: 1-second data for 3 hours, 1-minute for 15 days,
5-minute for 63 days, 1-hour for 15 months. A period finer than the
retained resolution for that age returns no datapoints, no error.

**Period** must be a whole number of seconds; `1.5` raises rather than
being rounded.

**Rate limit.** `GetMetricStatistics` is throttled per account and
Region (the default quota is on the order of hundreds of requests per
second, shared with other CloudWatch reads). Fanning out over
[`task`](task.md) is fine at modest concurrency, but a large fleet swept
at full width will throttle; the SDK retries throttled calls.

**Default vs explicit client.** `aws.cloudwatch` is backed by the
*ambient* configuration, resolved lazily on first use and cached. To
reach more than one region or account from a single script, build an
explicit client with [`aws.config`](aws-config.md); it carries a
`cloudwatch` key with exactly the surface documented here.

**Errors.** AWS API failures — bad credentials, denied access,
connectivity, a malformed request — raise a Luau error whose message is
the SDK's error chain flattened into one string. An invalid period, a
missing `:start_time()`, or a non-string positional to `list_metrics`
raise at the offending call.

## `aws.cloudwatch.get_metric_statistics`

Read one metric over a time range, aggregated into buckets of `period`.

**Signature:** `aws.cloudwatch.get_metric_statistics(<namespace>,
<metric>, <period>) -> builder`

**Parameters:**

- `<namespace>` — the metric's namespace (`"AWS/EC2"`, `"CWAgent"`, …).
- `<metric>` — the metric name (`"CPUUtilization"`).
- `<period>` — the bucket width, as a delta: a number of seconds, a
  duration string (`"5m"`), or a [`time`](time.md) delta. Must be 1, 5,
  10, 30, or a multiple of 60 seconds.

**Methods:**

- `:dimension(<name>, <value>)` — add one dimension. Repeatable, and
  **accumulating**: each call adds to the set rather than replacing it,
  because the set must match what was published exactly. An exact repeat
  is a no-op; the same name with a different value raises.
- `:dimensions({[<name>] = <value>})` — add several at once, from the
  `{[key] = value}` map shape the rest of the `aws` surface uses for
  tags. Adds to whatever `:dimension()` already named; sorted by name, so
  one script does not send a differently-ordered request each run.
- `:start_time(<time>)` — the start of the range, in seconds since the
  epoch (fractional allowed) or an RFC 3339 string, so
  `time.now() - time.hours(3)` works directly. **Required.**
- `:end_time(<time>)` — the end of the range. Defaults to now. `:send()`
  raises if the start is not before the end.
- `:average()`, `:sum()`, `:minimum()`, `:maximum()`, `:sample_count()` —
  request that statistic; each becomes a field on every datapoint. They
  accumulate, and a repeated call is a no-op. Requesting none at all
  means `:average()`.
- `:percentile(<spec>)` — request a percentile (`"p99"`, `"p99.9"`,
  `"tm90"`). A separate field upstream that AWS refuses to combine with
  the five named statistics, so mixing the two raises (in either order).
- `:unit(<unit>)` — only datapoints published in this unit (`"Percent"`,
  `"Bytes"`, `"Count"`, …); an unknown unit raises. Disambiguates a metric published under more
  than one; a unit that matches nothing returns no datapoints, the same
  silence a wrong dimension set produces.
- `:send()` — finalize the builder and run the request (async).

**Returns:** `(envelope, datapoints)` — `datapoints` is an array, sorted
by timestamp **oldest first** (the API itself returns them unordered),
always present even when empty:

| --- | --- |
| `timestamp` | string (RFC 3339) |
| `average` | number, only if requested |
| `sum` | number, only if requested |
| `minimum` | number, only if requested |
| `maximum` | number, only if requested |
| `sample_count` | number, only if requested |
| `unit` | string |
| `percentiles` | table, `{["p99"] = 12.3}` |

A statistic that was not requested is **absent, not zero** — `0` is a
real reading, so conflating the two would make a script's own arithmetic
wrong. `percentiles` is always a table, empty when none were requested,
keyed by the spelling that asked for them.

**Errors:** raises on any AWS API error; on a period the API would
reject; on a missing `:start_time()`; and on a range that, divided by
the period, exceeds 1440 datapoints (checked locally); on mixing
statistics with percentiles; on an unknown unit; and on a start not
before the end.

```lua
local r, points = aws.cloudwatch.get_metric_statistics("AWS/EC2", "NetworkIn", "1h")
    :dimension("InstanceId", "i-0abc123")
    :start_time(time.now() - time.days(1))
    :percentile("p99")
    :send()

for _, p in ipairs(points) do
    print(p.timestamp, p.percentiles["p99"])
end
```

## `aws.cloudwatch.list_metrics`

List the metrics published in the account — the discovery call, and the
way to find the dimension set `get_metric_statistics` has to match.

**Signature:** `aws.cloudwatch.list_metrics([<namespace>[, <metric>]])
-> builder`

**Parameters:** both optional (nil means unset) and both sugar for the builder methods of
the same name — a namespace is named by almost every listing, so
`list_metrics("AWS/EC2")` reads better than a builder call for it. A bare
`list_metrics()` is the account-wide listing.

**Methods:**

- `:namespace(<value>)`, `:metric_name(<value>)` — restrict the listing;
  the same knobs as the positional arguments.
- `:dimension(<name>[, <value>])` — restrict to metrics carrying this
  dimension. Unlike `get_metric_statistics`'s method of the same name
  this **is** a filter, not an exact-match set: the value is optional, so
  `:dimension("InstanceId")` asks which metrics have that dimension at
  all, whatever its value.
- `:dimensions({[<name>] = <value>})` — several name/value filters at
  once.
- `:recently_active()` — only metrics that published a datapoint in the
  last 3 hours. Three hours is the only window the API offers, which is
  why this is a flag and not a value. Without it, a listing includes
  metrics that have been dead for up to two weeks.
- `:max_metrics(<n>)` — stop after `n` metrics in total (across pages);
  `envelope.truncated` is `true` when a longer list was cut. Raises if `n`
  is not positive.
- `:send()` — finalize the builder and run the request (async).

**Returns:** `(envelope, metrics)` — `metrics` is an array, paginated to
exhaustion, always present even when empty:

| --- | --- |
| `namespace` | string |
| `metric_name` | string |
| `dimensions` | table, `{[name] = value}` |
| `dimension_names` | table (array of names, in order) |

`dimensions` is the map `get_metric_statistics:dimensions()` takes, so a
listing feeds a read directly. `dimension_names` is the names alone,
which is what tells you the *set* a metric is published under — the thing
an exact-match read has to reproduce. Both are always tables, possibly
empty.

**Errors:** raises on any AWS API error, and on a positional argument
that is not a string.

```lua
-- Which dimension sets does this metric actually exist under?
local r, metrics = aws.cloudwatch.list_metrics("CWAgent", "mem_used_percent")
    :recently_active()
    :send()

for _, m in ipairs(metrics) do
    print(table.concat(m.dimension_names, ", "))
end

-- ...then read one, reproducing its set exactly.
local _, points = aws.cloudwatch.get_metric_statistics("CWAgent", "mem_used_percent", "5m")
    :dimensions(metrics[1].dimensions)
    :start_time(time.now() - time.hours(1))
    :average()
    :send()
```
