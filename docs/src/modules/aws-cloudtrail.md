# `aws.cloudtrail` — AWS CloudTrail

| Function | Effect |
| --- | --- |
| `aws.cloudtrail.lookup_events()` | new `LookupEvents` builder |
| `:event_name(<value>)` | filter by `EventName` |
| `:event_id(<value>)` | filter by `EventId` |
| `:event_source(<value>)` | filter by `EventSource` |
| `:username(<value>)` | filter by `Username` |
| `:access_key_id(<value>)` | filter by `AccessKeyId` |
| `:resource_name(<value>)` | filter by `ResourceName` |
| `:resource_type(<value>)` | filter by `ResourceType` |
| `:read_only([<bool>])` | filter by `ReadOnly` (a strict boolean; nil = `true`) |
| `:filter(<key>, <value>)` | escape hatch: any known lookup attribute key by name |
| `:start_time(<time>)` | only events at or after this time |
| `:end_time(<time>)` | only events at or before this time |
| `:insights([<bool>])` | return Insights events instead of management events (`false` undoes) |
| `:max_events(<n>)` | stop after `n` events in total |
| `:send()` | finalize the builder, run the request |

## Example

```lua
local r, events = aws.cloudtrail.lookup_events()
    :resource_name("web-prod")
    :start_time(time.now() - time.hours(1))
    :send()

for _, e in ipairs(events) do
    print(e.event_time, e.username, e.event_name)
end
```

## Conventions

`aws.cloudtrail` has one entry point, `lookup_events`: "what happened,
and who/what did it" — the API call history behind a change, as opposed
to [`aws.autoscaling`](aws-autoscaling.md)'s own scaling-activity log,
which explains *what* a group did but not *who* asked for it.
`lookup_events` reads from CloudTrail's default 90-day event history and
needs no trail or S3 bucket configured, unlike a full trail export.

**Builder + `:send()`, two return values**, as documented for the whole
`aws` surface: `:send()` either raises or returns `(envelope, events)`.
`envelope.ok` is always `true` — a lookup that matched nothing is an
empty table, not a failure. `lookup_events()` returns a mutable builder;
each filter/time method mutates it in place and returns the same handle,
and validates its argument at the call. `:send()` is async and is the
only place a request (or an AWS error) happens.

**At most one lookup attribute.** CloudTrail's own `LookupAttributes`
accepts at most one attribute per request — asking for two is a
service-side error, not a limitation worked around here. Each typed
helper (`:event_name`, `:username`, …) and the generic `:filter(<key>,
<value>)` set the same single slot; setting it a second time raises.
`:filter()` rejects keys CloudTrail does not know.

**Times** (`:start_time`/`:end_time`) are Unix seconds or an RFC 3339
string; `:send()` raises if the start is after the end.

**Pacing.** Pages are fetched with a 500 ms pause between them, to stay
under the 2-lookups-per-second limit. `envelope.truncated` is `true`
when `:max_events()` cut a longer result short.

**`:max_events(n)` caps the total across pages**, not each page. A
lookup is otherwise unbounded — account-wide and time-unbounded, it is
the whole 90-day history — so this stops the walk once `n` events are in
hand. Omitted, pagination runs to exhaustion.

**Rate limit.** CloudTrail allows two lookups per second per account per
Region, well below what the other `aws` modules tolerate. Fanning
lookups out over [`task`](task.md) will throttle; keep the concurrency
low.

**Default vs explicit client.** `aws.cloudtrail` is backed by the
*ambient* configuration, resolved lazily on first use and cached. To
reach more than one region or account from a single script, build an
explicit client with [`aws.config`](aws-config.md); it carries a
`cloudtrail` key with exactly the surface documented here.

**Errors.** AWS API failures — bad credentials, denied access,
connectivity, a malformed request — raise a Luau error whose message is
the SDK's error chain flattened into one string. `:max_events()` raises
if given a non-positive number.

## `aws.cloudtrail.lookup_events`

List CloudTrail management events (or, under `:insights()`, Insights
events) matching at most one lookup attribute and an optional time
range.

**Signature:** `aws.cloudtrail.lookup_events() -> builder`

**Parameters:** none. Configure the request through the builder's
methods, below.

**Methods:**

- `:event_name(<value>)`, `:event_id(<value>)`, `:event_source(<value>)`,
  `:username(<value>)`, `:access_key_id(<value>)`,
  `:resource_name(<value>)`, `:resource_type(<value>)` — each sets
  CloudTrail's one lookup attribute to that key/value pair.
- `:read_only([<bool>])` — filter to write events (`false`) or read events
  (`true`, also the default for nil); also sets the one lookup attribute.
  Anything but a boolean raises.
- `:filter(<key>, <value>)` — the escape hatch: any lookup attribute key
  by name (`"EventName"`, `"Username"`, …), including one without a
  typed helper above.
- `:start_time(<time>)`/`:end_time(<time>)` — bound the lookup to events
  at or after/before this time, given as seconds since the epoch
  (fractional allowed) or an RFC 3339 string — so
  `time.now() - time.hours(1)` works directly.
- `:insights([<bool>])` — return Insights events **instead of** management
  events; no lookup returns both. Insights events support only
  `:event_id`, `:event_name` and `:event_source` as attributes.
- `:max_events(<n>)` — stop after `n` events in total (see
  [Conventions](#conventions)). Raises if `n` is not positive.
- `:send()` — finalize the builder and run the request (async).

Only one of the typed attribute helpers/`:filter()` may be called:
CloudTrail accepts at most one `LookupAttribute` per request, so a second
call raises.

**Returns:** `(envelope, events)` — `events` is an array of event
tables, newest first, always present even when empty:

| --- | --- |
| `event_id` | string |
| `event_name` | string |
| `read_only` | boolean |
| `access_key_id` | string |
| `event_source` | string |
| `username` | string |
| `event_time` | string (RFC 3339) |
| `error_code`, `aws_region`, `source_ip` | string, promoted from `raw` when present |
| `resources` | table (array of `{resource_type, resource_name}`) |
| `raw` | table or string |

`resources` is always a table, possibly empty — the resources an event
names (an ASG, an instance, a trail, …), each entry's `resource_type`
absent when CloudTrail could not determine it. `raw` is the event's own
`CloudTrailEvent` JSON, **parsed** into Luau data (keys keep CloudTrail's
camelCase, e.g. `raw.userIdentity`; JSON `null` fields are absent); a string that fails to
parse (never expected, but not asserted either) comes back as the raw
string rather than failing the whole lookup, the same trade-off
[`aws.s3`](aws-s3.md)'s body parsing makes.

**Errors:** raises on any AWS API error (credentials, permissions,
connectivity, an invalid time range).

```lua
local r, events = aws.cloudtrail.lookup_events()
    :username("alice")
    :start_time(time.now() - time.hours(24))
    :max_events(20)
    :send()

for _, e in ipairs(events) do
    print(e.event_time, e.event_name, e.resources[1] and e.resources[1].resource_name)
end
```
