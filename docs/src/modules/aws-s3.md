# `aws.s3` — AWS S3

| Function | Effect |
| --- | --- |
| `aws.s3.list_buckets()` | list every bucket in the account |
| `aws.s3.list_objects(bucket)` | list objects under a bucket, optionally filtered |
| `aws.s3.list_object_versions(bucket)` | list every object version and delete marker |
| `aws.s3.head_object(bucket, key)` | fetch an object's metadata without its body |
| `aws.s3.get_object(bucket, key)` | fetch an object's body and metadata |
| `aws.s3.get_object_tagging(bucket, key)` | read one object's tag set |
| `aws.s3.put_object_tagging(bucket, key)` | **replace** one object's tag set |
| `aws.s3.delete_object_tagging(bucket, key)` | remove one object's tag set |
| `aws.s3.get_bucket_tagging(bucket)` | read a bucket's tag set |
| `aws.s3.put_bucket_tagging(bucket)` | **replace** a bucket's tag set |
| `aws.s3.delete_bucket_tagging(bucket)` | remove a bucket's tag set |
| `:prefix(prefix)` (both listings) | restrict to keys starting with `prefix` |
| `:delimiter(sep)` (both listings) | group deeper keys into `common_prefixes` |
| `:start_after(key)` (`list_objects`) | begin listing after this key |
| `:start_after(key[, id])` (`list_object_versions`) | resume at this key, optionally mid-key |
| `:max_keys(n)` (both listings) | stop after `n` entries in total |
| `:version_id(id)` (`head_object`, `get_object`, the three object tagging calls) | address a specific object version |
| `:range(range)` (`get_object`) | fetch a byte range (`bytes=0-1023`) |
| `:tags(t)` (both `put_*_tagging`) | the whole tag set to write, as `{[key]=value}` |
| `:send([target])` (the five read builders) | perform the call; returns `(envelope, data)` |
| `:send()` (the six tagging builders) | perform the call; takes no target |

Mostly read-only: the five listing/fetch operations answer questions
about buckets and objects. The only writes are the tag operations —
nothing here uploads or deletes an object.

## Example

```lua
local r, listing = aws.s3.list_objects("my-bucket")
    :prefix("logs/2026/")
    :delimiter("/")
    :send()

for _, o in ipairs(listing.objects) do
    print(o.key, o.size, o.last_modified)
end
for _, p in ipairs(listing.common_prefixes) do
    print(p)  -- "logs/2026/01/", …
end
```

## Conventions

**Builders.** Required arguments go to the entry function —
`get_object(bucket, key)`, since two arguments in that order are
unambiguous — and the returned builder carries the optional parts as
chained methods until `:send()` finalizes the call. Every setter mutates
the builder in place and returns the same handle, so aliases observe later
calls. A required argument is validated at the entry call, a setter value
at its setter; `:send()` is async and is the only place a request or an
AWS error happens.

**The two return values.** Every `:send()` either raises or returns
**two** values:

1. an envelope table, always carrying `ok` — test it first, then read
   the rest.
2. the response as native Luau data, or — when `:send()` was given a
   target — a reference to where the body went.

```lua
local r, buckets = aws.s3.list_buckets():send()
local r, body = aws.s3.get_object("b", "k"):send()
local r, path = aws.s3.get_object("b", "k"):send(proc.file("out.tar"))
```

`ok` is `false` only where non-existence is a normal answer — a missing
object (`get_object`, `head_object`). Credentials, permissions,
connectivity and a missing *bucket* raise instead: those are bugs, not
answers. The listings cannot half-succeed, so they always report
`ok = true`; the field is still there so every call site reads the same.
The tagging operations are the same: `ok` stays reserved for existence
questions and a tag read has none, so they report `ok = true` or raise —
including the untagged-bucket case S3 spells as an error (see
[`get_bucket_tagging`](#awss3get_bucket_tagging)).

**`:send([target])` and streaming.** Every *read* builder's `:send()`
accepts an optional target — the same [`proc`](proc.md) redirect values `proc`
and `ssh` take (`proc.string()`, `proc.buffer()`, `proc.file(path)`,
`proc.NULL`), so there is one vocabulary for bytes across `proc`,
`ssh` and `s3`. `proc.INHERIT` and the stdout/stderr aliases are
rejected: a body written straight to stdout would interleave with a
report that owns that stream, and there is no second stream here for
the aliases to mean anything.

Only `get_object` has a payload body carried as a byte stream. With a
`proc.file(path)` target, `get_object` writes the body to disk chunk by
chunk as it arrives, so a large download never has to fit in memory —
each write happens on the blocking pool (`tokio::task::spawn_blocking`)
so it cannot stall other concurrent tasks. With no target or a capture
target, the body is fully buffered before being handed back (`.collect()`
on the SDK stream), since a whole in-memory value is what the caller
asked for.

The other operations (`list_buckets`, `list_objects`,
`list_object_versions`, `head_object`) have no payload body — their wire
format is XML, already parsed into Luau data by the time a target is
applied — so a target on those receives that data **re-serialized as
JSON** rather than a raw response.

A missing target (`nil`) is not an error: it simply means "give me the
data directly."

The six tagging builders take **no** target: a tag set is a handful of
string pairs, not a body to stream, so their `:send()` accepts no
argument at all — passing one raises rather than being ignored.

**Pagination.** Every listing pages to exhaustion; a script never sees a
page boundary. `:max_keys(n)` caps the *total* count across all pages,
not each page — there is no per-page cap to trip over.

**Errors.** A missing key (`head_object`, `get_object`, `get_object_tagging`)
is the one AWS answer modeled as `ok = false`. Every other AWS API error (bad credentials, a
missing bucket, denied access, connectivity) raises a Luau error from
the builder's `:send()` — with one mapping, not exception: S3 answers an
untagged bucket with the `NoSuchTagSet` service error, which
`get_bucket_tagging` reads as "no tags" (`ok = true`, empty table) rather
than a failure. A malformed script argument (e.g. a target that
isn't a `proc` redirect value, or a tag table that isn't string-to-string)
raises immediately, naming the accepted forms. An empty bucket name or
object key raises at the entry function, before any request is made.

**Client selection.** Every function reads the ambient AWS
configuration (`AWS_PROFILE`, `AWS_REGION`, `~/.aws/config`, SSO,
instance metadata) the first time any of them is called, and share one
client afterwards. To use an explicit region, profile or endpoint,
build a client with [`aws.config`](aws-config.md); it exposes the same
`s3` surface under a `.s3` field:

```lua
local eu = aws.config():region("eu-central-1"):profile("prod"):build()
local r, listing = eu.s3.list_objects("logs"):send()
```

**Timestamps.** Every timestamp (`creation_date`, `last_modified`) is
RFC 3339, comparable and printable in Luau, which has no native date
type.

## `aws.s3.list_buckets`

List every bucket in the account.

**Signature:**

```
aws.s3.list_buckets() -> builder
builder:send([target]) -> envelope, data
```

**Parameters:** none.

**Returns:** a builder with one method, `:send([target])`.
`:send()` returns `(envelope, buckets)` where `buckets` is an array of
tables:

| Field | Type |
| --- | --- |
| `name` | string |
| `region` | string |
| `arn` | string |
| `creation_date` | string (RFC 3339) |

Absent fields are omitted from the table rather than set to `nil`.

Paginated to exhaustion: an account with many buckets still comes back
as one array.

**Errors:** raises on any AWS API error (credentials, permissions,
connectivity). Cannot itself report `ok = false`.

```lua
local r, buckets = aws.s3.list_buckets():send()
for _, b in ipairs(buckets) do
    print(b.name, b.region)
end
```

## `aws.s3.list_objects`

List objects (and, with a delimiter, "subdirectories") under a bucket.

**Signature:**

```
aws.s3.list_objects(bucket: string) -> builder
builder:prefix(prefix: string) -> builder
builder:delimiter(sep: string) -> builder
builder:start_after(key: string) -> builder
builder:max_keys(n: number) -> builder
builder:send([target]) -> envelope, data
```

**Parameters:**

- `bucket` (string, required) — the bucket to list.
- `:prefix(prefix)` — restrict results to keys starting with `prefix`.
- `:delimiter(sep)` — group everything past `sep` under
  `common_prefixes` instead of listing it as an object; the standard
  way to project S3's flat key space as one directory level. Without
  it, every key under the prefix is returned however deep it lies.
- `:start_after(key)` — begin listing after this key (exclusive),
  S3's own pagination cursor exposed for a manual restart point.
- `:max_keys(n)` — stop after `n` objects **in total** across all
  pages; `n` must be positive (`0` or a negative raises). The page size
  requested is `min(n, 1000)`. Every `common_prefixes` entry of the last
  page read is kept even when the cap cuts its objects short. Pagination
  otherwise runs to exhaustion, and there is no truncation flag or resume
  cursor: to continue, pass the last key to `:start_after()`.

**Returns:** `:send()` returns `(envelope, data)` where `data` has
two keys, both always present:

- `objects` — array of object tables:

  | Field | Type |
  | --- | --- |
  | `key` | string |
  | `size` | number |
  | `etag` | string |
  | `storage_class` | string |
  | `last_modified` | string (RFC 3339) |

- `common_prefixes` — array of strings, non-empty only when
  `:delimiter` was set.

**Errors:** a missing bucket raises (an AWS API error, not an answer).
An empty prefix/result is just an empty `objects` array, which already
covers "nothing there".

```lua
local r, listing = aws.s3.list_objects("my-bucket")
    :prefix("logs/2026/")
    :delimiter("/")
    :max_keys(100)
    :send()

for _, o in ipairs(listing.objects) do
    print(o.key, o.size)
end
for _, p in ipairs(listing.common_prefixes) do
    print(p)  -- "logs/2026/01/", "logs/2026/02/", …
end
```

## `aws.s3.list_object_versions`

List every version of every object under a bucket, plus the delete
markers.

On a versioned bucket `list_objects` only ever reports **current** keys:
noncurrent versions are invisible to it, and a key whose current
"version" is a delete marker does not appear at all. This is the listing
that sees them. It is a separate operation rather than a flag on
`list_objects` because its result has a different *shape* — two arrays
instead of one, since a delete marker is not an object and carries no
size, etag or storage class.

Once you have a `version_id`, reach that specific version with
`get_object`/`head_object` and `:version_id(id)`.

**Signature:**

```
aws.s3.list_object_versions(bucket: string) -> builder
builder:prefix(prefix: string) -> builder
builder:delimiter(sep: string) -> builder
builder:start_after(key: string [, version_id: string]) -> builder
builder:max_keys(n: number) -> builder
builder:send([target]) -> envelope, data
```

**Parameters:**

- `bucket` (string, required) — the bucket to list.
- `:prefix(prefix)` — restrict results to keys starting with `prefix`.
- `:delimiter(sep)` — group everything past `sep` under
  `common_prefixes` instead of listing it, exactly as for
  `list_objects`.
- `:start_after(key[, version_id])` — resume the listing. S3's version
  cursor is a *pair*, because one key can hold more versions than fit in
  a page: `key` alone starts at the next key, and adding `version_id`
  resumes part-way through that key's versions. The two are one method
  rather than two because a version id marker without its key marker is
  meaningless to S3.
- `:max_keys(n)` — stop after `n` entries **in total** across all pages,
  counting versions and delete markers **together** — that is what S3's
  own per-page limit bounds, and counting them separately would make the
  limit depend on the mix. The kept entries are a key-ordered prefix of the
  listing (versions and markers are merged back into key order before the cut).
  `n` must be positive; `0` or a negative raises. Pagination otherwise runs to
  exhaustion.

**Returns:** `:send()` returns `(envelope, data)` where `data` has
three keys, all always present:

- `versions` — array of version tables:

  | Field | Type |
  | --- | --- |
  | `key` | string |
  | `version_id` | string |
  | `is_latest` | boolean |
  | `size` | number |
  | `etag` | string |
  | `storage_class` | string |
  | `last_modified` | string (RFC 3339) |

- `delete_markers` — array of delete marker tables. A delete marker is
  not an object, so it carries only:

  | Field | Type |
  | --- | --- |
  | `key` | string |
  | `version_id` | string |
  | `is_latest` | boolean |
  | `last_modified` | string (RFC 3339) |

- `common_prefixes` — array of strings, non-empty only when
  `:delimiter` was set.

Absent scalar fields are omitted rather than set to `nil`, with one
deliberate exception: `is_latest` is **always present** on both table
kinds, defaulting to `false`. "Is this the current version?" is the
question this listing exists to answer, so it must not be an
absent-means-false read.

An unversioned bucket is not an error: every object comes back as a
single version with `is_latest = true`, and `delete_markers` is empty.
Such an object's `version_id` is the **string** `"null"` — S3's own
literal id for an object stored before versioning was enabled, and what
you pass back to `:version_id("null")` to address it. It is deliberately
left as that string rather than converted to `nil`: a version id is an
opaque identifier that merely spells "null", and omitting the field
would make a round-trip into `:version_id` silently fetch the *current*
version instead. Use `is_latest`, never a `version_id` comparison, to
ask whether a version is the current one.

**Errors:** a missing bucket raises (an AWS API error, not an answer).
An empty prefix/result is just empty arrays, which already covers
"nothing there".

```lua
-- every version of one key, newest first as S3 returns them.
-- `:prefix()` matches by prefix ("conf.json.bak" too), so filter on the key.
local r, v = aws.s3.list_object_versions("my-bucket")
    :prefix("conf.json")
    :send()

for _, ver in ipairs(v.versions) do
    if ver.key == "conf.json" then
        print(ver.version_id, ver.size, ver.last_modified,
              ver.is_latest and "(current)" or "")
    end
end

-- recover the content that preceded a delete marker
local r, v = aws.s3.list_object_versions("my-bucket"):prefix("gone.txt"):send()
local versions = {}
for _, ver in ipairs(v.versions) do
    if ver.key == "gone.txt" then table.insert(versions, ver) end
end
if #v.delete_markers > 0 and #versions > 0 then
    local r, body = aws.s3.get_object("my-bucket", "gone.txt")
        :version_id(versions[1].version_id)
        :send()
    print(body)
end

-- total bytes held by noncurrent versions under a prefix
local r, v = aws.s3.list_object_versions("my-bucket"):prefix("logs/"):send()
local waste = 0
for _, ver in ipairs(v.versions) do
    if not ver.is_latest then waste = waste + (ver.size or 0) end
end
print(waste .. " bytes in noncurrent versions")
```

## `aws.s3.head_object`

Fetch an object's metadata without transferring its body — the cheap
way to check existence and size.

**Signature:**

```
aws.s3.head_object(bucket: string, key: string) -> builder
builder:version_id(id: string) -> builder
builder:send([target]) -> envelope, data
```

**Parameters:**

- `bucket` (string, required), `key` (string, required).
- `:version_id(id)` — inspect a specific object version rather than
  the current one.

**Returns:** `:send()` returns `(envelope, data)`. On success
(`ok = true`), `data` is a table:

| Field | Type |
| --- | --- |
| `content_length` | number |
| `content_type` | string |
| `etag` | string |
| `version_id` | string |
| `storage_class` | string |
| `last_modified` | string (RFC 3339) |
| `metadata` | table |

Absent scalar fields are omitted, not set to `nil`. `metadata` (the
object's user metadata, S3's `x-amz-meta-*` headers) is always present,
possibly empty, so `.metadata.x` needs no existence guard.

On a missing object, `ok = false` and `data` is `nil`.

**Errors:** HEAD has no response body, so S3 cannot return an error
payload for "not found" — the SDK models it as a bare not-found, which
this binding treats as `ok = false` rather than raising. S3 uses the same
answer for a missing *bucket*, so on a not-found one extra `HeadBucket` request
tells the two apart: a missing bucket **raises**
(`head_object: bucket '<name>' does not exist`). A 403 (no `ListBucket`
permission) also looks like "not found" and stays `ok = false`. Every other
failure (bad credentials, denied access) raises.

```lua
local r, meta = aws.s3.head_object("my-bucket", "big.iso"):send()
if r.ok then
    print(meta.content_length, meta.storage_class)
else
    print("absent")
end
```

## `aws.s3.get_object`

Fetch an object's body and metadata.

**Signature:**

```
aws.s3.get_object(bucket: string, key: string) -> builder
builder:version_id(id: string) -> builder
builder:range(range: string) -> builder
builder:parse(format: "json" | "yaml" | "toml") -> builder
builder:send([target]) -> envelope, body
```

**Parameters:**

- `bucket` (string, required), `key` (string, required).
- `:version_id(id)` — fetch a specific object version.
- `:range(range)` — a raw HTTP range header value, e.g.
  `"bytes=0-1023"` — the spelling S3 documents.
- `target` (optional, to `:send()`) — a `proc` redirect value
  (`proc.string()`, `proc.buffer()`, `proc.file(path)`, `proc.NULL`).

**Returns:** the envelope, on success (`ok = true`), also carries
response metadata directly (not nested): `content_type`,
`content_length`, `etag`, `version_id`, `last_modified` — each omitted
when S3 didn't report it.

The second return value depends on the target:

| Target | Second value |
| --- | --- |
| *(none)* | the body as a string (or decoded data with `:parse(format)`) |
| `proc.string()` | the body as a string, unparsed |
| `proc.buffer()` | the body as a `buffer`, unparsed |
| `proc.file(path)` | the path written to; streamed to disk chunk by chunk |
| `proc.NULL` | `nil` — body drained (so a mid-body error still surfaces) and discarded |

With no target, the body is **always a string** — the return type never
depends on the `Content-Type` the uploader happened to set. A Luau string is
8-bit clean, so a binary body survives intact. Parsing is on request:
`:parse("json")` (or `"yaml"`/`"toml"`) decodes the body, and a body that is not
valid UTF-8 or does not parse as that format **raises**
(`get_object:parse: body is not valid json: …`); `json.parse(body)` is the
equivalent by hand. A `:parse()` is ignored when a target is given (targets
receive raw bytes).

On a missing key, `ok = false` and the second value is `nil` — no
target is written to in that case, so a not-found never creates a file
or writes `null` into one.

**Errors:** a missing key is `ok = false` (a normal answer). A missing
bucket, denied access or bad/expired credentials raises, as does an
invalid target argument.

```lua
-- decoded on request
local r, cfg = aws.s3.get_object("my-bucket", "conf.json"):parse("json"):send()
if r.ok then print(cfg.database.host) end

-- streamed to disk, never held in memory
local r, path = aws.s3.get_object("my-bucket", "backup.tar")
    :send(proc.file("backup.tar"))
if r.ok then print("wrote " .. path) end

-- a byte range
local r, chunk = aws.s3.get_object("my-bucket", "big.log")
    :range("bytes=0-1023")
    :send(proc.string())

-- missing key
local r, body = aws.s3.get_object("my-bucket", "maybe.txt"):send()
if not r.ok then print("not there") end
```

## Tagging

Tags are their own six operations because S3 gives them no other door: a
listing never carries them, and `head_object` reports only how *many* an
object has (the `x-amz-tagging-count` header), never which. So reading
one object's tags is always one more request — a sweep over a prefix
costs a request per key — and that is why these functions exist instead
of being a field on `list_objects`.

**A put replaces the whole tag set.** `put_object_tagging` and
`put_bucket_tagging` are not a merge, and S3 offers no merging form:
adding one tag means reading the set, modifying the returned table, and
putting it back. The read side hands back exactly the shape the write
side takes — a `{[key]=value}` map of strings — so that round-trip needs
no reshaping:

```lua
local r, tags = aws.s3.get_object_tagging("my-bucket", "conf.json"):send()
tags.Reviewed = "yes"              -- add one, keep the rest
aws.s3.put_object_tagging("my-bucket", "conf.json"):tags(tags):send()
```

Dropping the `get_*` there would delete every other tag on the object.

`:tags(t)` requires **string** keys and values. A number raises rather
than being coerced: a silent `tostring` would put `"1"` on a bucket where
the script wrote `1`. The conversion happens inside the `:tags()` call,
so a malformed table raises at the line that wrote it, not later at
`:send()`. Tags go on the wire sorted by key, which makes a request
reproducible; an empty value is a legal tag and round-trips as one.

`:tags()` is *required* on a put — an omitted one would silently wipe the
set, and wiping is what `delete_*_tagging` says out loud.

`get_*_tagging` always returns a table, possibly empty, never `nil`, so
`.tags.Env` needs no existence guard — the same judgement `head_object`'s
`metadata` makes.

## `aws.s3.get_object_tagging`

Read one object's tag set.

**Signature:**

```
aws.s3.get_object_tagging(bucket: string, key: string) -> builder
builder:version_id(id: string) -> builder
builder:send() -> envelope, tags
```

**Parameters:**

- `bucket` (string, required), `key` (string, required) — neither may be
  empty.

**Methods:**

- `:version_id(id)` — read the tags of a specific object version rather
  than the current one.
- `:send()` — finalize the builder and run the request (async).

**Returns:** `(envelope, tags)`. `tags` is a `{[key]=value}` table
of strings, always a table — an untagged object yields an empty one. The
envelope carries `ok = true` and, when S3 reported one, `version_id`: the
version the tags were read from.

`:send()` takes no target — there is no body to stream.

**Errors:** a missing key is `ok = false` with `nil` data, as for
`head_object`/`get_object`; an empty tag table means "no tags", not "no
object". A missing bucket, denied access or bad credentials raises.

```lua
local r, tags = aws.s3.get_object_tagging("my-bucket", "conf.json"):send()
print(tags.Env or "(untagged)")
```

## `aws.s3.put_object_tagging`

Replace one object's entire tag set.

**Signature:**

```
aws.s3.put_object_tagging(bucket: string, key: string) -> builder
builder:version_id(id: string) -> builder
builder:tags(t: table) -> builder
builder:send() -> envelope, nil
```

**Parameters:**

- `bucket` (string, required), `key` (string, required) — neither may be
  empty.

**Methods:**

- `:version_id(id)` — tag a specific object version rather than the
  current one.
- `:tags(t)` — **required.** The complete tag set as `{[key]=value}`,
  string to string. This is what the object will carry afterwards:
  anything not in `t` is removed.
- `:send()` — finalize the builder and run the request (async).

**Returns:** `(envelope, nil)`. There is nothing to hand back, so
the second value is always `nil`. The envelope carries `ok = true` and,
when S3 reported one, `version_id` — the version the tags landed on,
which on a versioned bucket is the only way to know which that was.

**Errors:** a missing `:tags()` raises
`put_object_tagging requires :tags(t) — a put replaces the entire tag
set; use delete_object_tagging to clear it`. A non-string key or value
raises at the `:tags()` call (`tags must be a table of string keys to
string values, got …`). An empty bucket or key raises at the entry
function. Every AWS API error (missing bucket or key, denied access, bad
credentials) raises from `:send()`.

```lua
-- read, modify, write: the only way to add a tag
local r, tags = aws.s3.get_object_tagging("my-bucket", "conf.json"):send()
tags.Owner = "infra"
aws.s3.put_object_tagging("my-bucket", "conf.json"):tags(tags):send()

-- set the tag set outright, discarding whatever was there
local r = aws.s3.put_object_tagging("my-bucket", "conf.json")
    :tags({ Env = "prod", Owner = "infra" })
    :send()
```

## `aws.s3.delete_object_tagging`

Remove one object's tag set entirely.

**Signature:**

```
aws.s3.delete_object_tagging(bucket: string, key: string) -> builder
builder:version_id(id: string) -> builder
builder:send() -> envelope, nil
```

**Parameters:**

- `bucket` (string, required), `key` (string, required) — neither may be
  empty.

**Methods:**

- `:version_id(id)` — clear the tags of a specific object version.
- `:send()` — finalize the builder and run the request (async).

**Returns:** `(envelope, nil)`. The envelope carries `ok = true`
and, when S3 reported one, `version_id`.

**Errors:** raises on any AWS API error. Clearing an already-untagged
object is not an error.

```lua
aws.s3.delete_object_tagging("my-bucket", "conf.json"):send()
```

## `aws.s3.get_bucket_tagging`

Read a bucket's tag set.

**Signature:**

```
aws.s3.get_bucket_tagging(bucket: string) -> builder
builder:send() -> envelope, tags
```

**Parameters:**

- `bucket` (string, required) — may not be empty.

**Returns:** `(envelope, tags)`, with `tags` a `{[key]=value}` table
of strings, always a table. The envelope carries `ok = true`.

**Errors:** S3 answers an **untagged** bucket with the `NoSuchTagSet`
service error rather than an empty document. That is not a failure — it
is "no tags" — so this binding maps it to `ok = true` with an empty
table, which also makes the untagged-bucket and untagged-object paths
read the same. `ok` stays reserved for existence questions, and a tag
read has none: an untagged bucket is a bucket. Everything else still
raises: a missing bucket, denied access, bad credentials, connectivity.

Note the consequence: an empty table does **not** tell you the bucket
exists. Use `list_buckets` or `head_object` for that question.

```lua
local r, tags = aws.s3.get_bucket_tagging("my-bucket"):send()
for k, v in pairs(tags) do print(k, v) end   -- silent if untagged
```

## `aws.s3.put_bucket_tagging`

Replace a bucket's entire tag set.

**Signature:**

```
aws.s3.put_bucket_tagging(bucket: string) -> builder
builder:tags(t: table) -> builder
builder:send() -> envelope, nil
```

**Parameters:**

- `bucket` (string, required) — may not be empty.

**Methods:**

- `:tags(t)` — **required.** The complete tag set as `{[key]=value}`,
  string to string; anything absent from `t` is removed from the bucket.
- `:send()` — finalize the builder and run the request (async).

**Returns:** `(envelope, nil)`. The envelope carries `ok = true`
only — a bucket has no version, so no `version_id`.

**Errors:** a missing `:tags()` raises
`put_bucket_tagging requires :tags(t) — a put replaces the entire tag
set; use delete_bucket_tagging to clear it`. A non-string key or value
raises at the `:tags()` call. An empty bucket name raises at the entry
function. AWS API errors raise from `:send()`.

```lua
-- add a tag without losing the existing ones
local r, tags = aws.s3.get_bucket_tagging("my-bucket"):send()
tags["cost-center"] = "4711"
aws.s3.put_bucket_tagging("my-bucket"):tags(tags):send()
```

## `aws.s3.delete_bucket_tagging`

Remove a bucket's tag set entirely.

**Signature:**

```
aws.s3.delete_bucket_tagging(bucket: string) -> builder
builder:send() -> envelope, nil
```

**Parameters:**

- `bucket` (string, required) — may not be empty.

**Returns:** `(envelope, nil)`, the envelope carrying `ok = true`.

**Errors:** raises on any AWS API error. Clearing an already-untagged
bucket is not an error.

```lua
aws.s3.delete_bucket_tagging("my-bucket"):send()
```

## Concurrency

`:send()` yields rather than blocking, so several calls under
[`task`](task.md) overlap:

```lua
local handles = {}
for _, bucket in ipairs({ "logs-eu", "logs-us" }) do
    table.insert(handles, task.spawn(function()
        local r, listing = aws.s3.list_objects(bucket):prefix("2026/"):send()
        return listing.objects
    end))
end
local eu, us = task.await(handles)
```

Concurrent first calls share one client rather than each resolving
credentials separately. A `proc.file()` target streams through the
blocking pool, so a large download does not stall its sibling tasks.

## Object writes

The tag operations are the only writes. There is no `:body()` method for
uploading an object, and no delete: `get_object` is the only body path,
and it reads. All of the builders' `:send()` therefore means one thing in
both directions — perform the call, return `(envelope, data)`.
