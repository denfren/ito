# `aws.config` — explicit AWS clients

| Function/Method | Effect |
| --- | --- |
| `aws.config()` | start a configuration builder |
| `:region(<name>)` | set the region to call |
| `:profile(<name>)` | use a named profile from `~/.aws/config` |
| `:endpoint_url(<url>)` | call another endpoint entirely |
| `:timeout(<seconds>)` | set the per-operation timeout; fractional allowed |
| `:retries(<n>)` | set total attempts, `1` disables retrying |
| `:build()` | resolve credentials/config, returning a client (`.ec2`/`.s3`/`.autoscaling`) |

## Example

```lua
local eu = aws.config():region("eu-central-1"):profile("prod"):build()

local r, listing = eu.s3.list_objects("logs"):send()
local instances  = eu.ec2.describe_instances():name("web-*"):send()
```

`eu.s3`, `eu.ec2` and `eu.autoscaling` are the *same* functions, with the
same builders and the same `:send()`, as [`aws.s3`](aws-s3.md),
[`aws.ec2`](aws-ec2.md) and [`aws.autoscaling`](aws-autoscaling.md).
Only the configuration behind them differs — everything the per-service
chapters document applies unchanged.

## Why explicit clients exist

`aws.ec2`, `aws.s3` and `aws.autoscaling` read the *ambient*
configuration — whatever `AWS_PROFILE`, `AWS_REGION`, `~/.aws/config`,
SSO or instance metadata resolve to — built lazily on first use and then
cached, so a script that never calls AWS pays nothing for it. That is
right for the common case and wrong the moment one script has to reach
two regions or two accounts at once.

`aws.config()` is the escape hatch: a builder for the configuration
itself, finalized by `:build()` into a client carrying the same
per-service keys (`.ec2`, `.s3`, `.autoscaling`) as the `aws` global.

Every knob on the builder is optional, and every knob left unset falls
through to the same default provider chain the ambient surface uses — so
`aws.config():region("eu-central-1"):build()` keeps the machine's profile
and credentials, changing only the region:

```lua
local frankfurt = aws.config():region("eu-central-1"):build()
```

`:profile()` brings the profile's own region with it; `:region()`
overrides it if both are set — the same precedence the AWS CLI uses.

`:build()` is where credentials resolve — an async call, so it yields
rather than blocking, and several builds can overlap under
[`task`](task.md). This is the one deliberate difference from
`aws.ec2`/`aws.s3`/`aws.autoscaling`, which defer that same work to first
use so a script that never touches AWS stays free. An unusable profile
therefore raises **at the `:build()` call**, not at some later
`:send()`. A built client is worth reusing across many calls, since
`:build()` is the only step that costs a credential round trip.

## `aws.config()`

Start a new, empty configuration builder.

**Signature:** `aws.config() -> Config`

**Parameters:** none.

**Returns:** a `Config` builder userdata with no knobs set.

**Errors:** none — construction is local and cannot fail.

```lua
local builder = aws.config()
```

## `Config:region(name)`

Set the region every service call under this client uses.

**Signature:** `Config:region(name: string) -> Config`

**Parameters:**
- `name` — a region string, e.g. `"eu-central-1"`. Not validated here;
  an unknown region surfaces as a failure from the eventual AWS call.

**Returns:** the same `Config`, so calls chain.

**Errors:** none from this call itself.

```lua
local c = aws.config():region("ap-south-1")
```

## `Config:profile(name)`

Use a named profile from `~/.aws/config`/`~/.aws/credentials` for
credentials (and, unless overridden, region).

**Signature:** `Config:profile(name: string) -> Config`

**Parameters:**
- `name` — the profile name as it appears in the AWS config file.

**Returns:** the same `Config`, so calls chain.

**Errors:** none from this call; an unknown or misconfigured profile
raises later, at `:build()`.

```lua
local c = aws.config():profile("prod")
```

## `Config:endpoint_url(url)`

Point every service reached through this client at another endpoint —
localstack, minio, a VPC endpoint — instead of the real AWS endpoint.

**Signature:** `Config:endpoint_url(url: string) -> Config`

**Parameters:**
- `url` — a full URL, e.g. `"http://localhost:4566"`.

**Returns:** the same `Config`, so calls chain.

**Errors:** none from this call itself; an unreachable endpoint fails
the eventual AWS call, not `:build()`. A region is still required even
when talking to a local endpoint — the SDK signs requests with one
regardless of what the endpoint does with it.

```lua
local local_s3 = aws.config()
    :region("us-east-1")
    :endpoint_url("http://localhost:4566")
    :build()

local r, buckets = local_s3.s3.list_buckets():send()
```

## `Config:timeout(seconds)`

Set the per-operation timeout — the whole call, including its retries,
not each individual attempt.

**Signature:** `Config:timeout(seconds: number) -> Config`

**Parameters:**
- `seconds` — a positive number of seconds; fractional values allowed
  (e.g. `1.5`).

**Returns:** the same `Config`, so calls chain.

**Errors:** raises if `seconds` is not finite and greater than `0`
(`0`, negative, `NaN`/`inf` all raise) — a script mistake, not a
configuration:

```
timeout must be a positive number of seconds, got 0
```

```lua
local c = aws.config():timeout(1.5)
```

## `Config:retries(n)`

Set the total number of attempts for each operation.

**Signature:** `Config:retries(n: integer) -> Config`

**Parameters:**
- `n` — total attempts, counting the first try — matching the SDK's own
  counting. `1` disables retrying; `5` means up to four retries after the
  first failure.

**Returns:** the same `Config`, so calls chain.

**Errors:** raises on `0` — zero attempts would mean "never call AWS",
which is never what was meant:

```
retries must be at least 1 (the count includes the first attempt)
```

```lua
local c = aws.config():retries(5)
```

## `Config:build()`

Resolve the configuration — credentials, region, endpoint, timeout,
retries — and return a client exposing `.ec2`, `.s3` and `.autoscaling`.

**Signature:** `Config:build() -> Client` (async)

**Parameters:** none; consumes the knobs accumulated by the builder
calls above.

**Returns:** a `Client` with three fields:
- `.ec2` — the same table [`aws.ec2`](aws-ec2.md) exposes, over this
  configuration
- `.s3` — the same table [`aws.s3`](aws-s3.md) exposes, over this
  configuration
- `.autoscaling` — the same table [`aws.autoscaling`](aws-autoscaling.md)
  exposes, over this configuration

**Errors:** raises if credential resolution fails — an unknown profile,
an SSO session that needs re-login, missing IMDS/environment
credentials, or any other failure the AWS SDK's default provider chain
reports. This is the point at which such failures surface, not later at
`:send()`.

```lua
local prod = aws.config():profile("prod"):build()

local r, listing = prod.s3.list_objects("logs"):send()
```

## Spanning regions or accounts

The reason the module exists. Each built client is independent, so
several can be live — and, under [`task`](task.md), resolving —
at once:

```lua
local regions = { "eu-central-1", "us-east-1", "ap-south-1" }
local handles = {}

for _, region in ipairs(regions) do
    table.insert(handles, task.spawn(function()
        local aws_region = aws.config():region(region):build()
        return aws_region.ec2.describe_instances()
            :instance_state("running")
            :send()
    end))
end

for i, instances in ipairs({ task.await(handles) }) do
    print(regions[i], #instances)
end
```

Two accounts is the same shape with `:profile()` instead of `:region()`.
