# `hash` — cryptographic digests

| Function | Effect |
| --- | --- |
| `hash.md5(<value>)` | MD5 of a string or buffer, as lowercase hex |
| `hash.sha1(<value>)` | SHA-1 of a string or buffer, as lowercase hex |
| `hash.sha224(<value>)` | SHA-224 of a string or buffer, as lowercase hex |
| `hash.sha256(<value>)` | SHA-256 of a string or buffer, as lowercase hex |
| `hash.sha384(<value>)` | SHA-384 of a string or buffer, as lowercase hex |
| `hash.sha512(<value>)` | SHA-512 of a string or buffer, as lowercase hex |
| `hash.md5_bytes(<value>)` | the same digest as a raw `buffer` |
| `hash.sha1_bytes(<value>)` | the same digest as a raw `buffer` |
| `hash.sha224_bytes(<value>)` | the same digest as a raw `buffer` |
| `hash.sha256_bytes(<value>)` | the same digest as a raw `buffer` |
| `hash.sha384_bytes(<value>)` | the same digest as a raw `buffer` |
| `hash.sha512_bytes(<value>)` | the same digest as a raw `buffer` |
| `hash.md5_file(<path>)`, async | MD5 of a file's contents, as lowercase hex |
| `hash.sha1_file(<path>)`, async | SHA-1 of a file's contents, as lowercase hex |
| `hash.sha224_file(<path>)`, async | SHA-224 of a file's contents, as lowercase hex |
| `hash.sha256_file(<path>)`, async | SHA-256 of a file's contents, as lowercase hex |
| `hash.sha384_file(<path>)`, async | SHA-384 of a file's contents, as lowercase hex |
| `hash.sha512_file(<path>)`, async | SHA-512 of a file's contents, as lowercase hex |
| `hash.<algo>_file_bytes(<path>)`, async | the same file digest as a raw `buffer` |

The `_file` forms are not available in `ied`; the value forms exist
in both programs.

## Example

```lua
hash.sha256("abc")
--> "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"

-- Verify a download without reading it into the script.
if hash.sha256_file("release.tar.gz") ~= expected then
    error("checksum mismatch")
end
```

## Conventions

**Six algorithms, four forms each.** Every algorithm is spelled the same
way: `<algo>` for hex, `<algo>_bytes` for the raw digest, `<algo>_file` for
a file read from disk (hex) and `<algo>_file_bytes` (raw). Learning one tells
you the rest.

**Hex is the default return.** A digest is nearly always wanted as text — a
cache key, a filename, a comparison against a published checksum — and the
raw bytes reaching Luau as a string would be arbitrary bytes that print as
mojibake. The output is **lowercase**, matching `sha256sum`, `shasum` and
`openssl dgst`. A published digest in uppercase must be lowered first:
`hash.sha256(x) == published:lower()`. The comparison is ordinary string
equality, not constant-time.
Use the `_bytes` form when the digest is an input to something else (an
HMAC by hand, a binary protocol); it returns a `buffer`, not a string, for
the same reason `proc.buffer()` does — it is honest about the content.

**Input is bytes, not text.** The value forms accept a **string or a
`buffer`**, and nothing is UTF-8 validated on the way in: a Luau string is
8-bit clean, so hashing arbitrary binary works and gives the same answer
the file form would. A buffer is accepted because that is what
`proc.buffer()` and `hash.*_bytes` hand back, so hashing a captured
command's output needs no conversion. Any other type (a number, a table)
raises `"expected a string or buffer to hash"` rather than being coerced —
`hash.sha256(42)` almost certainly meant `hash.sha256("42")`, and guessing
which would be a silent wrong answer.

**The `_file` forms stream.** They read in 64 KiB chunks and never
materialize the file in Luau, so hashing a multi-gigabyte archive costs one
small buffer rather than its own size in script memory. The read happens on
a blocking thread, so other [`task`](task.md)s keep running while it
proceeds — a `task.pool` hashing a directory of large files genuinely
overlaps the reads.

**File access.** `<algo>_file` names a file path
(relative to `-C`, or absolute). Not available in `ied`, which reads only
inside its work directory ([reproducibility](../binaries/ied.md#reproducibility)).
They are **absent** rather than stubbed, so
`if hash.sha256_file then` is a usable feature test; under `ied` a script
reads through [`fs`](fs.md) and hashes the result. `fs.read` returns a byte
string, so this works for binary files too:

```lua
local digest = hash.sha256(fs.read("config.yaml"))
```

**Choosing an algorithm.** `sha256` is the default worth reaching for.
`md5` and `sha1` are **broken for security purposes** — do not use either
to verify that a file has not been tampered with — but remain correct for
matching a checksum some other system already publishes in that format, and
for non-adversarial uses like cache keys and change detection.

## `hash.md5` / `hash.sha1` / `hash.sha224` / `hash.sha256` / `hash.sha384` / `hash.sha512`

The digest of a value's bytes, as lowercase hex.

**Signature:** `hash.sha256(value) -> string`

**Parameters:**

- `value` — the bytes to hash, as a **string** or a **`buffer`**. Not UTF-8
  validated; arbitrary binary is fine.

**Returns:** the digest as a lowercase hex string — 32 characters for MD5,
40 for SHA-1, 56 for SHA-224, 64 for SHA-256, 96 for SHA-384, 128 for SHA-512.

**Errors:** raises `"expected a string or buffer to hash, got <type>"` for
any other argument type. Hashing itself cannot fail.

```lua
hash.md5("abc")     --> "900150983cd24fb0d6963f7d28e17f72"
hash.sha1("abc")    --> "a9993e364706816aba3e25717850c26c9cd0d89d"
hash.sha256("")     --> "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"

-- A cache key over a command's output.
local out = proc.command({ "terraform", "show", "-json" })
    :stdout(proc.string()):run().stdout
local key = hash.sha256(out)
```

## `hash.<algo>_bytes`

The same digest, undecoded.

**Signature:** `hash.sha256_bytes(value) -> buffer`

**Parameters:**

- `value` — the bytes to hash, as a **string** or a **`buffer`**, exactly as
  for the hex forms.

**Returns:** a `buffer` holding the raw digest — 16 bytes for MD5, 20 for
SHA-1, 28 for SHA-224, 32 for SHA-256, 48 for SHA-384, 64 for SHA-512. Hex-encoding it yields exactly what
the corresponding hex form returns.

**Errors:** the same `"expected a string or buffer to hash"` as the hex
forms.

```lua
local d = hash.sha256_bytes("abc")
buffer.len(d)            --> 32
buffer.readu8(d, 0)      --> 186  (0xba)
```

## `hash.<algo>_file` / `hash.<algo>_file_bytes`

The digest of a file's contents, without reading it into the script.
Not available in `ied`.

**Signature:** `hash.sha256_file(path) -> string`, async

**Parameters:**

- `path` — the file to hash. Relative paths resolve against the `-C` working
  directory, as everywhere in [`fs`](fs.md). The path is taken as bytes, so a
  non-UTF-8 name works on unix.

**Returns:** the digest as a lowercase hex string (`_file_bytes`: a `buffer`),
identical to what `hash.sha256(fs.read(path))` gives for the same bytes.

**Errors:** raises `"hash <path>: <reason>"` (the wording `fs` errors use) when the file
cannot be opened or read — it does not exist, it is a directory, or
permissions deny it. There is no `nil` return: a script asking for a
checksum of a file it cannot read has no useful answer to continue with.

```lua
-- Compare a local file against a published checksum.
local want = "ba7816bf..."
assert(hash.sha256_file("release.tar.gz") == want, "checksum mismatch")

-- Hash a whole tree, concurrently -- the reads overlap.
local pool = task.pool(8)
for _, p in ipairs(fs.finder():ext("iso"):files():find()) do
    pool:spawn(function()
        return p, hash.sha256_file(p)
    end)
end
while not pool:done() do
    local path, digest = pool:await()
    print(digest, path)
end
```
