# `http` — HTTP requests

`http` sends HTTP and HTTPS requests: a client builder for shared settings
(base URL, headers, auth, timeouts, retries, TLS, proxy), a request builder per
call, and an async `:send([<target>])` that returns the response's envelope and
body. The body is bytes, never deserialized for you: pass it to
[`json.parse`](json.md) or [`yaml.parse`](yaml.md) yourself. Absent under
`ied`, because the network is ambient state. `if http then` is the feature test.

| Function | Effect |
| --- | --- |
| `http.get(url)` / `post` / `put` / `patch` / `delete` / `head` | start a request on the default client |
| `http.request(method, url)` | start a request with any method on the default client |
| `http.client()` | start a client builder |

| Method | Effect |
| --- | --- |
| `Client:get(url)` / `post` / `put` / `patch` / `delete` / `head` | start a request, `url` resolved against the base URL |
| `Client:request(method, url)` | start a request with any method |
| `Request:header(name, value)` | add a header (accumulates) |
| `Request:headers(t)` | set several headers, replacing those names |
| `Request:basic_auth(user, [pass])` / `bearer_auth(token)` | set `Authorization` |
| `Request:query(t)` | append query parameters |
| `Request:body(source)` | the body: a string, a buffer, `proc.file(path)` (streamed) |
| `Request:json(value)` | the body as JSON, with `Content-Type: application/json` |
| `Request:form(t)` | the body URL-encoded, as an HTML form |
| `Request:multipart(t)` | the body as `multipart/form-data`, files streamed |
| `Request:timeout(delta)` | bound this request's total time |
| `Request:retries(n)` | total attempts for this request |
| `Request:expect(status, ...)` | raise on any other status |
| `Request:keep_failure_body([bool])` | write a non-2xx body to a `proc.file` target too |
| `Request:send([target])` | perform the request (async); returns `(envelope, body)` |

## Example

```lua
local api = http.client()
    :base_url("https://api.github.com/")
    :header("Accept", "application/vnd.github+json")
    :bearer_auth(os.getenv("GITHUB_TOKEN"))
    :retries(3)
    :build()

local r, body = api:get("repos/denfren/ito/releases"):query({ per_page = 5 }):send()
if not r.ok then
    error("GitHub said " .. r.status .. ": " .. body)
end
for _, release in ipairs(json.parse(body)) do
    print(release.tag_name, release.published_at)
end

-- a download, streamed to disk; a 404 raises rather than leaving an error page
api:get("https://example.com/app.tgz"):expect(200):send(proc.file("app.tgz"))
```

## Conventions

**Constructor → chain → terminal.** `http.client()` starts a client builder.
Its setters configure it, and `:build()` turns it into a `Client`.
`client:get(url)` (and the other methods) starts a request builder. Its setters
configure the request, and the async `:send([<target>])` performs it.
`http.get(url)` and the other module-level calls are the same requests on a
default client, which is built on first use and then shared. It is equivalent
to `http.client():build()`.

**The client builder copies, the request builder mutates.** Every client
builder setter returns a *new* builder and leaves the receiver as it was, so one
base can be refined several ways without the variants interfering. Every request
setter changes the request in place and returns that same handle, so aliases see
later calls and a discarded return value still counts:

```lua
local base = http.client():timeout("10s")
local a, b = base:header("X-Env", "a"):build(), base:header("X-Env", "b"):build()

local req = http.get(url)
req:header("X-Trace", id)   -- applies to req
local r, body = req:send()
```

A request can be sent more than once. Each `:send()` sends what the request
holds at that moment.

**When validation happens.** A value is checked at the setter that receives
it: a header name or value, a URL, a delta, a status, a part. A relative URL
on a client without `:base_url()` raises at `get`/`request`. Files are read only
when they are used. A `:ca_cert()`/`:identity()` file is read at `:build()`,
so a bad certificate raises there. A `:body(proc.file(path))` upload is opened
at `:send()`.

**Replace or accumulate.** `:header` adds a value, so calling it twice sends the
header twice. `:headers(t)` replaces every value of each name it lists. `:query`
appends parameters. `:body`, `:json`, `:form` and `:multipart` each replace
the whole body. `:basic_auth`/`:bearer_auth`, `:timeout`, `:retries`,
`:expect` and `:keep_failure_body` replace the previous setting. On the client,
`:ca_cert` accumulates and every other setter replaces.

**Client defaults and per-request settings.** A request header replaces the
client's default header *of the same name*. Other client headers are sent
unchanged. Request-level `:basic_auth`/`:bearer_auth` replaces a
client-level one the same way. The default `User-Agent` is `ito/<version>`.
`:timeout`, `:retries` and `:keep_failure_body` on a request override the
client's setting.

**`:send([<target>])` and its two return values.** `:send()` is async, so it
yields under [`task`](task.md) and several requests can overlap. It either
raises or returns **two** values:

1. an envelope table: `ok` (`true` for 2xx), `status` (a number), `url`
   (the final URL, after redirects), `version` (`"HTTP/1.1"`, `"HTTP/2.0"`)
   and `headers` (lower-case names to string values).
2. the body, delivered to the target:

| Target | Second value |
| --- | --- |
| none (`nil`) | the body as a Luau string (8-bit clean, so binary survives) |
| `proc.string()` | the same, a string |
| `proc.buffer()` | the body as a Luau `buffer` |
| `proc.file(path)` | `path`, after the body was streamed to disk chunk by chunk; `nil` if the response was not 2xx (see below) |
| `proc.NULL` | `nil`; the body is read and discarded |

These are the [`proc` redirect values](redirect.md). `proc.INHERIT`,
`proc.OUT`/`ERR` and the `STDOUT`/`STDERR` aliases are rejected. A `proc.file`
handle reused across sends continues at its offset, as it does for `proc`.
`{append = true}` appends.

**A status is an answer.** A 404 or a 500 is not an error: `r.ok` is
`false`, `r.status` says which, and the body is there to read. Only transport
failures raise (DNS, connection, TLS, a timeout, a body cut off mid-read).
`:expect(...)` makes a status an error as well. Any status outside the list
raises `http: <method> <url>: status <n>, expected <list>: <start of body>`.

**A failed response never reaches a file by default.** With a
`proc.file(path)` target, the body is written only for a 2xx response. For any
other status the file is not opened, created or truncated, and the second
value is `nil`. Without this rule a failed download would leave behind a file
that looks fine but holds an error page. `:keep_failure_body()`, set on the
request or as the client's default, writes the body whatever the status.
String, buffer and `NULL` targets are unaffected.

**Retries.** `:retries(n)` is the *total* number of attempts (`1`, the
default, disables retrying). It applies only to idempotent methods (`GET`,
`HEAD`, `PUT`, `DELETE`, `OPTIONS`, `TRACE`), because a retried `POST` that
had reached the server would act twice. An attempt is retried after a
connection failure, after a timeout, or after a `429`, `502`, `503` or `504`
response. A `500` is not retried. The wait is the response's `Retry-After`
(in seconds or as an HTTP date), or else an exponential backoff starting at
0.5s and capped at 8s. A `Retry-After` over 60s is not waited out: that
response is returned. Each attempt rebuilds the request, so a file body is
reopened.

**URLs.** A URL must be `http` or `https`. A client's `:base_url()` is joined
with each request URL by standard reference resolution (RFC 3986), the way a
browser resolves a link:

| Base | Request | Result |
| --- | --- | --- |
| `https://h/v1/` | `users` | `https://h/v1/users` |
| `https://h/v1` | `users` | `https://h/users` (no trailing slash: `v1` is replaced) |
| `https://h/v1/` | `/users` | `https://h/users` |
| `https://h/v1/` | `https://other/x` | `https://other/x` |

End a base URL with `/` unless you mean otherwise.

**Tables of pairs.** `:query`, `:form` and `:headers` take `{ name = value }`.
A value may be a string, a number or a boolean. An array value repeats the
key (`{ tag = { "a", "b" } }` → `tag=a&tag=b`). Keys are sent in sorted order,
because a Luau table has no stable order and a request should be the same on
every run.

**Not included.** Cookies are not stored between requests (read
`r.headers["set-cookie"]` and send `Cookie` yourself). Responses are not
decompressed. Paging is not done for you, since every API pages differently.

## `http.get` / `http.post` / `http.put` / `http.patch` / `http.delete` / `http.head`

Start a request with that method on the default client.

**Signature:** `http.get(url: string) -> Request`

**Parameters:** `url` — an absolute `http`/`https` URL. The default client
has no base URL.

**Returns:** a `Request` builder. Nothing is sent until `:send()`.

**Errors:** raises `http.get: …` (naming the function) on a relative URL, a
malformed one or another scheme.

```lua
local r, body = http.get("https://example.com/"):send()
print(r.status, #body)   --> 200	1256
```

## `http.request`

Start a request with any method on the default client.

**Signature:** `http.request(method: string, url: string) -> Request`

**Parameters:**
- `method` — the method name. It is upper-cased, so `"options"` sends
  `OPTIONS`. Extension methods are allowed.
- `url` — as for [`http.get`](#httpget--httppost--httpput--httppatch--httpdelete--httphead).

**Returns:** a `Request` builder.

**Errors:** raises `http.request: …` for a method that is not a valid token,
or for a bad URL.

```lua
local r = http.request("OPTIONS", "https://example.com/"):send()
print(r.headers.allow)
```

## `http.client`

Start a client builder with every setting at its default.

**Signature:** `http.client() -> ClientBuilder`

**Returns:** a `ClientBuilder`. Every setter returns a new builder, and
`:build()` returns a `Client`.

**Methods:**

| Method | Effect | Default |
| --- | --- | --- |
| `:base_url(url)` | resolve relative request URLs against `url` | none |
| `:header(name, value)` | add a default header (accumulates) | — |
| `:headers(t)` | set default headers, replacing those names | — |
| `:basic_auth(user, [pass])` | default `Authorization: Basic …` | none |
| `:bearer_auth(token)` | default `Authorization: Bearer …` | none |
| `:user_agent(s)` | the `User-Agent` | `ito/<version>` |
| `:timeout(delta)` | total time per request, including the body | none |
| `:connect_timeout(delta)` | time to establish a connection | none |
| `:retries(n)` | total attempts, `1`–`100` (see Conventions) | `1` |
| `:redirect(n)` | follow at most `n` redirects, `0` to follow none | `10` |
| `:proxy(url)` | send every request through this proxy | the environment's |
| `:no_proxy()` | use no proxy, ignoring the environment | — |
| `:ca_cert(path)` | also trust the PEM certificates in `path` (accumulates) | system roots |
| `:identity(path)` | present this client certificate (mTLS) | none |
| `:insecure([bool])` | skip certificate verification | `false` |
| `:keep_failure_body([bool])` | default for [`Request:keep_failure_body`](#requestkeep_failure_body) | `false` |
| `:build()` | finalize into a `Client` | — |

A delta is anything [`time.parse_delta`](time.md) accepts: seconds as a
number, or a string such as `"30s"` or `"1m 30s"`. A path may be a string or
a `proc.file(path)`, relative to the working directory. By default the proxy
comes from `HTTP_PROXY`/`HTTPS_PROXY`/`ALL_PROXY`/`NO_PROXY` and the system
settings. Certificates are verified against the operating system's trust store.
`:ca_cert()` adds to it rather than replacing it.

**Errors:** a setter raises `http.client:<method>: …` on a bad value (an empty
or malformed URL, an invalid header, a non-positive delta, a count out of
range, a malformed proxy URL). `:build()` raises `http.client:build: …` when a
certificate or identity file cannot be read or holds no usable PEM data.

```lua
local internal = http.client()
    :base_url("https://registry.internal/api/")
    :ca_cert("certs/internal-ca.pem")
    :identity("certs/client.pem")   -- certificate chain plus private key
    :timeout("20s")
    :build()
```

## `Client:get` / `post` / `put` / `patch` / `delete` / `head`

Start a request with that method on this client.

**Signature:** `client:get(url: string) -> Request`

**Parameters:** `url` — resolved against the client's `:base_url()` (see
Conventions). An absolute URL stands alone.

**Returns:** a `Request` builder carrying the client's settings.

**Errors:** raises `http.client:get: …` (naming the method) on a malformed
URL, a non-http(s) scheme, or a relative URL when the client has no base URL.

```lua
local r, body = api:delete("sessions/" .. id):expect(204):send()
```

## `Client:request`

Start a request with any method on this client.

**Signature:** `client:request(method: string, url: string) -> Request`

**Parameters:**
- `method` — upper-cased, as for [`http.request`](#httprequest).
- `url` — as for [`Client:get`](#clientget--post--put--patch--delete--head).

**Returns:** a `Request` builder.

**Errors:** raises `http.client:request: …` for an invalid method or URL.

```lua
local r = api:request("PROPFIND", "files/"):header("Depth", "1"):send()
```

## `Request:header`

Add one header value.

**Signature:** `request:header(name: string, value: string) -> Request`

**Parameters:**
- `name` — the header name, case-insensitive.
- `value` — a string or a number. It must not contain a newline or a control
  character.

**Returns:** the same request.

**Errors:** raises `http.request:header: …` on an invalid name or value.

```lua
req:header("Accept", "application/json"):header("X-Request-Id", id)
```

## `Request:headers`

Set several headers at once, replacing every existing value of each name
given.

**Signature:** `request:headers(t: {[string]: string | {string}}) -> Request`

**Parameters:** `t` — header names to values. An array value sends the
header once per element.

**Returns:** the same request.

**Errors:** raises `http.request:headers: …` on an invalid name or value.

```lua
req:headers({ ["If-None-Match"] = etag, Accept = { "text/html", "*/*" } })
```

## `Request:basic_auth` / `bearer_auth`

Set the `Authorization` header, replacing any client default.

**Signature:** `request:basic_auth(user: string, pass?: string) -> Request`,
`request:bearer_auth(token: string) -> Request`

**Parameters:**
- `user` / `pass` — sent as `Basic base64(user:pass)`. A missing password is
  sent as empty.
- `token` — sent as `Bearer <token>`. It must not be empty.

**Returns:** the same request.

**Errors:** raises on an empty token or one containing a newline.

```lua
req:basic_auth("deploy", os.getenv("DEPLOY_PASSWORD"))
```

## `Request:query`

Append query parameters to the URL.

**Signature:** `request:query(t: {[string]: any}) -> Request`

**Parameters:** `t` — names to scalars, or to arrays of scalars to repeat a
name (see Conventions). Values are percent-encoded. Parameters already in the
URL are kept, and these follow them.

**Returns:** the same request.

**Errors:** raises `http.request:query: …` on a value that is not a string,
number or boolean.

```lua
api:get("search"):query({ q = "a b", tag = { "x", "y" } })
-- → search?q=a+b&tag=x&tag=y
```

## `Request:body`

Set the body to raw bytes, or to a file streamed from disk.

**Signature:** `request:body(source: string | buffer | Redirect) -> Request`

**Parameters:** `source` — a string or a buffer (sent as-is), a
`proc.file(path)` (opened at `:send()` and streamed, with `Content-Length`
set from the file's size), or `proc.NULL` for no body. No `Content-Type` is
added, so set one with `:header` if the server needs it.

**Returns:** the same request.

**Errors:** raises `http.request:body: …` for any other value. A file that
cannot be opened raises at `:send()`.

```lua
api:put("artifacts/app.tgz")
    :header("Content-Type", "application/gzip")
    :body(proc.file("dist/app.tgz"))
    :expect(200, 201)
    :send()
```

## `Request:json`

Set the body to a Luau value serialized as JSON.

**Signature:** `request:json(value: any) -> Request`

**Parameters:** `value` — anything [`json.to_string`](json.md) accepts, with
the same rules (`null`, `table.array` for an empty array). Adds
`Content-Type: application/json` unless the request already sets a
`Content-Type`.

**Returns:** the same request.

**Errors:** raises at the call for a value with no JSON form (a function, NaN,
a cyclic table), naming where it sits in the value.

```lua
api:post("issues"):json({ title = "Disk full", labels = { "ops" } }):send()
```

## `Request:form`

Set the body to URL-encoded form fields.

**Signature:** `request:form(t: {[string]: any}) -> Request`

**Parameters:** `t` — as for [`Request:query`](#requestquery). Adds
`Content-Type: application/x-www-form-urlencoded` unless one is set.

**Returns:** the same request.

**Errors:** raises `http.request:form: …` on an unsupported value.

```lua
http.post(token_url):form({ grant_type = "client_credentials", scope = "read" })
```

## `Request:multipart`

Set the body to `multipart/form-data`, the encoding of an HTML form with file
inputs.

**Signature:** `request:multipart(t: {[string]: Part | {Part}}) -> Request`

**Parameters:** `t` — field names to parts. An array value sends several
parts under one name. A part is one of:
- a string or buffer — a plain field.
- `proc.file(path)` — a file, streamed at `:send()`. Its filename is the
  path's last component, and its content type is guessed from the extension
  (`application/octet-stream` when unknown).
- `{ value = <string | buffer | proc.file(path)>, filename = ?, content_type = ? }`
  — either of the above with its filename and/or content type set explicitly.

**Returns:** the same request. The `Content-Type` (with its boundary) is set
for you.

**Errors:** raises `http.request:multipart: …` on an unsupported part, a
table without `value`, or a malformed content type. A file that cannot be
opened raises at `:send()`.

```lua
api:post("upload"):multipart({
    comment = "nightly",
    file = proc.file("report.pdf"),
    meta = { value = json.to_string(meta), content_type = "application/json" },
}):send()
```

## `Request:timeout`

Bound this request's total time, overriding the client's `:timeout()`.

**Signature:** `request:timeout(delta: number | string) -> Request`

**Parameters:** `delta` — a positive delta (`5`, `"500ms"`, `"2m"`). It covers
connecting, sending and reading the whole body, for each attempt separately.

**Returns:** the same request.

**Errors:** raises `http.request:timeout: …` on a non-positive or
unparseable delta. When the time runs out, `:send()` raises (or retries,
under `:retries`).

```lua
http.get(health_url):timeout("2s"):send()
```

## `Request:retries`

Set the total number of attempts for this request, overriding the client's
setting.

**Signature:** `request:retries(n: integer) -> Request`

**Parameters:** `n` — `1` to `100`. `1` means one attempt and no retry. Has no
effect on a non-idempotent method such as `POST` (see Conventions).

**Returns:** the same request.

**Errors:** raises `http.request:retries: …` on a value out of range or not a
whole number.

```lua
http.get(url):retries(5):send()
```

## `Request:expect`

Treat any status not in the list as an error.

**Signature:** `request:expect(status: integer, ...: integer) -> Request`

**Parameters:** one or more statuses, each `100`–`599`. Replaces any earlier
list.

**Returns:** the same request.

**Errors:** raises `http.request:expect: …` with no arguments or an
out-of-range status. At `:send()`, any other status raises
`http: <method> <url>: status <n> <reason>, expected <list>`, followed by up to
512 bytes of the response body. The target is not touched in that case.

```lua
local _, body = api:get("config"):expect(200):send()
```

## `Request:keep_failure_body`

Write a non-2xx response body to a `proc.file` target as well, overriding the
client's default.

**Signature:** `request:keep_failure_body(keep?: boolean) -> Request`

**Parameters:** `keep` — `true` when omitted. `false` restores the default
(only a 2xx body reaches the file).

**Returns:** the same request.

```lua
local r, path = http.get(url):keep_failure_body():send(proc.file("response.html"))
if not r.ok then log.warn("saved the error page to " .. path) end
```

## `Request:send`

Perform the request. This is async.

**Signature:** `request:send(target?: Redirect) -> (Envelope, any)`

**Parameters:** `target` — where the body goes: `nil` for a string,
`proc.string()`, `proc.buffer()`, `proc.file(path)` or `proc.NULL` (see
Conventions).

**Returns:** `(envelope, body)`. `envelope` is
`{ ok, status, url, version, headers }`. `body` is the string, the buffer,
the path, or `nil` as the target dictates. A `HEAD` response has an empty body.

**Errors:** raises `http: <method> <url>: …` when no response was received
(DNS, connection refused, TLS verification, a timeout), when the body broke off
mid-read, or when a body file could not be opened. A status outside
`:expect(...)` raises as described there. Any other status is returned, not
raised. A target that is not a body target raises
`http.request:send: expected a body target …`.

```lua
local r, body = http.get("https://example.com/missing"):send()
print(r.ok, r.status)   --> false	404
```

## Running requests concurrently

`:send()` yields while it waits for the network, so requests started under
[`task`](task.md) overlap. Share one `Client` between them so connections are
reused:

```lua
local api = http.client():base_url("https://api.example.com/"):build()
local pool = task.pool(8)
for _, id in ipairs(ids) do
    pool:spawn(function()
        local r, body = api:get("items/" .. id):send()
        return id, r.status, body
    end)
end
while not pool:done() do
    local id, status = pool:await()
    print(id, status)
end
```
