# `path` — path manipulation

Pure path manipulation, separate from the [`fs`](fs.md) module. These are
total string functions — no disk access, no sandbox resolution — so their
results feed straight back into `fs.` (mirrors the `std::fs` / `std::path`
split).

| Function | Effect |
| --- | --- |
| `path.join(parts)` / `path.join(a, b, ...)` | Join fragments into one path. |
| `path.product(a, b)` | Cartesian product of two axes, each pair joined into a path. |
| `path.parent(path)` | Parent directory of `path` (`""` if none). |
| `path.file_name(path)` | Final path component (`""` if none). |
| `path.extension(path)` | Extension without the dot (`""` if none). |
| `path.stem(path)` | File name without its extension. |
| `path.relative_to(path, base)` | `path` expressed relative to `base`, or `nil` when it does not lie under it. |
| `path.capture(path, pattern)` | Match `path` against a named-segment pattern; table of captures, or `nil` on no match. |

## Motivating example

Turn a directory listing into structured data without any string slicing:

```lua
for _, d in ipairs(fs.finder():path("AWS/**"):dirs():find()) do
    local m = path.capture(d, "AWS/{account}/{project}/resources")
    if m ~= nil then
        print(`{m.account} / {m.project}`)
    end
end
```

## General rules

- **Purely lexical.** No disk access, no symlink resolution, no
  normalization beyond what `std::path` does on join. Segments come from
  splitting on `/`; leading, trailing, and repeated slashes are ignored,
  so `/a/b/` and `a/b` have the same segments.
- **Batch overloads — avoid looping in Luau.** `parent`, `file_name`,
  `extension`, `stem`, `relative_to`, and `capture` each take an **array
  of paths** as well as a single path, mapping element-wise and returning
  an array of the same length. Arrays must be flat (elements are strings,
  not nested arrays):

  ```lua
  path.parent({"/a/b", "/c/d.txt"})   -- {"/a", "/c"}
  path.stem({"/c/d.txt"})             -- {"d"}
  path.capture({"/AWS/1/web", "/x"}, "/AWS/{acct}/{proj}")
  -- {{acct = "1", proj = "web"}, nil}   -- one entry per input, nil on no match
  ```

- **String-or-array arguments raise on the wrong shape.** Anywhere an
  argument is documented as "a string or an array of strings"
  (`join`/`product`'s axes, the subject of `relative_to`/`capture`, the
  batch form of the component extractors), a non-string array element or
  a value that is neither a string nor a table raises a runtime error —
  there is no silent coercion.

## `path.join` — concatenate fragments

Flattens its arguments (each a string or an array of strings) in order
and joins them into a single path.

**Signature:** `path.join(parts: {string} | string, ...) -> string`

**Parameters:** any number of arguments, each either a string (one
segment) or an array of strings (multiple segments). All arguments are
flattened, in order, before joining.

**Returns:** the joined path as a string.

**Errors:** raises if any argument is neither a string nor an array, or
if an array contains a non-string element.

```lua
path.join({"/a", "b", "c"})     -- "/a/b/c"
path.join("/a", "b")            -- "/a/b"
path.join("/a", {"b", "c"})     -- "/a/b/c"
path.join({"/a", "b"}, "c")     -- "/a/b/c"
path.join({"/a", "b"}, {"c"})   -- "/a/b/c"
```

## `path.product` — cartesian product of paths

Takes two axes and joins every left×right combination into a path.

**Signature:** `path.product(a: {string} | string, b: {string} | string) -> {string}`

**Parameters:**
- `a` — left axis: a string (one segment) or an array of strings.
- `b` — right axis: a string (one segment) or an array of strings.

**Returns:** a flat array of joined paths, one per combination, in
left-major order (`a[1]` against every `b`, then `a[2]` against every
`b`, ...). An empty axis yields an empty result.

**Errors:** raises under the same "string or array of strings" rule as
`join`.

```lua
path.product({"/a", "/b"}, {"x", "y"})  -- {"/a/x", "/a/y", "/b/x", "/b/y"}
path.product("/a", {"x", "y"})          -- {"/a/x", "/a/y"}
path.product({}, {"x"})                 -- {}
```

## `path.parent` — parent directory

**Signature:** `path.parent(path: string) -> string`, or `path.parent(paths: {string}) -> {string}` element-wise.

**Parameters:** `path` — a path string, or an array of path strings.

**Returns:** the parent directory, or `""` when `path` has none (e.g. a
single component or the root).

```lua
path.parent("/a/b")             -- "/a"
path.parent("b")                -- ""
path.parent({"/a/b", "/c/d.txt"}) -- {"/a", "/c"}
```

## `path.file_name` — final component

**Signature:** `path.file_name(path: string) -> string`, or `path.file_name(paths: {string}) -> {string}` element-wise.

**Parameters:** `path` — a path string, or an array of path strings.

**Returns:** the final component (file or directory name), or `""` when
`path` has none (e.g. `/`).

```lua
path.file_name("/c/d.txt")      -- "d.txt"
path.file_name({"/a/b", "/c/d.txt"}) -- {"b", "d.txt"}
```

## `path.extension` — extension without the dot

**Signature:** `path.extension(path: string) -> string`, or `path.extension(paths: {string}) -> {string}` element-wise.

**Parameters:** `path` — a path string, or an array of path strings.

**Returns:** the extension without its leading dot, or `""` when `path`
has none.

```lua
path.extension("/c/d.txt")      -- "txt"
path.extension({"/c/d.txt", "/x"}) -- {"txt", ""}
```

## `path.stem` — file name without extension

**Signature:** `path.stem(path: string) -> string`, or `path.stem(paths: {string}) -> {string}` element-wise.

**Parameters:** `path` — a path string, or an array of path strings.

**Returns:** the file name with its extension (if any) removed. `""`
when `path` has no file name.

```lua
path.stem("/c/d.txt")           -- "d"
path.stem({"/c/d.txt"})         -- {"d"}
```

## `path.relative_to` — express a path against a base

Returns `path` written relative to `base`, or `nil` when it does not lie
under it.

**Signature:** `path.relative_to(path: string, base: string) -> string | nil`, or `path.relative_to(paths: {string}, base: string) -> {string | nil}` element-wise.

**Parameters:**
- `path` (or `paths`) — the subject path, or array of subject paths.
- `base` — the base path each subject is tested against. Always a
  single string, even in the array form.

**Returns:** the subject path with `base`'s segments stripped from the
front, joined with `/`. When `path == base`, returns `"."` — the one
form of the result that is still usable as a path, matching what the
traversals report for the directory they started in. Returns `nil` when
`path` does not lie under `base`.

**Errors:** none of its own; the subject/array-shape check follows the
general string-or-array rule above (raises on a non-string, non-table
subject, or a non-string array element).

```lua
path.relative_to("/repo/src/main.rs", "/repo")  -- "src/main.rs"
path.relative_to("/repo/src/", "/repo/")        -- "src"    (trailing slashes ignored)
path.relative_to("/repo", "/repo")              -- "."      (usable as a path)
path.relative_to("/other/src", "/repo")         -- nil
path.relative_to("/repo-old/src", "/repo")      -- nil      (whole segments only)
path.relative_to("repo/src", "/repo")           -- nil      (relative is not under absolute)
```

Segments are compared **whole**, which is what a hand-rolled
`string.sub` prefix strip gets wrong: `/repo-old` shares a string prefix
with `/repo` without being under it, and a trailing slash on either side
shifts every offset by one. An absolute path is likewise never under a
relative base, or the reverse — those name the same file only by
coincidence of the working directory.

The common use is a containment test, where `nil` is the "outside"
branch:

```lua
local rel = path.relative_to(p, root)
if rel then print(`inside, at {rel}`) else print("outside the tree") end
```

## `path.capture` — named path segments

Matches a path against a segment pattern and returns a table of the
captured segments, or `nil` when it does not match.

**Signature:** `path.capture(path: string, pattern: string) -> {[string]: string} | nil`, or `path.capture(paths: {string}, pattern: string) -> {({[string]: string} | nil)}` element-wise.

**Parameters:**
- `path` (or `paths`) — the subject path, or array of subject paths.
- `pattern` — a `/`-separated pattern. Always a single string, even in
  the array form; when `paths` is an array, the pattern is parsed once
  and matched against every element. Literal segments must match
  exactly; `{...}` segments are placeholders:

  | Placeholder | Captures |
  | --- | --- |
  | `{name}` | exactly one segment (required) |
  | `{name?}` | zero or one segment (absent ⇒ key omitted from the result) |
  | `{name*}` | one or more segments, joined with `/` |
  | `{name*?}` | zero or more segments, joined with `/` (zero ⇒ `""`) |

**Returns:** a table mapping each placeholder's name to its captured
text, or `nil` when `path` does not match `pattern`. An absent `{name?}`
is omitted from the table rather than mapped to `""` or `nil`.

**Errors:**
- Raises at pattern-parse time for a malformed pattern: an empty capture
  name (`{}`), a segment mixing literal text with braces (`foo{bar}`),
  or a variable-width capture (`{x*}`/`{x*?}`) not followed by a literal
  segment or the end of the pattern — two adjacent variable-width
  captures are ambiguous and rejected (e.g. `{a*}/{b*}`, `{a*}/{b?}`,
  `{a*}/{b}` all raise).
- Returns `nil` (does not raise) for a well-formed pattern that simply
  does not match — including a segment-count mismatch (`{name}` requires
  exactly one segment; `{name*}` requires at least one).

A variable-width capture is greedy but backs off until the remainder of
the pattern matches, since the parser guarantees it is always bounded by
a literal or the end of input.

```lua
path.capture("AWS/123/web/resources", "AWS/{account}/{project}/resources")
-- {account = "123", project = "web"}

path.capture("AWS/123/web/prod/resources/a/b", "AWS/{account}/{rest*}/resources/{tail*}")
-- {account = "123", rest = "web/prod", tail = "a/b"}

path.capture("AWS/123", "AWS/{account}/{project}")
-- nil  — no match (segment count doesn't fit)

for _, d in ipairs(fs.finder():path("AWS/**"):dirs():find()) do
    local m = path.capture(d, "AWS/{account}/{project}/resources")
    if m ~= nil then print(`{m.account} / {m.project}`) end
end
```
