# `fs` — files and directories

Two implementations share one Luau surface. `iq`/`ied` get a **rooted
VFS**: paths are root-relative (the `-C` work directory is the root),
`..`/symlink escapes are rejected, and writes are either denied (`iq`) or
buffered until the host flushes (`ied`). `ito`/`irun` get the **host
filesystem**: paths resolve exactly as `std::fs` would from the
already-`chdir`ed work directory, `..` and absolute paths reach outside
it, outward symlinks are followed, and writes land immediately. For any
path the rooted VFS would have accepted, the host filesystem names the
same file — escapes are the only behavioural delta. Where a function
exists only under one model, that is noted per-function; a script can
feature-test with `if fs.write then` / `if fs.cwd then` / `if
fs.finder().grep then`.

## Summary

### Read

| Function | Effect |
| --- | --- |
| `fs.list(dir)` | Sorted array of entries directly under `dir`. |
| `fs.walk([dir])` | Top-down tree walk, one entry per directory: `{path, dirs, files}`. |
| `fs.glob(pattern)` | Sorted array of regular files matching a glob, one shot. |
| `fs.finder([dir])` | Start a chainable `fd`-like search (see below). Host takes an optional start directory; rooted takes none. |
| `fs.read(path)` | File contents as a string, current disk state. |
| `fs.exists(path)` | Whether anything exists at `path`. Follows links. |
| `fs.is_file(path)` | Whether `path` is a regular file. Follows links. |
| `fs.is_dir(path)` | Whether `path` is a directory. Follows links. |
| `fs.is_symlink(path)` | Whether `path` is itself a symlink. Does **not** follow. |

### Write (see [Writes and removes](#writes-and-removes) for mode differences)

| Function | Effect | Availability |
| --- | --- | --- |
| `fs.write(path, content)` | Write a file, creating parents. | absent under `iq`; buffered under `ied`; immediate under `ito`/`irun` |
| `fs.remove(path)` | Delete a file; absent file is a no-op. | absent under `iq`; buffered under `ied`; immediate under `ito`/`irun` |
| `fs.write_new(path[, content])` | Atomic create-if-absent write; returns whether *this* call created it. | `ito`/`irun` only |
| `fs.mkdir(path)` | Create a directory and missing parents. | `ito`/`irun` only |
| `fs.rmdir(path)` | Remove a directory and everything under it. | `ito`/`irun` only |
| `fs.rename(from, to)` | Move a file or directory. | `ito`/`irun` only |
| `fs.copy(from, to)` | Copy a file or a whole tree. | `ito`/`irun` only |
| `fs.symlink(target, path)` | Create `path` as a symlink to `target`. | `ito`/`irun` only, Unix only |

### Host-only location/link functions

| Function | Effect |
| --- | --- |
| `fs.cwd()` | The work directory, absolute. |
| `fs.canonicalize(path)` | Absolute, symlink-resolved, `.`/`..`-collapsed form of `path`. |
| `fs.readlink(path)` | The symlink's target, verbatim, one hop, unresolved. |

### `fs.finder()` — builder methods (chainable, return the finder)

| Method | Effect |
| --- | --- |
| `:name(g, ...)` | Match the basename, any depth. |
| `:path(g, ...)` | Match the reported path (`*` stops at `/`, `**` spans). |
| `:ext(e, ...)` | Match extension, with or without the dot; `""` means no extension. |
| `:exclude(g, ...)` | Drop matches and prune matched directories from the walk. |
| `:files()` | Keep regular files. |
| `:dirs()` | Keep directories. |
| `:type(k, ...)` | Keep `"file"`, `"dir"` or `"symlink"`. |
| `:depth(n)` / `:depth(min, max)` | Exact depth, or an inclusive range. |
| `:min_depth(n)` / `:max_depth(n)` | Floor / ceiling on its own. |
| `:hidden()` | Include dotfiles/dot-directories. |
| `:no_ignore()` | Include entries a `.gitignore`/`.ignore` excludes. |
| `:unrestricted()` | Both `:hidden()` and `:no_ignore()`. |
| `:follow()` | Descend into symlinked directories (host only). |
| `:sort(k)` | Order by `"path"` (default), `"name"`, `"size"` or `"mtime"`. |
| `:limit(n)` | Keep at most `n` results (matches, not paths, for `:grep`/`:grep_files`). |

### `fs.finder()` — terminals (end the chain)

| Method | Result |
| --- | --- |
| `:find()` | Array of matching paths. |
| `:count()` | How many matched. |
| `:any()` | Whether anything matched (stops at the first hit). |
| `:grep(pattern)` | Array of matching lines with captures (`ito`/`irun` only). |
| `:grep_files(pattern)` | Array of paths whose contents match (`ito`/`irun` only). |

## Example

```lua
-- rewrite every TOML file's version pin, previewed by ied and applied by ito
for _, p in ipairs(fs.finder():ext("toml"):find()) do
  local content = fs.read(p)
  fs.write(p, content:gsub('version = "1%.0%.0"', 'version = "1.0.1"'))
end
```

## Conventions

**Rooted vs. host.** `iq`/`ied` see a VFS anchored at the `-C` root:
script-visible paths are root-relative with no leading `/` (a leading `/`
is accepted and treated the same as none), `..` that climbs above the
root is an error, and a symlink resolving outside the root is refused by
every read and dropped by every traversal — checked against the
*canonical* (resolved) location, so following a link can only surface
files already inside the root. `ito`/`irun` see the real filesystem: a
relative path resolves against the work directory because the process
cwd is already there (`-C` did `chdir`), an absolute path means what it
says, and `..` climbs out without error. There is no mapper and no
boundary on the host side — a program that can spawn `proc`/`ssh` is not
confined by its filesystem module anyway.

**Traversal output spelling.** Rooted traversals report paths relative to
the root (the root itself renders as `""`/`"."` for `fs.walk`'s starting
step). Host traversals report paths in the same form the *start
directory* was given: a relative start (default `"."`) gives
work-directory-relative results (`sub/b.txt`), an absolute start gives
absolute results, and `".."` gives `../b/y` — matching `find`/`fd`/
`walkdir`. This is decided once per traversal from the call site, not per
entry, so a glob anchored to the call's own spelling always matches the
whole result set.

**Write modes.** Whether `fs.write`/`fs.remove` exist, and how they reach
disk, is chosen by which binary runs the script:

- **`iq`** — denied. The functions are not registered at all; there is
  nothing to feature-test around except their absence.
- **`ied`** — buffered. Calls queue into a per-path pending slot (write
  and remove share one slot, so the *last* call to a path wins) and reach
  disk only if the host flushes after the script returns, which happens
  only under `-w`/`--write`. A script cannot read its own buffered
  writes — `fs.read` always reflects current disk state. There is no
  script-facing flush or clear.
- **`ito`/`irun`** — direct. Each call lands on disk immediately, so a
  script does read back its own writes. Nothing is held back, so there is
  no change set to preview.

Either way, a write creates missing parent directories, and removing an
absent path is a no-op rather than an error.

**Symlinks.** A link stands for what it names: a link to a file counts as
a file, a link to a directory is descended into (or copied, or matched)
as that directory, but is reported under its own name — a link and its
target are two entries, not one path listed twice. A link back up its own
ancestry does not create a cycle. The predicates follow this rule except
`fs.is_symlink`, which asks about the entry itself and is the only one
that is `true` for a dangling link. Under the rooted VFS every traversal
and read already resolves links (there being no `:follow()` there — it
could not mean anything else); under the host filesystem, `fs.finder`
does *not* descend into a symlinked directory by default — add
`:follow()` to opt in — while every other host traversal and read follows
links as above.

## `fs.list(dir)`

Entries (files and directories) directly under `dir`, non-recursive.

**Signature:** `fs.list(dir: string) -> {string}`

**Parameters:**
- `dir` — directory to list.

**Returns:** sorted array of entries. Rooted: root-relative. Host: in
`dir`'s own spelling (relative `dir` gives `dir/name`, absolute gives
absolute paths).

**Errors:** raises if `dir` cannot be read (missing, not a directory, no
permission); rooted also raises if `dir` escapes the root.

```lua
for _, name in ipairs(fs.list("src")) do print(name) end
```

## `fs.walk([dir])`

Top-down recursive walk, one entry per directory visited, parents before
children — the shape Python's `os.walk` established.

**Signature:** `fs.walk(dir: string?) -> {{path: string, dirs: {string}, files: {string}}}`

**Parameters:**
- `dir` — start directory. Defaults to the root (rooted) / work directory
  (host, `"."`).

**Returns:** array of steps. `step.dirs`/`step.files` are bare names, not
paths — join them onto `step.path` yourself (`path.join` handles the
rooted model's empty top-level path). Hidden entries are **included**;
`fs.walk` reports what a directory holds rather than filtering it.

**Errors:** an unreadable directory during the walk is silently skipped,
not fatal (matches `fs.finder`'s general "one bad entry does not fail the
survey" stance); a `dir` that cannot be resolved at all (rooted escape,
or simply missing) raises or returns an empty result depending on where
the failure occurs — the start directory itself failing to read yields a
walk of zero steps rather than an error.

```lua
for _, step in ipairs(fs.walk("src")) do
  for _, name in ipairs(step.files) do
    print(path.join(step.path, name))
  end
end
```

## `fs.glob(pattern)`

One-shot glob match against regular files, full [`globset`](https://docs.rs/globset)
syntax including brace expansion.

**Signature:** `fs.glob(pattern: string) -> {string}`

**Parameters:**
- `pattern` — glob matched against the full path. Anchor with a leading
  `**` to search at any depth (`"**/*.toml"`).

**Returns:** sorted array of matching regular files. Rooted:
root-relative. Host: relative pattern gives work-directory-relative
results, absolute pattern gives absolute results.

**Errors:** raises on an invalid glob pattern.

Hidden files and dot-directories (`.git`, `.terraform`) are always
excluded — there is no opt-in on `fs.glob` itself; use
`fs.finder():hidden()` for that. `fs.glob` does not consult
`.gitignore`/`.ignore` (unlike `fs.finder`), so it walks everything except
hidden entries.

```lua
fs.glob("**/*.toml")            -- every .toml file, anywhere
fs.glob("src/**/*.{rs,toml}")   -- .rs or .toml under src
```

## `fs.finder([dir])`

Start a chainable, `fd`-like search.

**Signature:** `fs.finder(dir: string?) -> Finder` (host); `fs.finder() -> Finder` (rooted)

**Parameters:**
- `dir` — start directory (host only; default `"."`, the work directory).
  The rooted finder has no argument — the root is the only place a rooted
  search can begin.

**Returns:** a `Finder` handle. There is no seed pattern: an unfiltered
finder means "everything under here," and each condition is added by a
named method.

**Default scope:** `.gitignore`/`.ignore` rules are respected and hidden
entries are skipped — and pruned, not just filtered, so an ignored
directory costs one `stat`, not its whole contents. Widen a level at a
time with `:hidden()`/`:no_ignore()`/`:unrestricted()`. Ignore files are
honoured even outside a git repository.

```lua
fs.finder():ext("rs"):find()          -- every .rs file
fs.finder("src"):files():find()       -- everything under src/ (host)
```

### `:name(g, ...)`

Match the entry's basename, at any depth. Never matches a path.

**Signature:** `finder:name(...glob: string) -> Finder`

**Parameters:** one or more glob patterns; alternatives within one call OR,
separate calls AND.

**Returns:** the same finder (chainable).

**Errors:** raises on an invalid or empty glob pattern.

```lua
fs.finder():name("*.rs", "*.toml"):find()   -- .rs OR .toml, by basename
```

### `:path(g, ...)`

Match the path as the finder reports it. `*` stops at `/`; use `**` to
span depths.

**Signature:** `finder:path(...glob: string) -> Finder`

**Parameters:** one or more glob patterns, OR within a call, AND across
calls.

**Returns:** the same finder.

**Errors:** raises on an invalid or empty pattern.

```lua
fs.finder():path("**/AWS/**/*.tf"):find()
```

### `:ext(e, ...)`

Match by extension, with or without the leading dot.

**Signature:** `finder:ext(...ext: string) -> Finder`

**Parameters:** one or more extensions. `""` is a real alternative meaning
"no extension" (judged by `Path::extension`, matching `path.extension`) —
`Makefile`/`README` match, `odd.` (empty-but-present extension) does not.
There is no glob spelling for "no extension," which is why `""` exists as
its own case rather than a pattern.

**Returns:** the same finder.

**Errors:** raises on an invalid pattern (a non-empty extension is turned
into a `*.ext` glob internally).

```lua
fs.finder():ext("rs", "toml"):find()   -- .rs OR .toml
fs.finder():ext(""):find()             -- Makefile, README, ...
fs.finder():ext("log", ""):find()      -- .log OR no extension
```

### `:exclude(g, ...)`

Drop matches, pruning a matched directory's whole subtree from the walk
rather than filtering its contents after the fact.

**Signature:** `finder:exclude(...glob: string) -> Finder`

**Parameters:** one or more patterns. Target auto-detects per pattern:
one containing `/` matches the path, otherwise the basename — the one
filter where both readings are routinely wanted (`"node_modules"` names a
directory by name; `"**/target/**"` names a region by path).

**Returns:** the same finder.

**Errors:** raises on an invalid pattern.

```lua
fs.finder():path("**/*.tf"):exclude(".terraform"):find()
```

### `:files()` / `:dirs()` / `:type(k, ...)`

Keep only entries of the given kind(s).

**Signature:** `finder:files() -> Finder`, `finder:dirs() -> Finder`,
`finder:type(...kind: string) -> Finder`

**Parameters:** `:type` takes one or more of `"file"`/`"f"`,
`"dir"`/`"directory"`/`"d"`, `"symlink"`/`"link"`/`"l"`. Repeating any of
these three widens the kept kinds (`:files():dirs()` keeps either); there
is no way to narrow once widened within one finder.

**Returns:** the same finder.

**Errors:** `:type` raises on an unrecognized kind string.

A link counts as what it points at for `:files()`/`:dirs()` (a link to a
file survives `:files()`), but is still reported under its own name;
`:type("symlink")` is how a script asks for links as entries in their own
right.

```lua
fs.finder():dirs():name("node_modules"):find()
```

### `:depth(n)` / `:depth(min, max)`

Bound how far below the start directory an entry may be. Direct children
of the start are depth 1.

**Signature:** `finder:depth(n: number) -> Finder`, `finder:depth(min: number, max: number) -> Finder`

**Parameters:** `n` — exact depth (`fd --exact-depth`); or `min`, `max` —
inclusive range. Both must be non-negative integers.

**Returns:** the same finder.

**Errors:** raises if either bound is negative.

```lua
fs.finder():ext("rs", "toml"):files():depth(2, 3):find()
```

### `:min_depth(n)` / `:max_depth(n)`

Set only the floor or only the ceiling, independent of the other.

**Signature:** `finder:min_depth(n: number) -> Finder`, `finder:max_depth(n: number) -> Finder`

**Parameters:** `n` — non-negative integer bound.

**Returns:** the same finder.

**Errors:** raises if `n` is negative.

`:max_depth` prunes the walk (the descent stops there, so it is cheap);
`:min_depth` filters per entry, since a shallow entry must still be
walked through to reach the deep ones.

### `:hidden()` / `:no_ignore()` / `:unrestricted()`

Widen the default scope by one level each, named for what they add (`fd`'s
`-H`/`-I`/`-u`).

**Signature:** `finder:hidden() -> Finder`, `finder:no_ignore() -> Finder`, `finder:unrestricted() -> Finder`

**Parameters:** none.

**Returns:** the same finder.

| Method | Adds |
| --- | --- |
| `:hidden()` | dotfiles and dot-directories |
| `:no_ignore()` | entries a `.gitignore`/`.ignore` excluded |
| `:unrestricted()` | both |

### `:follow()`

Descend into symlinked directories during the walk. Host only.

**Signature:** `finder:follow() -> Finder`

**Parameters:** none.

**Returns:** the same finder.

**Availability:** absent on the rooted finder — links there are already
followed unconditionally (containment is checked against the resolved
location, so following more can only surface files already inside the
root), and `:follow()` could not mean anything else there.

```lua
fs.finder("/srv"):follow():files():find()
```

### `:sort(key)`

Order results by something other than path.

**Signature:** `finder:sort(key: string) -> Finder`

**Parameters:** `key` — one of `"path"` (default), `"name"`, `"size"`, or
`"mtime"`/`"modified"` (newest first — the one key that sorts descending;
every other key sorts ascending). Ties break by path.

**Returns:** the same finder.

**Errors:** raises on an unrecognized key.

```lua
fs.finder("/var/log"):ext("log"):sort("mtime"):limit(10):find()
```

### `:limit(n)`

Cap the number of results, applied after sorting for `:find()`/`:count()`/
`:any()`.

**Signature:** `finder:limit(n: number) -> Finder`

**Parameters:** `n` — non-negative maximum.

**Returns:** the same finder.

**Errors:** raises if `n` is negative.

For `:find()`/`:count()` a limit bounds the number of **paths**. For
`:grep()`/`:grep_files()` it bounds the number of **matches** instead (a
match *is* a file for `:grep_files`, so the two readings coincide there):
the search itself runs unlimited over candidate files but stops reading
as soon as the limit is reached, so the tail of a large tree is never
opened. `:sort()` composes with it, deciding which files are reached
first.

```lua
fs.finder():ext("rs"):limit(10):find()               -- first 10 paths
fs.finder():ext("rs"):limit(10):grep("TODO")         -- first 10 matching lines
```

### `:find()`

Run the search and return every match.

**Signature:** `finder:find() -> {string}`

**Returns:** array of matching paths, sorted (by path unless `:sort()`
says otherwise). There is no `:first()`: `:find()` is sorted, so
`:find()[1]` is already the predictable first match.

```lua
for _, p in ipairs(fs.finder():ext("rs"):find()) do print(p) end
```

### `:count()`

How many entries match, without building the array of paths.

**Signature:** `finder:count() -> number`

**Returns:** count of matches. A `:limit(n)` in the chain caps this too,
so `:limit(10):count()` answers "up to ten."

```lua
print(fs.finder():ext("rs"):count())
```

### `:any()`

Whether anything matches, stopping at the first hit.

**Signature:** `finder:any() -> boolean`

**Returns:** `true` if at least one entry matches. This is the one
terminal that genuinely short-circuits the walk.

```lua
if fs.finder():ext("rs"):any() then ... end
```

### `:grep(pattern)`

Search the contents of every matched file for `pattern`, returning one
record per matching line. Host only (`ito`/`irun`).

**Signature:** `finder:grep(pattern: string) -> {{path: string, line: number, text: string, captures: table}}`

**Parameters:**
- `pattern` — a regular expression in the same dialect [`re`](re.md)
  takes (built on `regex-syntax` via `grep-regex`, so the meaning agrees
  with `re`).

**Returns:** flat array, one entry per matching line, in walk order (then
sort order if `:sort()` was set):

| Field | Value |
| --- | --- |
| `path` | the file, in the same spelling as every other finder result |
| `line` | 1-based line number |
| `text` | the line's text, trailing newline stripped |
| `captures` | capture groups from a second regex pass over the matched line: `[1]..[n]` positionally plus named groups by name; always present (empty table when the pattern has no groups); group 0 (the whole match) is omitted since `text` already carries it |

**Errors:** raises if `pattern` is not a valid regex. Does **not** raise
for an unreadable file or a non-regular-file hit encountered during the
search — those are skipped, since the search is a survey over a tree,
not a promise that every file is readable. Binary files are detected
(first NUL byte) and abandoned, so a match is always a line of text.

**Availability:** absent under the rooted finder (`iq`/`ied`) — it is the
only terminal that *opens* a matched file's contents, which the rooted
model does not yet extend containment to (containment governs which
*paths* may be named, not which *bytes* may be read). Terminal rather
than chainable: it is the only condition that costs a file read where
every other clause costs a `stat`, and a chained `:grep(a):grep(b)` would
have to answer whether that ANDs or ORs the way filter clauses do.

```lua
for _, m in ipairs(fs.finder("src"):ext("rs"):grep([[fn (\w+)]])) do
  print(m.path .. ":" .. m.line .. " " .. m.captures[1])
end
```

### `:grep_files(pattern)`

The paths whose contents match `pattern` — the cheaper question, each
file read only as far as its first hit. Host only (`ito`/`irun`).

**Signature:** `finder:grep_files(pattern: string) -> {string}`

**Parameters:**
- `pattern` — same regex dialect as `:grep`.

**Returns:** array of matching paths (no line/capture detail — a script
wanting that uses `:grep` instead).

**Errors:** same as `:grep`: raises on an invalid pattern, skips unreadable
or non-regular files.

**Availability:** same as `:grep` (host only, terminal).

```lua
for _, p in ipairs(fs.finder():ext("yml"):grep_files("image:")) do
  print(p)
end
```

## `fs.read(path)`

Read a file's contents. Always reflects current disk state, never a
pending buffered write.

**Signature:** `fs.read(path: string) -> string`

**Parameters:** `path` — file to read.

**Returns:** file contents as a string.

**Errors:** raises if the file cannot be read (missing, permission,
not UTF-8-losslessly-representable is not itself an error — reading is
lossy-safe at the OS layer but `std::fs::read_to_string` does require
valid UTF-8 and raises otherwise). Rooted also raises on an escape.

```lua
local content = fs.read("config.toml")
```

## `fs.write(path, content)`

Write `content` to `path`, creating missing parent directories.

**Signature:** `fs.write(path: string, content: string)`

**Parameters:**
- `path` — file to write.
- `content` — full contents to write (not appended).

**Returns:** nothing.

**Errors:** absent entirely under `iq` (not a raise — the function does
not exist). Under `ied`, queues the write; nothing on disk changes during
the call, and a later `fs.write`/`fs.remove` to the same path replaces
it (last call wins) — actually reaching disk (or not) depends on the
host's `-w`/`--write` flag after the script returns. Under `ito`/`irun`,
writes immediately; raises on an I/O error (e.g. parent creation fails,
permission denied). Rooted also raises on an escape.

```lua
fs.write("out/report.txt", "done\n")
```

## `fs.remove(path)`

Delete a file. Removing an absent path is a no-op, not an error.

**Signature:** `fs.remove(path: string)`

**Parameters:** `path` — file to delete.

**Returns:** nothing.

**Errors:** absent under `iq`. Buffers under `ied` (shares the per-path
slot with `fs.write`; last call wins). Removes immediately under
`ito`/`irun`; raises only on a real I/O error (not "file already gone").
Rooted also raises on an escape.

```lua
fs.remove("stale.tmp")
```

## `fs.exists(path)`

Whether anything exists at `path`, current disk state. Follows symlinks,
so a dangling link reports `false`.

**Signature:** `fs.exists(path: string) -> boolean`

**Parameters:** `path` — path to test.

**Returns:** `true`/`false`.

**Errors:** rooted raises if `path` escapes the root (an out-of-root path
is a resolve error, not a silent `false`). Host never raises — a
non-existent path simply reports `false`.

```lua
if not fs.exists("out") then fs.mkdir("out") end
```

## `fs.is_file(path)`

Whether `path` is a regular file. Follows symlinks: a link to a file
reports `true`.

**Signature:** `fs.is_file(path: string) -> boolean`

**Parameters:** `path` — path to test.

**Returns:** `true`/`false` (`false` for anything unreadable, missing, or
not a file — this predicate never raises for those cases).

**Errors:** rooted raises on an escape.

```lua
fs.is_file(p) and not fs.is_symlink(p)   -- a real file, not a link to one
```

## `fs.is_dir(path)`

Whether `path` is a directory. Follows symlinks.

**Signature:** `fs.is_dir(path: string) -> boolean`

**Parameters:** `path` — path to test.

**Returns:** `true`/`false`.

**Errors:** rooted raises on an escape.

```lua
if fs.is_dir(p) then ... end
```

## `fs.is_symlink(path)`

Whether `path` is itself a symlink, whatever it points at. The one
predicate that does **not** follow — `true` even for a dangling link,
which is the one case where `is_symlink` is true but `exists` is false.

**Signature:** `fs.is_symlink(path: string) -> boolean`

**Parameters:** `path` — path to test.

**Returns:** `true`/`false`.

**Errors:** rooted raises if the entry itself resolves outside the root
(the escape check runs on the canonical location before this answers, so
an outward link raises rather than reporting `true`).

```lua
fs.is_symlink(p) and not fs.exists(p)   -- a dangling link
```

## `fs.write_new(path[, content])`

Create `path` only if it does not already exist, atomically
(`O_CREAT|O_EXCL`, one syscall) — the lock-acquire primitive, with
`fs.remove` as the release. Host only.

**Signature:** `fs.write_new(path: string, content: string?) -> boolean`

**Parameters:**
- `path` — file to create.
- `content` — contents to write if this call creates it. Defaults to
  empty.

**Returns:** `true` if *this* call created the file, `false` if it
already existed as a regular file. Losing the race is the return value,
not an error — it is the question being asked.

**Errors:** raises if a parent directory cannot be created, or if
`path` already exists but is **not** a regular file (a directory,
socket, or dangling symlink) — that is a name `fs.remove` can never free,
so reporting `false` would send a waiting caller into a loop with no
exit. Also raises on any other I/O error (e.g. permission denied).

**Availability:** host only (`ito`/`irun`) — "created now, atomically" is
exactly what a buffered write cannot express, since the answer must
arrive before the script continues.

```lua
if fs.write_new("/tmp/deploy.lock", os.getenv("USER") or "") then
  local ok, err = pcall(deploy)
  fs.remove("/tmp/deploy.lock")
  if not ok then error(err) end
else
  error(`already locked by {fs.read("/tmp/deploy.lock")}`)
end
```

There is deliberately no lock *object* — no handle, no
release-on-garbage-collection, no built-in wait/timeout, no staleness
rule; wait with a loop over `task.wait`, and record ownership in
`content` if that matters.

## `fs.mkdir(path)`

Create a directory and any missing parents. Host only.

**Signature:** `fs.mkdir(path: string)`

**Parameters:** `path` — directory to create.

**Returns:** nothing.

**Errors:** an already-existing directory is success, not an error.
Raises on a real I/O failure (e.g. a file already occupies part of the
path).

**Availability:** host only — a buffered run has no honest way to report
"this directory will exist."

```lua
fs.mkdir("out/reports")
```

## `fs.rmdir(path)`

Remove a directory and everything under it. Host only.

**Signature:** `fs.rmdir(path: string)`

**Parameters:** `path` — directory to remove.

**Returns:** nothing.

**Errors:** an absent path is a no-op, matching `fs.remove`. Raises on a
real I/O failure.

**Availability:** host only.

```lua
fs.rmdir("out/stale")
```

## `fs.rename(from, to)`

Move a file or directory. Parents of `to` are created, as `fs.write`
does. Host only.

**Signature:** `fs.rename(from: string, to: string)`

**Parameters:**
- `from` — existing path.
- `to` — destination path.

**Returns:** nothing.

**Errors:** raises on an I/O failure (missing `from`, permission, `to`'s
parent cannot be created).

**Availability:** host only — no previewable form exists for a buffered
run.

```lua
fs.rename("draft.txt", "final/draft.txt")
```

## `fs.copy(from, to)`

Copy a file or a whole directory tree. Parents of `to` are created. Host
only.

**Signature:** `fs.copy(from: string, to: string)`

**Parameters:**
- `from` — source file or directory.
- `to` — destination path.

**Returns:** nothing.

**Errors:** raises if `to` is inside `from` (copying a directory into
itself would never terminate — the destination keeps growing inside the
source). Raises on other I/O failures. A symlink in the source tree
copies as what it stands for (a link to a directory copies that
directory's tree, including one pointing outside the work directory);
copying merges into a `to` that already exists rather than replacing it.

**Availability:** host only.

```lua
fs.copy("templates/base", "out/site")
```

## `fs.cwd()`

The work directory, absolute. Host only.

**Signature:** `fs.cwd() -> string`

**Returns:** absolute path of the `-C`-resolved work directory — the
frame every relative path here resolves against.

**Errors:** raises only if the current directory cannot be read (rare —
e.g. it was deleted out from under the process).

**Availability:** host only — under the rooted VFS the root is a
boundary, not a location a script should learn; `if fs.cwd then` is a
usable feature test. It is the one path with no relative spelling
(traversals would render it `"."`, which a script asking for it already
has).

```lua
print(fs.cwd())                    -- /home/you/proj
local here = path.base(fs.cwd())   -- "proj"
```

## `fs.canonicalize(path)`

The absolute, symlink-resolved, `.`/`..`-collapsed form of `path`. Host
only.

**Signature:** `fs.canonicalize(path: string) -> string`

**Parameters:** `path` — path to resolve. May be relative.

**Returns:** absolute string form, links resolved.

**Errors:** raises if `path` does not exist — resolving a symlink means
reading it, so there is nothing to answer for a path that is not there.

**Availability:** host only, for the same reason `fs.cwd` is — the
counterpart that makes any path a script holds directly comparable to
`fs.cwd()`.

```lua
fs.canonicalize("./sub/../sub/a.txt")   -- /home/you/proj/sub/a.txt
fs.canonicalize("link.txt")             -- /home/you/proj/real.txt

local rel = path.relative_to(fs.canonicalize(p), fs.cwd())
if rel then print(`inside, at {rel}`) else print("outside") end
```

## `fs.readlink(path)`

The symlink's target, verbatim, one hop, unresolved. Host only.

**Signature:** `fs.readlink(path: string) -> string`

**Parameters:** `path` — a symlink.

**Returns:** the target exactly as written: relative targets stay
relative (to the link's own directory, not the work directory), and a
dangling target is still readable.

**Errors:** raises if `path` is not a symlink.

**Availability:** host only. `fs.canonicalize` is the opposite
question — every hop followed, absolute answer, error if missing.

```lua
fs.readlink("current")   -- e.g. "releases/v3"
```

## `fs.symlink(target, path)`

Create `path` as a symlink to `target`. Host only, Unix only.

**Signature:** `fs.symlink(target: string, path: string)`

**Parameters:**
- `target` — what the link points to, stored verbatim and never
  resolved (may be relative, and need not exist yet — a dangling link is
  a legitimate thing to create).
- `path` — where to create the link. Argument order matches `ln -s` (and
  `fs.copy`/`fs.rename`: source first).

**Returns:** nothing.

**Errors:** raises on an I/O failure (e.g. `path` already exists).
Parents of `path` are created first, as `fs.write` does.

**Availability:** host only; absent (not raising) on non-Unix platforms,
so `if fs.symlink then` feature-tests it — `std::fs` has no portable
symlink, and the Windows form needs a file-vs-directory distinction this
signature doesn't carry plus a privilege the process may not hold.

```lua
fs.symlink("releases/v3", "current")
```
