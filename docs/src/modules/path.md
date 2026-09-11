# `path` — path manipulation

Pure path manipulation, separate from the [`fs`](fs.md) module. These are
total string functions — no disk access, no sandbox resolution — so their
results feed straight back into `fs.` (mirrors the `std::fs` / `std::path`
split).

| Function | Effect |
| --- | --- |
| `path.join(parts)` / `path.join(a, b, ...)` | Join fragments into one path. |
| `path.product(a, b, ...)` | Cartesian product of the axes, each combination joined into a path. |
| `path.parent(path)` | Parent directory of `path` (`""` if none). |
| `path.file_name(path)` | Final path component (`""` if none). |
| `path.extension(path)` | Extension without the dot (`""` if none). |
| `path.stem(path)` | File name without its extension. |
| `path.split(path)` | `parent, file_name` in one call. |
| `path.with_extension(path, ext)` | `path` with its extension replaced (`""` removes it). |
| `path.with_file_name(path, name)` | `path` with its final component replaced. |
| `path.is_absolute(path)` | Whether `path` starts at the root. |
| `path.normalize(path)` | Lexically clean `.`/`..`/repeated slashes. |
| `path.relative_to(path, base[, opts])` | `path` expressed relative to `base`, or `nil` when it does not lie under it. |
| `path.capture(path, pattern)` | Match `path` against a named-segment pattern; table of captures, or `nil` on no match. |

## Example

Turn a directory listing into structured data without any string slicing:

```lua
for _, d in ipairs(fs.finder():path("AWS/**"):dirs():find()) do
    local m = path.capture(d, "AWS/{account}/{project}/resources")
    if m ~= nil then
        print(`{m.account} / {m.project}`)
    end
end
```

## Conventions

- **Purely lexical.** No disk access, no symlink resolution. Segment-based
  functions (`relative_to`, `capture`) split on `/`; leading, trailing,
  and repeated slashes and `.` segments are ignored, so `/a/b/`, `a/./b`
  and `a/b` have the same segments (`..` is an ordinary segment there; use
  `path.normalize` first to resolve it).
- **Arguments are strings exactly** — a number is not coerced — and must be
  valid UTF-8. Every error starts with `path.<function>:` and names the
  offending argument.
- **Batch overloads — avoid looping in Luau.** `parent`, `file_name`,
  `extension`, `stem`, `normalize`, `relative_to`, and `capture` each take
  an **array of paths** as well as a single path, mapping element-wise and
  returning an array of the same length. Arrays must be flat (elements are
  strings, not nested arrays). Where a single-path call would return `nil`
  (no match / not under the base), the batch form has **`false`** in that
  slot, so the result never has holes and `#out == #in`:

  ```lua
  path.parent({"/a/b", "/c/d.txt"})   -- {"/a", "/c"}
  path.stem({"/c/d.txt"})             -- {"d"}
  path.capture({"/AWS/1/web", "/x"}, "/AWS/{acct}/{proj}")
  -- {{acct = "1", proj = "web"}, false}   -- one entry per input
  ```

- **String-or-array arguments raise on the wrong shape.** Anywhere an
  argument is documented as "a string or an array of strings"
  (`join`/`product`'s axes, the subject of `relative_to`/`capture`, the
  batch form of the component extractors), a non-string array element or
  a value that is neither a string nor a table raises — there is no silent
  coercion.
- **Component edge cases** follow `std::path`:

  | Path | `parent` | `file_name` | `stem` | `extension` |
  | --- | --- | --- | --- | --- |
  | `/a/b.txt` | `/a` | `b.txt` | `b` | `txt` |
  | `a.tar.gz` | `""` | `a.tar.gz` | `a.tar` | `gz` |
  | `.bashrc` (dotfile) | `""` | `.bashrc` | `.bashrc` | `""` |
  | `foo.` | `""` | `foo.` | `foo` | `""` |
  | `a` | `""` | `a` | `a` | `""` |
  | `/a` | `/` | `a` | `a` | `""` |
  | `/` | `""` | `""` | `""` | `""` |
  | `..` | `""` | `""` | `""` | `""` |

## `path.join`

Flattens its arguments (each a string or an array of strings) in order
and joins them into a single path.

**Signature:** `path.join(parts: {string} | string, ...) -> string`

**Parameters:** any number of arguments, each either a string (one
segment) or an array of strings (multiple segments). All arguments are
flattened, in order, before joining.

**Returns:** the joined path as a string. An **empty segment is skipped**
(so `path.join("a", "")` is `"a"`, with no trailing slash), and an
**absolute segment replaces everything before it**
(`path.join("a", "/b", "c")` is `"/b/c"`), as `Path::push` does.

**Errors:** raises if any argument is neither a string nor an array, or
if an array contains a non-string element.

```lua
path.join({"/a", "b", "c"})     -- "/a/b/c"
path.join("/a", "b")            -- "/a/b"
path.join("/a", {"b", "c"})     -- "/a/b/c"
path.join("a", "/b", "c")       -- "/b/c"
```

## `path.product`

Takes two or more axes and joins every combination into a path.

**Signature:** `path.product(a: {string} | string, b: {string} | string, ...) -> {string}`

**Parameters:** two or more axes, each a string (one segment) or an array
of strings.

**Returns:** a flat array of joined paths, one per combination, in
left-major order (the first axis varies slowest). An empty axis yields an
empty result. The join rules are `path.join`'s.

**Errors:** raises under the same "string or array of strings" rule as
`join`, or with fewer than two axes.

```lua
path.product({"/a", "/b"}, {"x", "y"})  -- {"/a/x", "/a/y", "/b/x", "/b/y"}
path.product("/a", {"x", "y"}, "f")     -- {"/a/x/f", "/a/y/f"}
path.product({}, {"x"})                 -- {}
```

## `path.parent`

**Signature:** `path.parent(path: string) -> string`, or `path.parent(paths: {string}) -> {string}` element-wise.

**Parameters:** `path` — a path string, or an array of path strings.

**Returns:** the parent directory, or `""` when `path` has none (the root
`/`, or `..`). A single relative component's parent is `""`; `/a`'s is `/`.

```lua
path.parent("/a/b")             -- "/a"
path.parent("b")                -- ""
path.parent("/")                -- ""
path.parent({"/a/b", "/c/d.txt"}) -- {"/a", "/c"}
```

## `path.file_name`

**Signature:** `path.file_name(path: string) -> string`, or `path.file_name(paths: {string}) -> {string}` element-wise.

**Parameters:** `path` — a path string, or an array of path strings.

**Returns:** the final component (file or directory name), or `""` when
`path` has none (e.g. `/` or `..`).

```lua
path.file_name("/c/d.txt")      -- "d.txt"
path.file_name({"/a/b", "/c/d.txt"}) -- {"b", "d.txt"}
```

## `path.extension`

**Signature:** `path.extension(path: string) -> string`, or `path.extension(paths: {string}) -> {string}` element-wise.

**Parameters:** `path` — a path string, or an array of path strings.

**Returns:** the extension without its leading dot, or `""` when `path`
has none. Only the last extension counts (`a.tar.gz` → `gz`), a dotfile
has none (`.bashrc`), and a trailing dot gives `""` (`foo.`).

```lua
path.extension("/c/d.txt")      -- "txt"
path.extension({"/c/d.txt", "/x"}) -- {"txt", ""}
```

## `path.stem`

**Signature:** `path.stem(path: string) -> string`, or `path.stem(paths: {string}) -> {string}` element-wise.

**Parameters:** `path` — a path string, or an array of path strings.

**Returns:** the file name with its extension (if any) removed. `""`
when `path` has no file name.

```lua
path.stem("/c/d.txt")           -- "d"
path.stem("a.tar.gz")           -- "a.tar"
path.stem({"/c/d.txt"})         -- {"d"}
```

## `path.split`

**Signature:** `path.split(path: string) -> (string, string)`

**Parameters:** `path` — a path string.

**Returns:** `path.parent(path)` and `path.file_name(path)`.

```lua
local dir, name = path.split("/a/b.txt")  -- "/a", "b.txt"
```

## `path.with_extension`

**Signature:** `path.with_extension(path: string, ext: string) -> string`

**Parameters:** `path`; `ext` — the new extension, with or without the
leading dot. `""` removes the extension.

**Returns:** the new path (an extension is appended if there was none).

**Errors:** raises on a non-string argument.

```lua
path.with_extension("/a/b.txt", "md")   -- "/a/b.md"
path.with_extension("/a/b.txt", "")     -- "/a/b"
```

## `path.with_file_name`

**Signature:** `path.with_file_name(path: string, name: string) -> string`

**Parameters:** `path`; `name` — the new final component.

**Returns:** the new path.

**Errors:** raises on a non-string argument.

```lua
path.with_file_name("/a/b.txt", "c")    -- "/a/c"
```

## `path.is_absolute`

**Signature:** `path.is_absolute(path: string) -> boolean`

**Returns:** `true` if `path` starts at the root (`/`).

```lua
path.is_absolute("/a")   -- true
path.is_absolute("a")    -- false
```

## `path.normalize`

Lexically resolves `.`, `..` and repeated or trailing slashes.

**Signature:** `path.normalize(path: string) -> string`, or `path.normalize(paths: {string}) -> {string}` element-wise.

**Returns:** the cleaned path. A `..` cancels the segment before it; a
leading `..` of a relative path is kept, one at the root of an absolute
path is dropped; the empty result is `"."`. Symlinks are not consulted (it
can differ from `fs.canonicalize`).

```lua
path.normalize("a/./b/../c//d/")   -- "a/c/d"
path.normalize("../a")             -- "../a"
path.normalize("/../a")            -- "/a"
path.normalize("a/..")             -- "."
```

## `path.relative_to`

Returns `path` written relative to `base`, or `nil` when it does not lie
under it.

**Signature:** `path.relative_to(path: string, base: string, opts: {up: boolean}?) -> string | nil`, or `path.relative_to(paths: {string}, base: string, opts) -> {string | false}` element-wise.

**Parameters:**
- `path` (or `paths`) — the subject path, or array of subject paths.
- `base` — the base path each subject is tested against. Always a
  single string, even in the array form.
- `opts.up` — when `true`, a path outside `base` is reached through `..`
  segments instead of giving `nil` (`"/a/x"` against `"/a/b"` is `"../x"`).
  Default `false`.

**Returns:** the subject path with `base`'s segments stripped from the
front, joined with `/`. When `path == base`, returns `"."` — the one
form of the result that is still usable as a path. Returns `nil` (`false`
in the array form) when `path` does not lie under `base` and `up` is not
set. `.` segments are ignored on both sides (`relative_to("a/b", ".")` is
`"a/b"`), and an absolute path is never under a relative base or the
reverse.

**Errors:** raises on a non-string `base`, a non-string/non-table
subject, a non-string array element, or a non-boolean `opts.up`.

```lua
path.relative_to("/repo/src/main.rs", "/repo")  -- "src/main.rs"
path.relative_to("/repo/src/", "/repo/")        -- "src"    (trailing slashes ignored)
path.relative_to("/repo", "/repo")              -- "."      (usable as a path)
path.relative_to("/other/src", "/repo")         -- nil
path.relative_to("/repo-old/src", "/repo")      -- nil      (whole segments only)
path.relative_to("repo/src", "/repo")           -- nil      (relative is not under absolute)
path.relative_to("/a/x", "/a/b", {up = true})   -- "../x"
```

Segments are compared **whole**, which is what a hand-rolled
`string.sub` prefix strip gets wrong: `/repo-old` shares a string prefix
with `/repo` without being under it, and a trailing slash on either side
shifts every offset by one.

The common use is a containment test, where `nil` is the "outside"
branch:

```lua
local rel = path.relative_to(p, root)
if rel then print(`inside, at {rel}`) else print("outside the tree") end
```

## `path.capture`

Matches a path against a segment pattern and returns a table of the
captured segments, or `nil` when it does not match.

**Signature:** `path.capture(path: string, pattern: string) -> {[string]: string} | nil`, or `path.capture(paths: {string}, pattern: string) -> {({[string]: string} | false)}` element-wise.

**Parameters:**
- `path` (or `paths`) — the subject path, or array of subject paths.
- `pattern` — a `/`-separated pattern. Always a single string, even in
  the array form; when `paths` is an array, the pattern is parsed once
  and matched against every element. Literal segments must match
  exactly; `{...}` segments are placeholders:

  | Placeholder | Captures |
  | --- | --- |
  | `{name}` | exactly one segment (required) |
  | `{name?}` | zero or one segment |
  | `{name*}` | one or more segments, joined with `/` |
  | `{name*?}` | zero or more segments, joined with `/` |

**Returns:** a table mapping each placeholder's name to its captured
text, or `nil` (`false` in the array form) when `path` does not match
`pattern`. An optional capture that matched nothing — `{name?}` or
`{name*?}` — is **omitted** from the table, never `""`. Matching works on
segments, so a leading `/` (or its absence) on the path or the pattern is
not significant, unlike `relative_to`.

**Errors:**
- Raises at pattern-parse time for a malformed pattern: an empty capture
  name (`{}`, `{*}`, `{?}`, `{*?}`), a **duplicate** capture name, a
  segment mixing literal text with braces (`foo{bar}`), or two
  multi-segment captures (`{x*}`/`{x*?}`) directly adjacent (`{a*}/{b*}`),
  which have no boundary to split on.
- Returns `nil` (does not raise) for a well-formed pattern that simply
  does not match — including a segment-count mismatch.

A multi-segment capture is greedy but backs off until the rest of the
pattern matches, so `{dirs*}/{file}` splits a path into its directory part
and last segment, and `{a*}/{b?}` is fine.

```lua
path.capture("AWS/123/web/resources", "AWS/{account}/{project}/resources")
-- {account = "123", project = "web"}

path.capture("AWS/123/web/prod/resources/a/b", "AWS/{account}/{rest*}/resources/{tail*}")
-- {account = "123", rest = "web/prod", tail = "a/b"}

path.capture("a/b/c/d", "{dirs*}/{file}")
-- {dirs = "a/b/c", file = "d"}

path.capture("AWS/123", "AWS/{account}/{project}")
-- nil  — no match (segment count doesn't fit)
```
