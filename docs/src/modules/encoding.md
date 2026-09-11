# `encoding` — binary-to-text encodings

| Function | Effect |
| --- | --- |
| `encoding.base64.encode(<data>)` | standard, padded base64 text |
| `encoding.base64.decode(<text>)` | the decoded bytes, as a string |
| `encoding.base64.engine()` | a builder for another alphabet/padding |
| `<engine>:alphabet(<name>)` | a new builder with that alphabet |
| `<engine>:padding(<on>)` | a new builder that pads (or not) when encoding |
| `<engine>:decode_padding(<mode>)` | a new builder with that decode-padding rule |
| `<engine>:encode(<data>)` / `<engine>:decode(<text>)` | encode/decode with the builder's alphabet and padding |

## Example

```lua
encoding.base64.encode("hi")            --> "aGk="
encoding.base64.decode("aGk=")          --> "hi"

-- URL-safe, unpadded (JWT segments, SRI-style tokens).
local url = encoding.base64.engine():alphabet("url_safe"):padding(false)
url:encode(hash.sha256_bytes("abc"))
```

Only base64 is exposed; hex is an internal helper, not part of this module.
The module is **pure** and present in both programs.

## Conventions

**Defaults.** `encode`/`decode` are the standard alphabet (`+`, `/`), padded
on encode, and **canonical padding required** on decode (an unpadded or
mis-padded input raises). The builder mirrors the Rust `base64` crate's
engine configuration.

**Builders are immutable.** `encoding.base64.engine()` starts the builder;
`:alphabet`, `:padding` and `:decode_padding` configure it and
`:encode`/`:decode` are the terminals. Each step returns a *new* builder and
leaves the receiver unchanged, so a half-configured engine can be shared
and the base keeps its settings — unlike the mutable builders in `hcl`,
`proc` and `ui.prompt`.

**Bytes in, bytes out.** `encode` takes a **string or `buffer`** (nothing is
UTF-8 validated), which is what `hash.*_bytes` and `proc.buffer()` hand
back. `decode` returns a **string**, which in Luau is 8-bit clean; wrap it
in `buffer.fromstring` if a buffer is wanted. Any other argument type raises
`"encoding.base64.<fn>: expected a string or buffer, got <type>"`.

## `encoding.base64.encode` / `encoding.base64.decode`

**Signature:** `encoding.base64.encode(data) -> string`,
`encoding.base64.decode(text) -> string`

**Parameters:** `data` — a string or buffer; `text` — base64 text (string or
buffer).

**Errors:** `decode` raises `"encoding.base64.decode: <reason>"` on an invalid
byte, invalid length, or wrong padding. `encode` cannot fail.

```lua
encoding.base64.encode(buffer.fromstring("abc"))   --> "YWJj"
```

## `encoding.base64.engine`

**Signature:** `encoding.base64.engine() -> engine` — starts from the defaults
above. `tostring(engine)` shows the alphabet and padding.

**Methods:** `alphabet` · `padding` · `decode_padding` · `encode` · `decode`

### `engine:alphabet`

**Signature:** `engine:alphabet(name) -> engine`

`name` is one of `"standard"` (default), `"url_safe"` (`-`, `_`), `"crypt"`,
`"bcrypt"`, `"imap_mutf7"`, `"bin_hex"`. Raises on any other name.

### `engine:padding`

**Signature:** `engine:padding(on: boolean) -> engine`

Whether `encode` appends `=` padding (default `true`).

### `engine:decode_padding`

**Signature:** `engine:decode_padding(mode) -> engine`

What `decode` accepts: `"required"` (default; canonical padding only),
`"none"` (no padding allowed), or `"indifferent"` (either). Raises on any
other mode. Pair `:padding(false)` with `:decode_padding("none")` (or
`"indifferent"`) for an unpadded round trip.

### `engine:encode` / `engine:decode`

Same as the module-level functions, using the builder's alphabet and padding.

```lua
local e = encoding.base64.engine():alphabet("url_safe"):padding(false):decode_padding("none")
e:encode(string.char(251, 255))   --> "-_8"
e:decode("-_8") == string.char(251, 255)
```
