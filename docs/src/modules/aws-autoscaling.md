# `aws.autoscaling` — AWS EC2 Auto Scaling

| Function/Method | Effect |
| --- | --- |
| `aws.autoscaling.describe_groups()` | list Auto Scaling groups (paginated) |
| `:name(<value>)` | filter `describe_groups` to one or more group names |
| `:tag(<key>, <value>)` | filter `describe_groups` by `tag:<key>` |
| `:tag_key(<key>)` | filter `describe_groups` by `tag-key` |
| `:filter(<name>, <value>)` | filter `describe_groups` by any named filter |
| `aws.autoscaling.describe_instances([<ids>])` | list Auto Scaling instances (paginated), each with its group name |
| `aws.autoscaling.set_desired_capacity(<group>, <n>)` | move a group to `<n>` within its existing bounds |
| `:honor_cooldown()` | wait out the cooldown instead of scaling immediately |
| `aws.autoscaling.update_group(<group>)` | patch a group's bounds/cooldown/grace period |
| `:min_size(<n>)` | set the group's minimum size |
| `:max_size(<n>)` | set the group's maximum size |
| `:desired_capacity(<n>)` | set the group's desired capacity |
| `:health_check_grace_period(<secs>)` | set the health-check grace period |
| `:default_cooldown(<secs>)` | set the default scaling cooldown |
| `aws.autoscaling.start_instance_refresh(<group>)` | begin replacing a group's instances in batches |
| `:min_healthy_percentage(<n>)` | minimum capacity to keep in service during the refresh |
| `:max_healthy_percentage(<n>)` | maximum capacity above desired during the refresh |
| `:instance_warmup(<secs>)` | settling time before a new instance counts healthy |
| `:skip_matching()` | leave instances already matching the desired configuration |
| `:auto_rollback()` | roll back automatically on failure |
| `:strategy(<name>)` | the refresh strategy, e.g. `"Rolling"` |
| `:wait()` | poll until the refresh reaches a terminal state |
| `aws.autoscaling.describe_instance_refreshes(<group>)` | list a group's instance refreshes (paginated) |
| `:id(<id>)` | narrow to one refresh id |
| `aws.autoscaling.cancel_instance_refresh(<group>)` | stop a group's in-progress refresh |
| `aws.autoscaling.rollback_instance_refresh(<group>)` | revert a group's refresh to its prior configuration |
| `aws.autoscaling.create_or_update_tags(<group>)` | set tags on a group (not on its instances) |
| `:tags(<map>)` | the `{[key] = value}` tag set to write; required |
| `:propagate_at_launch([<bool>])` | copy these tags onto newly launched instances |
| `aws.autoscaling.delete_tags(<group>)` | remove tags from a group |
| `:tags(<keys-or-map>)` | key array or `{[key] = value}` map to remove; required |

Every entry function returns a builder finalized by `:send()`, which
either raises or returns `(envelope, data)`.

```lua
local r, groups = aws.autoscaling.describe_groups():name("web-prod"):send()
local asg = groups[1]

print(asg.desired_capacity, asg.min_size, asg.max_size)
for _, i in ipairs(asg.instances) do
    print(i.instance_id, i.lifecycle_state, i.health_status)
end

aws.autoscaling.set_desired_capacity("web-prod", 6):send()
```

## Conventions

Its own key rather than a nesting under [`aws.ec2`](aws-ec2.md): despite
the "EC2 Auto Scaling" branding this is a separate API, with its own
endpoint, SDK crate and IAM prefix (`autoscaling:*`), so nesting it would
be the one place the `aws` table stops mirroring the SDK.

`aws.autoscaling` reads the *ambient* AWS configuration (profile/region/
credentials from the environment, `~/.aws/config`, SSO or IMDS), resolved
lazily on first call and cached. [`aws.config()`](aws-config.md) builds an
explicit client instead, carrying the same `autoscaling` key with this
identical surface — `aws.config():region("eu-central-1"):build().autoscaling`
— for a script that has to span regions or accounts.

Every `:send()` either raises or returns **two** values: an envelope table
carrying `ok`, and the response as native Luau data. Calls that return
nothing on the wire (`set_desired_capacity`, `update_group`,
`create_or_update_tags`, `delete_tags`) yield `nil`
as the second value — the envelope alone is the answer. `describe_groups`,
`describe_instances` and `describe_instance_refreshes` follow pagination
to exhaustion before `:send()` returns; there is no manual paging.

AWS API failures — bad credentials, denied access, connectivity, a
malformed request — raise a Luau error whose message is the SDK's error
chain flattened into one string (top-level message, then each `source()`,
deduplicated). `cancel_instance_refresh`/`rollback_instance_refresh` raise
when there is no refresh in progress on the group; `update_group` raises
if called with no knob set, since that would otherwise be a request that
silently does nothing.

## `describe_groups`

List Auto Scaling groups, optionally filtered.

**Signature:** `aws.autoscaling.describe_groups() -> builder`

**Parameters:** none. Filtering is added via the builder's methods before
`:send()`.

**Builder methods:**

- `:name(<value>)` — one group name or an array of names (`value_to_string_list`:
  a bare string or a table of strings). Repeatable; names accumulate.
  Omitted, every group in the region/account is described.
- `:tag(<key>, <value>)` — filters on `tag:<key>`, matching `<value>` (a
  string or array of strings), the same spelling `aws.ec2` uses.
- `:tag_key(<key>)` — filters on `tag-key`, matching any group carrying
  that key (a string or array of strings).
- `:filter(<name>, <value>)` — any named filter the API accepts, for
  filters not covered by `:tag`/`:tag_key`.

**Returns:** `(envelope, groups)` — `groups` is an array of group tables
(see below), always present even when empty.

**Errors:** raises on any AWS API error (credentials, permissions,
connectivity).

```lua
local r, groups = aws.autoscaling.describe_groups()
    :tag("Environment", "prod")
    :send()

for _, g in ipairs(groups) do
    print(g.name, g.desired_capacity)
end
```

### The group table

| Field | Type |
| --- | --- |
| `name` | string |
| `arn` | string |
| `min_size` | number |
| `max_size` | number |
| `desired_capacity` | number |
| `health_check_type` | string |
| `health_check_grace_period` | number |
| `default_cooldown` | number |
| `launch_configuration_name` | string |
| `launch_template` | table (`name`, `version`) |
| `vpc_zone_identifier` | string |
| `status` | string |
| `new_instances_protected_from_scale_in` | boolean |
| `created_time` | string (RFC 3339) |
| `availability_zones` | table (array) |
| `target_group_arns` | table (array) |
| `tags` | table (map, `{[key] = value}`) |
| `tag_details` | table (array of `{key, value, propagate_at_launch}`) |
| `instances` | table (array) |

Every field is present only when the API returned it, except
`availability_zones`, `target_group_arns`, `tags`, `tag_details` and
`instances`, which are always tables — possibly empty — so indexing them
needs no guard.

The group's tags arrive **twice over**, because neither shape answers both
questions. `tags` is the ergonomic read: the `{[key] = value}` map every
other module hands back, what `g.tags.Name` reads, and exactly what
`create_or_update_tags` takes straight back, so a read/modify/write
round-trip needs no reshaping. A key whose value the API omitted is absent
from the map, there being no value to map it to. `tag_details` is the
lossless read: one entry per tag, in the order the API listed them,
carrying `key` (always), `value` (only when the API returned one) and
`propagate_at_launch` (only when the API stated it — absent means "the API
did not say", **not** false). `propagate_at_launch` is visible on
`tag_details` alone; a map has nowhere to put it, and silently dropping it
turned a round-trip through us into an un-propagating copy of the group's
tags.

Each entry in `instances` carries `instance_id`, `instance_type`,
`availability_zone`, `lifecycle_state` (`InService`, `Pending`,
`Terminating`, …), `health_status` (`Healthy`/`Unhealthy`), `image_id`
and `protected_from_scale_in`.

> **The embedded `instances` array is capped at 50 by the API.** For a
> group larger than that it is incomplete, silently. Use
> `describe_instances` for the uncapped view.

## `describe_instances`

List Auto Scaling instances directly — the uncapped counterpart to a
group table's embedded `instances`, and the only way to ask the reverse
question (which group an instance belongs to), since each row carries
`group_name`.

**Signature:** `aws.autoscaling.describe_instances([<ids>]) -> builder`

**Parameters:**

- `ids` (optional) — a string or array of instance ids. `nil`/omitted
  describes every Auto Scaling instance in the account.

**Returns:** `(envelope, instances)` — an array of instance-detail
tables: `instance_id`, `instance_type`, `group_name`,
`availability_zone`, `lifecycle_state`, `health_status`, `image_id`,
`protected_from_scale_in` (each present only when the API returned it).

**Errors:** raises on any AWS API error.

```lua
local r, instances = aws.autoscaling.describe_instances({ "i-abc" }):send()
print(instances[1].group_name)
```

## `set_desired_capacity`

Move a group to a size **within its existing `min_size`/`max_size`
bounds**. Kept apart from `update_group` deliberately: asking for 20
instances in a group whose `max_size` is 10 should say so rather than
silently raise the ceiling.

**Signature:** `aws.autoscaling.set_desired_capacity(<group>, <n>) -> builder`

**Parameters:**

- `group` (string) — the Auto Scaling group name.
- `n` (number) — the desired capacity.

**Builder methods:**

- `:honor_cooldown()` — wait out the group's cooldown period instead of
  scaling now. Off by default: the API's own default is off, and a script
  asking for a capacity usually means now, not "once the cooldown
  allows it".

**Returns:** `(envelope, nil)` — the API returns nothing; the envelope
alone is the answer.

**Errors:** raises if `n` is outside `[min_size, max_size]`, or on any
other AWS API error.

```lua
aws.autoscaling.set_desired_capacity("web-prod", 6):send()
```

## `update_group`

Patch a group's bounds, desired capacity, health-check grace period or
default cooldown — the call that reshapes the group itself, as opposed to
`set_desired_capacity`'s routine resize within it.

**Signature:** `aws.autoscaling.update_group(<group>) -> builder`

**Parameters:**

- `group` (string) — the Auto Scaling group name.

**Builder methods** (every one optional; an unset knob is left alone, so
this is a patch, not a replacement):

- `:min_size(<n>)`
- `:max_size(<n>)`
- `:desired_capacity(<n>)`
- `:health_check_grace_period(<secs>)`
- `:default_cooldown(<secs>)`

**Returns:** `(envelope, nil)` — the API returns nothing.

**Errors:** raises with a message naming the available methods if
`:send()` is called with no knob set at all — that call would otherwise
be a request that silently does nothing. Also raises on any AWS API
error (e.g. `min_size` greater than `max_size`).

```lua
aws.autoscaling.update_group("web-prod")
    :min_size(4)
    :max_size(20)
    :send()
```

## Instance refresh

A refresh replaces a group's instances in batches. It is asynchronous:
`start_instance_refresh` returns a refresh id, and progress arrives
through `describe_instance_refreshes` — or through `:wait()` on the start
builder itself.

### `start_instance_refresh`

Begin an instance refresh on a group.

**Signature:** `aws.autoscaling.start_instance_refresh(<group>) -> builder`

**Parameters:**

- `group` (string) — the Auto Scaling group name.

**Builder methods:**

- `:min_healthy_percentage(<n>)` — minimum percentage of capacity that
  must stay in service during the refresh.
- `:max_healthy_percentage(<n>)` — maximum percentage of capacity allowed
  above desired capacity during the refresh.
- `:instance_warmup(<secs>)` — seconds to wait after an instance reaches
  `InService` before counting it healthy; falls back to the group's own
  warmup when unset.
- `:skip_matching()` — leave instances that already match the desired
  configuration alone, rather than replacing them too.
- `:auto_rollback()` — roll back automatically if the refresh fails.
- `:strategy(<name>)` — the refresh strategy, e.g. `"Rolling"`.
- `:wait()` — poll `describe_instance_refreshes` (every 15 seconds) until
  the refresh reaches a terminal state, instead of returning the id and
  leaving the loop to the script. It yields rather than blocking, so
  several groups can refresh concurrently under [`task`](task.md).

**Returns:**

- Without `:wait()`: `(envelope, { instance_refresh_id = <id> })`, `ok`
  is simply true — the refresh was accepted, nothing is known yet about
  how it ends.
- With `:wait()`: `(envelope, refresh)` once the refresh reaches a
  terminal state — `refresh` is the refresh table (see
  `describe_instance_refreshes` below). `ok` reflects the **outcome**:
  true for `Successful`/`RollbackSuccessful`, false for
  `Failed`/`Cancelled`/`RollbackFailed`. A failed or cancelled refresh is
  a real answer to "how did it go", not an error to raise on.

**Errors:** raises on any AWS API error starting the refresh. Under
`:wait()`, also raises if the refresh disappears from
`describe_instance_refreshes` while polling (an unexpected condition, not
a normal failure mode).

```lua
local r, refresh = aws.autoscaling.start_instance_refresh("web-prod")
    :min_healthy_percentage(90)
    :auto_rollback()
    :wait()
    :send()

if not r.ok then
    error("refresh " .. refresh.status .. ": " .. (refresh.status_reason or ""))
end
```

### `describe_instance_refreshes`

List a group's instance refreshes, newest first.

**Signature:** `aws.autoscaling.describe_instance_refreshes(<group>) -> builder`

**Parameters:**

- `group` (string) — the Auto Scaling group name.

**Builder methods:**

- `:id(<id>)` — narrow the listing to one refresh id. Repeatable.

**Returns:** `(envelope, refreshes)` — an array of refresh tables, newest
first, as the API returns them:

| Field | Type |
| --- | --- |
| `instance_refresh_id` | string |
| `group_name` | string |
| `status` | string (`Pending`, `InProgress`, `Baking`, `Cancelling`, `RollbackInProgress`, `Successful`, `Failed`, `Cancelled`, `RollbackSuccessful`, `RollbackFailed`, …) |
| `status_reason` | string |
| `percentage_complete` | number |
| `instances_to_update` | number |
| `start_time` | string (RFC 3339) |
| `end_time` | string (RFC 3339) |

**Errors:** raises on any AWS API error.

```lua
local r, refreshes = aws.autoscaling
    .describe_instance_refreshes("web-prod"):send()

print(refreshes[1].status, refreshes[1].percentage_complete)
```

### `cancel_instance_refresh`

Stop a group's in-progress refresh, leaving already-replaced instances
replaced.

**Signature:** `aws.autoscaling.cancel_instance_refresh(<group>) -> builder`

**Parameters:**

- `group` (string) — the Auto Scaling group name.

**Returns:** `(envelope, { instance_refresh_id = <id> })`.

**Errors:** raises when there is no refresh in progress on the group, or
on any other AWS API error.

```lua
aws.autoscaling.cancel_instance_refresh("web-prod"):send()
```

### `rollback_instance_refresh`

Return a group to the configuration in place before its current refresh
started.

**Signature:** `aws.autoscaling.rollback_instance_refresh(<group>) -> builder`

**Parameters:**

- `group` (string) — the Auto Scaling group name.

**Returns:** `(envelope, { instance_refresh_id = <id> })`.

**Errors:** raises when there is no refresh in progress on the group, or
on any other AWS API error.

```lua
aws.autoscaling.rollback_instance_refresh("web-prod"):send()
```

## Tagging

> **A group's tags are not the EC2 tags on the instances it launches.**
> They live in this API, keyed by group name, and
> [`aws.ec2`](aws-ec2.md)'s `describe_instances` will never show them.
> `propagate_at_launch` is the only bridge between the two namespaces: it
> copies a group tag onto each instance the group launches *from then on*,
> leaving instances already running untouched. To tag the instances
> themselves — including the ones already there — reach for
> `aws.ec2.create_tags` instead.

The group is the **entry-function argument**, one call per group, as it is
for every other call in this module. Upstream models each tag as a
`{ResourceId, ResourceType, Key, Value, PropagateAtLaunch}` record and
takes a list of them, so one request could in principle span groups; naming
the group once keeps `ResourceId` from being repeated on every tag, and
`ResourceType` is always `auto-scaling-group`, so we set it rather than
asking.

### `create_or_update_tags`

Set tags on an Auto Scaling group, creating each key or overwriting its
value.

**Signature:** `aws.autoscaling.create_or_update_tags(<group>) -> builder`

**Parameters:**

- `group` (string) — the Auto Scaling group name.

**Builder methods:**

- `:tags(<map>)` — the tag set, a `{[key] = value}` map (`{ Env = "prod" }`).
  Keys and values must both be strings. **Required**: neither "nothing" nor
  "everything" is a good guess at what an absent tag set meant. A list of
  keys is refused here — a write has no value to set. Called more than once,
  the last call wins.
- `:propagate_at_launch([<bool>])` — copy these tags onto the instances the
  group launches from now on. Bare is `true`; never called is `false`;
  `:propagate_at_launch(false)` is accepted and states the default, which
  reads better next to a conditional. It is **builder-wide**, applying to
  every tag in the set. Upstream puts the flag on each tag, but a set of
  tags that disagree about propagation is not the case that occurs, and
  spelling it per tag would need a nested table that fights the
  `{[key] = value}` map — so a set genuinely needing both must be split
  into two calls.

**Returns:** `(envelope, nil)` — the API returns nothing; the envelope
alone is the answer.

**Errors:** raises when

- `:tags()` was never called — `create_or_update_tags: set the tags with
  ':tags()'`;
- `:tags()` was given an empty table — `create_or_update_tags: ':tags()'
  was given an empty table`;
- `:tags()` was given a list of keys — `create_or_update_tags: expected a
  {[key] = value} map of tags, not a list of keys`;
- `:tags()` was given a non-table, a non-string key or a non-string value
  (the message names the action and the offending type);
- `<group>` is an empty string;
- the AWS API fails (message prefixed `create_or_update_tags:`).

The shape checks raise at the `:tags()` call; the rest raise from
`:send()`.

```lua
aws.autoscaling.create_or_update_tags("web-prod")
    :tags({ Env = "prod", Owner = "platform" })
    :propagate_at_launch()
    :send()
```

### `delete_tags`

Remove tags from an Auto Scaling group.

**Signature:** `aws.autoscaling.delete_tags(<group>) -> builder`

**Parameters:**

- `group` (string) — the Auto Scaling group name.

**Builder methods:**

- `:tags(<keys-or-map>)` — **either** an array of key strings
  (`{ "Env", "Name" }`), which deletes each key whatever value it holds, or
  a `{[key] = value}` map, which deletes only on a value match — so a
  group table's own `tags` can be handed straight back. A table carrying
  **both** a sequence part and string keys means both: the sequence entries
  become key-only deletions and the pairs value-matched ones, neither half
  silently losing to the other. **Required**, for a sharper reason than on
  a write: upstream reads an absent tag set on a delete as *every tag on
  the group*. Called more than once, the last call wins.
- `:propagate_at_launch()` — **not available here.** Calling it raises
  `delete_tags: ':propagate_at_launch()' applies to
  'create_or_update_tags'; a delete removes the tag either way`.

**Returns:** `(envelope, nil)` — the API returns nothing.

**Errors:** raises when

- `:tags()` was never called — `delete_tags: set the tags with ':tags()'`;
- `:tags()` was given an empty table — `delete_tags: ':tags()' was given an
  empty table`;
- `:tags()` was given a non-table (`delete_tags: expected a table of tags,
  got <type>`), a non-string key or a non-string value;
- `:propagate_at_launch()` was called;
- `<group>` is an empty string;
- the AWS API fails (message prefixed `delete_tags:`).

```lua
aws.autoscaling.delete_tags("web-prod"):tags({ "Env", "Owner" }):send()
```
