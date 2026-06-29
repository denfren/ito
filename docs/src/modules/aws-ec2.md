# `aws.ec2` — AWS EC2

| Function/method | Effect |
| --- | --- |
| `aws.ec2.describe_instances()` | new `DescribeInstances` builder |
| `aws.ec2.describe_images()` | new `DescribeImages` builder |
| `aws.ec2.describe_snapshots()` | new `DescribeSnapshots` builder |
| `aws.ec2.start_instances(ids)` | new `start_instances` action builder |
| `aws.ec2.stop_instances(ids)` | new `stop_instances` action builder |
| `aws.ec2.reboot_instances(ids)` | new `reboot_instances` action builder |
| `aws.ec2.terminate_instances(ids)` | new `terminate_instances` action builder |
| `aws.ec2.create_tags(resources)` | new `create_tags` action builder |
| `aws.ec2.delete_tags(resources)` | new `delete_tags` action builder |
| `:id`/`:ids` | filter/select by instance, AMI or snapshot id |
| `:name` | filter by `tag:Name` (instances/images: `name` for images) |
| `:tag`/`:tag_key` | filter by tag value / tag presence |
| `:instance_state`/`:instance_type`/`:vpc_id`/`:subnet_id` | typed instance filters |
| `:owner` | filter images/snapshots by owner |
| `:state`/`:status` | filter images by state, snapshots by status |
| `:architecture` | filter images by architecture |
| `:volume_id` | filter snapshots by source volume |
| `:include_deprecated` | include deprecated AMIs (images) |
| `:filter` | escape hatch: any EC2 filter by name |
| `:dry_run` | validate an action's permissions, run nothing |
| `:force`/`:hibernate` | `stop_instances`-only refinements |
| `:tags` | the tag set a tag action writes or deletes |
| `:send` | finalize any builder, run the request |

## Example

```lua
local r, instances = aws.ec2.describe_instances()
    :instance_state("running")
    :tag("Environment", "prod")
    :send()

for _, i in ipairs(instances) do
    print(i.instance_id, i.private_ip_address, i.tags.Name)
end
```

## Conventions

**Builder + `:send()`.** Every entry point (`describe_instances`,
`describe_images`, `describe_snapshots`, the four lifecycle actions and
the two tag actions) returns a mutable builder. Filter/option methods return the same builder
so calls chain; `:send()` finalizes it and performs the request.

**Two return values.** As documented for the whole `aws` surface,
`:send()` either raises or returns exactly two values: an envelope table
always carrying `ok`, and the response as native Luau data.

```lua
local r, instances = aws.ec2.describe_instances():send()
local r, states    = aws.ec2.stop_instances({ "i-abc" }):send()
```

For a describe, `ok` is always `true` — a query that matched nothing is
an empty table, not a failure — so the envelope earns its keep on the
actions instead, where [`:dry_run()`](#dry_run) makes "allowed,
and nothing happened" a real answer rather than an error.
`create_tags`/`delete_tags` carry no response data at all, so their second
value is always `nil`; the pair is kept anyway, so one `:send()` convention
holds for the whole `aws` surface.

**Value arguments accept a string or a list.** Every filter value and the
lifecycle actions' instance-id argument take either a single string or a
list of strings; a list means *any of*:

```lua
:instance_state({ "running", "stopped" })
:tag("Environment", { "prod", "staging" })
aws.ec2.stop_instances({ "i-abc", "i-def" })
```

Passing anything else (a number, a table with non-string entries) raises
`"expected string or array of strings, got <type>"`.

**Wildcards.** Filter values support the `*` and `?` wildcards EC2 itself
supports (`:name("web-*")`); there is no separate ends-with helper
because EC2 has no such filter.

**Pagination is followed to exhaustion.** All three describes drain their
paginator fully before returning; there is no page token to manage and no
result cap to trip over.

**Default vs explicit client.** `aws.ec2` is backed by the *ambient*
configuration (`AWS_PROFILE`/`AWS_REGION`/`~/.aws/config`/SSO/IMDS),
resolved lazily on first use and cached — a script that never calls into
EC2 pays nothing. To reach more than one region or account from a single
script, build an explicit client with [`aws.config`](aws-config.md); it
carries an `ec2` key with exactly the surface documented here:

```lua
local eu = aws.config():region("eu-central-1"):build()
local r, instances = eu.ec2.describe_instances():instance_state("running"):send()
```

**Errors.** Failures that are bugs rather than answers — bad or expired
credentials, denied access, connectivity, a malformed request — raise a
Luau error. `DescribeInstances`/`DescribeImages`/`DescribeSnapshots`
never raise on "matched nothing"; they return an empty table. The
lifecycle actions raise if `ids` is empty, and the tag actions if
`resources` is empty. All of them raise `UnauthorizedOperation`
normally, but treat the `DryRunOperation` refusal under `:dry_run()` as
success (see below).

**Concurrency.** `:send()` is async and yields rather than blocking, so
several calls under [`task`](task.md) overlap instead of each pinning the
runtime for a full round of pagination:

```lua
local handles = {}
for _, state in ipairs({ "running", "stopped" }) do
    table.insert(handles, task.spawn(function()
        local r, instances = aws.ec2.describe_instances():instance_state(state):send()
        return instances
    end))
end
local running, stopped = task.await(handles)
```

Concurrent first calls into the ambient client share one lazily-built
client rather than each building their own.

## `describe_instances`

Builds a `DescribeInstances` request over instance ids and/or filters.

### Signature

```
aws.ec2.describe_instances() -> DescribeInstances
```

### Parameters

None. Configure the request through the builder's methods, below.

### Return value

A `DescribeInstances` builder.

### Builder methods

| Method | Signature | EC2 filter |
| --- | --- | --- |
| `:id` | `(id)` | one instance id |
| `:ids` | `(list)` | several instance ids |
| `:name` | `(value)` | `tag:Name` |
| `:tag` | `(key, value)` | `tag:<key>` |
| `:tag_key` | `(key)` | `tag-key` |
| `:instance_state` | `(value)` | `instance-state-name` |
| `:instance_type` | `(value)` | `instance-type` |
| `:vpc_id` | `(value)` | `vpc-id` |
| `:subnet_id` | `(value)` | `subnet-id` |
| `:filter` | `(name, value)` | any filter, by name |

`value`/`list` parameters accept a single string or list of strings
(see [Conventions](#conventions)). `:name` is sugar for
`:tag("Name", ...)`, since `Name` is a tag rather than an instance
attribute. `:filter` is the escape hatch for any EC2 filter without a
typed helper above.

### `:send()`

#### Return value

`(envelope, instances)` — `envelope.ok` is always `true`; `instances` is
an array of instance tables, possibly empty.

| Field | Type |
| --- | --- |
| `instance_id` | string |
| `image_id` | string |
| `instance_type` | string |
| `architecture` | string |
| `private_ip_address` | string |
| `public_ip_address` | string |
| `vpc_id` | string |
| `subnet_id` | string |
| `availability_zone` | string |
| `state` | string |
| `state_code` | number |
| `state_transition_reason` | string |
| `launch_time` | string (RFC 3339) |
| `tags` | table |

Keys are snake_case, mirroring the SDK. `state` is flattened to its name
(`running`, `stopped`, …), the field scripts actually want, with the
numeric code kept alongside as `state_code`. `launch_time` is RFC 3339,
keeping it both comparable and printable in Luau, which has no native
date type. Absent scalars are omitted rather than set to `nil`. `tags` is
always present — a `{[key] = value}` map, possibly empty — so
`i.tags.Name` is safe to index without a guard.

#### Error conditions

Raises on any SDK/transport error (bad credentials, denied access,
connectivity, a malformed filter). Never raises on "no matches" — that is
an empty `instances` table.

#### Example

```lua
local r, instances = aws.ec2.describe_instances()
    :vpc_id("vpc-0123")
    :instance_state({ "running", "stopped" })
    :send()

for _, i in ipairs(instances) do
    print(i.instance_id, i.state, i.private_ip_address)
end
```

## `describe_images`

Builds a `DescribeImages` (AMI) request over image ids, owners and/or
filters.

### Signature

```
aws.ec2.describe_images() -> DescribeImages
```

### Parameters

None. Configure the request through the builder's methods, below.

### Return value

A `DescribeImages` builder.

### Builder methods

| Method | Signature | Meaning |
| --- | --- | --- |
| `:id` | `(id)` | one AMI id |
| `:ids` | `(list)` | several AMI ids |
| `:owner` | `(value)` | account id, or `self`/`amazon`/`aws-marketplace` |
| `:name` | `(value)` | the `name` filter |
| `:tag` | `(key, value)` | `tag:<key>` |
| `:tag_key` | `(key)` | `tag-key` |
| `:state` | `(value)` | `available`, `pending`, `failed`, … |
| `:architecture` | `(value)` | `x86_64`, `arm64`, … |
| `:include_deprecated` | `()` | include deprecated AMIs |
| `:filter` | `(name, value)` | any filter, by name |

Pass an owner. Without one the result is every public AMI in the region,
which is never what a script wants. Deprecated AMIs are hidden unless
`:include_deprecated()` is called, matching the API's own default.

### `:send()`

#### Return value

`(envelope, images)` — `envelope.ok` is always `true`; `images` is an
array of image tables, possibly empty.

| Field | Type |
| --- | --- |
| `image_id` | string |
| `name` | string |
| `description` | string |
| `owner_id` | string |
| `owner_alias` | string |
| `state` | string |
| `architecture` | string |
| `root_device_type` | string |
| `virtualization_type` | string |
| `public` | boolean |
| `creation_date` | string (RFC 3339) |
| `deprecation_time` | string (RFC 3339) |
| `tags` | table |

`creation_date` arrives as an RFC 3339 string on the wire already
(unlike every other EC2 timestamp, which is converted from a typed
`DateTime`). `tags` is always present, possibly empty.

#### Error conditions

Raises on any SDK/transport error. Never raises on "no matches".

#### Example

```lua
local r, images = aws.ec2.describe_images()
    :owner("self")
    :state("available")
    :send()

for _, img in ipairs(images) do
    print(img.image_id, img.name, img.creation_date)
end
```

## `describe_snapshots`

Builds a `DescribeSnapshots` (EBS snapshot) request over snapshot ids,
owners and/or filters.

### Signature

```
aws.ec2.describe_snapshots() -> DescribeSnapshots
```

### Parameters

None. Configure the request through the builder's methods, below.

### Return value

A `DescribeSnapshots` builder.

### Builder methods

| Method | Signature | Meaning |
| --- | --- | --- |
| `:id` | `(id)` | one snapshot id |
| `:ids` | `(list)` | several snapshot ids |
| `:owner` | `(value)` | account id, or `self`/`amazon` |
| `:volume_id` | `(value)` | `volume-id` |
| `:status` | `(value)` | `pending`, `completed`, `error` |
| `:tag` | `(key, value)` | `tag:<key>` |
| `:tag_key` | `(key)` | `tag-key` |
| `:filter` | `(name, value)` | any filter, by name |

As for images, pass an owner — otherwise the result is every public
snapshot in the region.

### `:send()`

#### Return value

`(envelope, snapshots)` — `envelope.ok` is always `true`; `snapshots` is
an array of snapshot tables, possibly empty.

| Field | Type |
| --- | --- |
| `snapshot_id` | string |
| `volume_id` | string |
| `description` | string |
| `owner_id` | string |
| `state` | string |
| `state_message` | string |
| `progress` | string |
| `volume_size` | number |
| `encrypted` | boolean |
| `storage_tier` | string |
| `start_time` | string (RFC 3339) |
| `tags` | table |

#### Error conditions

Raises on any SDK/transport error. Never raises on "no matches".

#### Example

```lua
local r, snaps = aws.ec2.describe_snapshots()
    :owner("self")
    :volume_id("vol-abc")
    :send()

for _, s in ipairs(snaps) do
    print(s.snapshot_id, s.state, s.progress)
end
```

## Lifecycle actions: `start_instances`/`stop_instances`/`reboot_instances`/`terminate_instances`

Each starts an action builder over a required set of instance ids, and is
the only part of this module that changes anything.

### Signature

```
aws.ec2.start_instances(ids) -> LifecycleAction
aws.ec2.stop_instances(ids) -> LifecycleAction
aws.ec2.reboot_instances(ids) -> LifecycleAction
aws.ec2.terminate_instances(ids) -> LifecycleAction
```

### Parameters

- `ids` (string or list of strings, required) — the instance id(s) to
  act on. Required at the entry function rather than a builder method:
  there is no useful call without them.

### Return value

A `LifecycleAction` builder, one type shared by all four calls.

### Builder methods

| Method | Applies to | Meaning |
| --- | --- | --- |
| `:dry_run()` | all four | validate permissions, change nothing |
| `:force()` | `stop_instances` only | stop without a graceful shutdown |
| `:hibernate()` | `stop_instances` only | hibernate rather than stop |

Calling `:force()` or `:hibernate()` on any action other than
`stop_instances` raises immediately (not deferred to `:send()`): a script
bug is caught here rather than being silently dropped on the way to EC2.

### `:send()`

#### Return value

`(envelope, changes)`. `envelope.ok` is `true` unless the call raises.
`changes` is an array of state-change tables — **except** under
`:dry_run()` success, where it is `nil` (see [Dry run](#dry-run)), and
**except** for `reboot_instances`, where it is always an empty table:
EC2 reports no state transition for a reboot, only that the request was
accepted.

State-change table fields:

| Field | Type |
| --- | --- |
| `instance_id` | string |
| `current_state` | string |
| `current_state_code` | number |
| `previous_state` | string |
| `previous_state_code` | number |

Both states are flattened to their names, with the numeric codes kept
alongside — the same treatment `describe_instances` gives `state`.

#### Error conditions

- Raises `"<action>: expected at least one instance id"` if `ids` is
  empty (checked before the request is sent — EC2's own rejection message
  is less helpful).
- Raises `"<action>: ':force()' applies to stop_instances only"` (or the
  equivalent for `:hibernate()`) if that method was called on a
  non-`stop_instances` action.
- Raises on any other SDK/transport error, including
  `UnauthorizedOperation` (the caller lacks the permission being tested).
- Does **not** raise on `DryRunOperation` under `:dry_run()` — that is
  the success case.

#### Example

```lua
local r, states = aws.ec2.stop_instances({ "i-abc", "i-def" }):send()

for _, s in ipairs(states) do
    print(s.instance_id, s.previous_state, "->", s.current_state)
end
```

### Dry run

`:dry_run()` maps onto EC2's own `DryRun` parameter: the request is
validated for permissions and then refused by the service, so nothing
can run by accident on our side of the call.

```lua
local r = aws.ec2.terminate_instances({ "i-abc" }):dry_run():send()
if r.ok then
    print("permitted -- and nothing happened")
end
```

EC2 reports that refusal as the `DryRunOperation` error, which this
module treats as success: `ok` is `true` and the second return value is
`nil`. `UnauthorizedOperation` — the same exchange for a caller who lacks
the permission — still raises, since that one is a real answer rather
than a validation artifact.

## `create_tags`

Sets tags on any EC2 resources, by id.

### Signature

```
aws.ec2.create_tags(resources) -> TagRequest
```

### Parameters

- `resources` (string or list of strings, required) — the resource id(s)
  to tag. Required at the entry function rather than a builder method:
  there is no useful call without them. One generic call covers
  instances, AMIs, snapshots, volumes and the rest, because EC2's tag API
  is keyed by resource *id*, not by resource type — a per-resource-type
  function would invent a distinction the service does not make.

### Return value

A `TagRequest` builder, one type shared with `delete_tags`.

### Builder methods

| Method | Signature | Meaning |
| --- | --- | --- |
| `:tags` | `(table)` | the tags to set, as a `{[<key>] = <value>}` map |
| `:dry_run` | `()` | validate permissions, change nothing |

`:tags()` takes the same `{[<key>] = <value>}` map that
`.tags` reads back on every describe result, so a read/modify/write
round-trip needs no reshaping. Tag values may be given as strings,
numbers or booleans; a number or boolean is stringified. Calling
`:tags()` again replaces the previous set rather than merging into it.

A key without a value has no meaning on create — there is nothing to set
— so an array entry (`{ "Env" }`) is refused at the declaring call rather
than by EC2.

### `:send()`

#### Return value

`(envelope, nil)`. `envelope.ok` is `true` unless the call raises;
`CreateTags` returns no data, so the envelope is the whole answer. The
second value is present, and `nil`, to keep the module's two-value
convention.

#### Error conditions

- Raises `"create_tags: expected a table of tags, got <type>"` from
  `:tags()` if the argument is not a table.
- Raises `"create_tags: tags must be a {[key] = value} map, got a key
  without a value (<key>)"` from `:tags()` for an array (sequence) entry.
- Raises `"create_tags: tag keys must be strings, got <type>"` and
  `"create_tags: tag value for <key> must be a string, got <type>"` from
  `:tags()` for a non-string key, or a value that is not a string, number
  or boolean.
- Raises `"create_tags: expected at least one resource id"` from
  `:send()` if `resources` is empty (checked before the request goes out
  — EC2's own rejection message is less helpful).
- Raises `"create_tags: expected at least one tag"` from `:send()` if
  `:tags()` was never called, or was called with an empty table.
- Raises on any other SDK/transport error, including
  `UnauthorizedOperation`.
- Does **not** raise on `DryRunOperation` under `:dry_run()` — that is the
  success case, exactly as for the lifecycle actions: EC2 validates the
  permissions and then refuses, so `ok` is `true` and nothing happened.

Nothing here returns `nil` to signal a failure; every failure raises.

#### Example

```lua
local r, instances = aws.ec2.describe_instances():instance_state("running"):send()

for _, i in ipairs(instances) do
    if not i.tags.Owner then
        aws.ec2.create_tags(i.instance_id)
            :tags({ Owner = "platform", Reviewed = "2026-08-13" })
            :send()
    end
end
```

## `delete_tags`

Removes tags from any EC2 resources, by id.

### Signature

```
aws.ec2.delete_tags(resources) -> TagRequest
```

### Parameters

- `resources` (string or list of strings, required) — the resource id(s)
  to untag. As for `create_tags`, one generic call covers every resource
  kind, EC2's tag API being keyed by id rather than type.

### Return value

A `TagRequest` builder, one type shared with `create_tags`.

### Builder methods

| Method | Signature | Meaning |
| --- | --- | --- |
| `:tags` | `(table)` | the tags to delete — a map, an array of keys, or both |
| `:dry_run` | `()` | validate permissions, change nothing |

`:tags()` accepts **either** shape, mirroring upstream, where omitting a
tag's value deletes whatever value it has:

- a `{[<key>] = <value>}` map deletes only where the value matches;
- an array of key strings (`{ "Env", "Name" }`) deletes the key
  regardless of its value.

A table carrying **both** a sequence part and string keys means both: the
sequence entries are keys, the remaining string keys are key/value pairs.

```lua
aws.ec2.delete_tags("i-abc"):tags({ "Scratch", Env = "staging" }):send()
```

`:tags()` is **required**. Upstream reads an absent tag set as "delete
every tag on these resources" — a silent fleet-wide tag loss one
forgotten call away — so it has to be asked for by naming the keys, and
there is deliberately no shorthand for it. An *explicitly* empty table is
accepted, unlike on create: a caller who passed `{}` has said what they
meant, and the call deletes nothing.

### `:send()`

#### Return value

`(envelope, nil)`. `envelope.ok` is `true` unless the call raises;
`DeleteTags` returns no data, so the envelope is the whole answer, with
the `nil` keeping the module's two-value convention.

#### Error conditions

- Raises `"delete_tags: expected a table of tags, got <type>"` from
  `:tags()` if the argument is not a table.
- Raises `"delete_tags: tag keys must be strings, got <type>"` and
  `"delete_tags: tag value for <key> must be a string, got <type>"` from
  `:tags()` for a non-string key, or a value that is not a string, number
  or boolean. A key without a value is *not* an error here — that is the
  array form.
- Raises `"delete_tags: expected at least one resource id"` from
  `:send()` if `resources` is empty.
- Raises `"delete_tags: expected ':tags()' — pass the keys to delete;
  there is no shorthand for deleting every tag"` from `:send()` if
  `:tags()` was never called.
- Raises on any other SDK/transport error, including
  `UnauthorizedOperation`.
- Does **not** raise on `DryRunOperation` under `:dry_run()` — that is the
  success case: EC2 validates the permissions and then refuses, so `ok` is
  `true` and nothing happened.

Nothing here returns `nil` to signal a failure; every failure raises.

#### Example

```lua
local r = aws.ec2.delete_tags({ "i-abc", "vol-def" })
    :tags({ "Scratch" })
    :dry_run()
    :send()

if r.ok then
    print("permitted -- and nothing happened")
end
```
