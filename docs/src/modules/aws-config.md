# `aws.config` — explicit AWS clients

| Function | Effect |
| --- | --- |
| `aws.config()` | start a configuration builder |
| `:region(<name>)` | set the region to call |
| `:profile(<name>)` | use a named profile from `~/.aws/config` (authoritative) |
| `:endpoint_url(<url>)` | call another endpoint entirely |
| `:path_style([<bool>])` | address S3 buckets in the path (MinIO, LocalStack) |
| `:timeout(<seconds>)` | set the per-operation timeout; fractional allowed |
| `:retries(<n>)` | set total attempts, `1` disables retrying |
| `:build()` | resolve the configuration, returning a client (`.ec2`/`.s3`/`.autoscaling`/`.cloudtrail`/`.cloudwatch`) |

## Example

```lua
local eu = aws.config():region("eu-central-1"):profile("prod"):build()

local r, listing = eu.s3.list_objects("logs"):send()
local r, instances = eu.ec2.describe_instances():name("web-*"):send()
```

`eu.s3`, `eu.ec2`, `eu.autoscaling`, `eu.cloudtrail` and `eu.cloudwatch` are
the *same* functions, with the same builders and the same `:send()`, as
[`aws.s3`](aws-s3.md), [`aws.ec2`](aws-ec2.md),
[`aws.autoscaling`](aws-autoscaling.md),
[`aws.cloudtrail`](aws-cloudtrail.md) and
[`aws.cloudwatch`](aws-cloudwatch.md). Only the configuration behind them
differs — everything the per-service chapters document applies unchanged.
Every `:send()` returns `(envelope, data)`, so bind both values.

## Conventions

The `aws.<service>` tables read the *ambient* configuration — whatever
`AWS_PROFILE`, `AWS_REGION`, `~/.aws/config`, SSO or instance metadata
resolve to — built lazily on first use and then cached, so a script that never
calls AWS pays nothing for it. That is right for the common case and wrong the
moment one script has to reach two regions or two accounts at once.

`aws.config()` is the escape hatch: a builder for the configuration itself,
finalized by `:build()` into a client carrying the same per-service keys
(`.ec2`, `.s3`, `.autoscaling`, `.cloudtrail`, `.cloudwatch`) as the `aws`
global.

Every knob on the builder is optional, and every knob left unset falls through
to the same default provider chain the ambient surface uses — so
`aws.config():region("eu-central-1"):build()` keeps the machine's profile and
credentials, changing only the region:

```lua
local frankfurt = aws.config():region("eu-central-1"):build()
```

**Setters copy.** Every setter returns a *new* builder and leaves the receiver
untouched, so one base configuration can be refined several ways without the
variants interfering:

```lua
local base = aws.config():retries(5)
local eu, us = base:region("eu-central-1"):build(), base:region("us-east-1"):build()
```

**`:profile()` is authoritative.** Credentials **and** region come from that
profile alone, so `AWS_ACCESS_KEY_ID`/`AWS_REGION` in the environment cannot
send the script to another account. `:region()` still overrides the profile's
region, the same precedence the AWS CLI uses.

**`:build()` with a profile resolves its credentials** — an async call, so it
yields rather than blocking, and several builds can overlap under
[`task`](task.md). An unusable profile therefore raises **at the `:build()`
call**, not at some later `:send()`. Without `:profile()` nothing is resolved
eagerly (the ambient chain is consulted on first use, as for the `aws`
globals), which keeps credential-less setups such as LocalStack working.

## `aws.config`

Start a new, empty configuration builder.

**Signature:** `aws.config() -> Config`

**Returns:** a `Config` builder with no knobs set.

**Methods:** `region` · `profile` · `endpoint_url` · `path_style` · `timeout` ·
`retries` · `build`

**Errors:** none — construction is local and cannot fail.

```lua
local builder = aws.config()
```

## `Config:region`

Set the region every service call under this client uses.

**Signature:** `Config:region(name: string) -> Config`

**Parameters:**
- `name` — a region string, e.g. `"eu-central-1"`. Not validated against the
  list of regions; an unknown region surfaces as a failure from the eventual
  AWS call.

**Returns:** a new `Config`.

**Errors:** raises `aws.config:region: …` on a non-string or empty `name`.

```lua
local c = aws.config():region("ap-south-1")
```

## `Config:profile`

Use a named profile from `~/.aws/config`/`~/.aws/credentials` for credentials
and, unless `:region()` overrides it, region. Environment variables do not take
precedence over it.

**Signature:** `Config:profile(name: string) -> Config`

**Parameters:**
- `name` — the profile name as it appears in the AWS config file.

**Returns:** a new `Config`.

**Errors:** raises `aws.config:profile: …` on a non-string or empty `name`. An
unknown or misconfigured profile raises later, at `:build()`.

```lua
local c = aws.config():profile("prod")
```

## `Config:endpoint_url`

Point every service reached through this client at another endpoint —
LocalStack, MinIO, a VPC endpoint — instead of the real AWS endpoint.

**Signature:** `Config:endpoint_url(url: string) -> Config`

**Parameters:**
- `url` — a full URL, e.g. `"http://localhost:4566"`.

**Returns:** a new `Config`.

**Errors:** raises on a non-string or empty `url`. An unreachable endpoint
fails the eventual AWS call, not `:build()`. A region is still required even
when talking to a local endpoint — the SDK signs requests with one regardless
of what the endpoint does with it. For S3 against MinIO/LocalStack also set
[`:path_style()`](#configpath_style).

```lua
local local_s3 = aws.config()
    :region("us-east-1")
    :endpoint_url("http://localhost:4566")
    :path_style()
    :build()

local r, buckets = local_s3.s3.list_buckets():send()
```

## `Config:path_style`

Address S3 buckets in the URL path (`host/bucket/key`) instead of the host name
(`bucket.host/key`). Custom endpoints such as MinIO and LocalStack need this;
`:endpoint_url()` alone does not imply it, and bucket operations fail without
it. Affects `.s3` only.

**Signature:** `Config:path_style(enabled: boolean?) -> Config`

**Parameters:**
- `enabled` — defaults to `true`.

**Returns:** a new `Config`.

**Errors:** raises on a non-boolean `enabled`.

## `Config:timeout`

Set the per-operation timeout — the whole call, including its retries, not each
individual attempt.

**Signature:** `Config:timeout(seconds: number) -> Config`

**Parameters:**
- `seconds` — a positive, finite number of seconds; fractional values allowed
  (e.g. `1.5`).

**Returns:** a new `Config`.

**Errors:** raises on `0`, a negative, `NaN`/`inf` or an absurdly large value:

```
aws.config:timeout: expected a positive number of seconds, got 0
```

```lua
local c = aws.config():timeout(1.5)
```

## `Config:retries`

Set the total number of attempts for each operation. This replaces the
profile's retry mode with the SDK's standard one.

**Signature:** `Config:retries(n: integer) -> Config`

**Parameters:**
- `n` — total attempts, counting the first try — matching the SDK's own
  counting. `1` disables retrying; `5` means up to four retries after the first
  failure.

**Returns:** a new `Config`.

**Errors:** raises `aws.config:retries: …` on anything but an integer of at
least `1` (zero attempts would mean "never call AWS"):

```
aws.config:retries: expected an integer of at least 1 (the count includes the first attempt), got 0
```

```lua
local c = aws.config():retries(5)
```

## `Config:build`

Resolve the configuration — credentials (under `:profile()`), region,
endpoint, timeout, retries — and return a client exposing `.ec2`, `.s3`,
`.autoscaling`, `.cloudtrail` and `.cloudwatch`.

**Signature:** `Config:build() -> Client` (async)

**Parameters:** none; consumes the knobs accumulated by the builder calls
above.

**Returns:** a `Client` with five fields, each the same table as the matching
`aws.<service>` global, over this configuration.

**Errors:** with `:profile()`, raises `aws.config:build: profile '<name>': …`
if the profile's credentials cannot be resolved — an unknown profile, an SSO
session that needs re-login, a broken `credential_process`. Without a profile
nothing is resolved here; failures surface at the first `:send()`.

```lua
local prod = aws.config():profile("prod"):build()

local r, listing = prod.s3.list_objects("logs"):send()
```

## Spanning regions or accounts

The reason the module exists. Each built client is independent, so several can
be live — and, under [`task`](task.md), resolving — at once:

```lua
local regions = { "eu-central-1", "us-east-1", "ap-south-1" }
local handles = {}

for _, region in ipairs(regions) do
    table.insert(handles, task.spawn(function()
        local aws_region = aws.config():region(region):build()
        local r, instances = aws_region.ec2.describe_instances()
            :instance_state("running")
            :send()
        return instances
    end))
end

for i, instances in ipairs({ task.await(handles) }) do
    print(regions[i], #instances)
end
```

Two accounts is the same shape with `:profile()` instead of `:region()`.
