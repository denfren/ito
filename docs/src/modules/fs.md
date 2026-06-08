# `fs` — virtual filesystem

A VFS anchored at the working directory (`-C`). Scripts address files in
an absolute namespace rooted at `/`: a real file `<root>/subdir/x.txt`
is `/subdir/x.txt`. Relative paths (`subdir/x.txt`) are also accepted
and resolved against the root. A `..` traversal that escapes the root is
an error.

| Function | Effect |
| --- | --- |
| `fs::list(dir)` | Sorted array of entries (files and dirs) directly under `dir`, as absolute paths. |
| `fs::glob(pattern)` | Sorted array of regular files matching a glob, in one shot. Brace expansion and all [`globset`](https://docs.rs/globset) syntax supported. Hidden files and dot-directories are always excluded. |
| `fs::finder(glob)` | Start an `fd`-like search (chainable, see below). |

`fs::glob` is the one-shot form: give it a single full-path glob and get
back the matching files. The pattern is matched against the full
script-visible path, so anchor it with a leading `/**` to search at any
depth:

```rhai
fs::glob("/**/*.toml")            // every .toml file, anywhere
fs::glob("/**/Cargo.toml")        // every Cargo.toml, anywhere
fs::glob("/src/**/*.{rs,toml}")   // .rs or .toml under /src
```

Hidden files (dotfiles) and dot-directories like `.git`/`.terraform`
are always excluded, matching `fd`. `fs::glob` has no opt-in for them;
to traverse hidden entries reach for `fs::finder(..).hidden()` (below).

For anything beyond a single glob — combining several filters,
restricting by depth, matching directories, or excluding a subtree —
reach for `fs::finder` (below). `fs::glob(g)` is exactly
`fs::finder(g).find()`; the finder just lets you chain more filters
before running the walk.
| `fs::read(path)` | File contents as a `String`. Reflects **current disk state**. |
| `fs::write(path, content)` | Buffer a file write (parents auto-created on flush). Shares a per-path slot with `fs::remove`; the last call wins. |
| `fs::remove(path)` | Buffer a file deletion. Shares a slot with `fs::write` (last call wins); removing an absent file is a no-op. |
| `fs::exists(path)` | `true` if anything exists at `path` (current disk state). |
| `fs::is_file(path)` | `true` if `path` is a regular file. Symlinks report `false`. |
| `fs::is_dir(path)` | `true` if `path` is a directory. Symlinks report `false`. |

Pure path manipulation lives in a separate `path::` module (see the
[`path`](path.md) chapter).

## `fs::finder` — `fd`-like search

`fs::finder(glob)` begins a chainable search. The scan always starts at
the VFS root; the seed `glob` is the first filter. Its match target is
auto-detected:

- a leading `/` (or any `/` or `**`) → matched against the **full
  script-visible path**, e.g. `"/src/**/*.rs"`.
- otherwise → matched against the **basename** at any depth, like `fd`,
  e.g. `"*.rs"` finds every `*.rs` file under the root.

Chain any of the filters below to narrow the result; **all filters must
match** (logical AND). Hidden files and dot-directories are **excluded
by default** (like `fd`); call `.hidden()` to include them. Symlinks are
always skipped. `.find()` runs the walk and returns a sorted array of
script-visible paths.

| Method | Effect |
| --- | --- |
| `.files()` | Keep only regular files. |
| `.dirs()` | Keep only directories. |
| `.hidden()` | Include hidden entries (dotfiles, dot-directories), which are excluded by default. |
| `.ext(e)` | Keep entries whose name ends in `.e` (bare extension, no dot). |
| `.glob(g)` | Additional glob filter (same basename/path auto-detection as the seed). |
| `.name(n)` | Alias for `.glob(n)`; reads as "the file or directory name". |
| `.depth(n)` | Exact depth `n` (direct children of the root are depth 1). |
| `.depth(a..b)` / `.depth(a..=b)` | Depth within an exclusive / inclusive range. |
| `.not_glob(g)` / `.not_name(n)` | Exclude entries matching the glob. |
| `.not_ext(e)` | Exclude entries with extension `e`. |
| `.find()` | Run the search; returns the sorted `[String]`. |

The seed is just the first filter — keep it broad and let the chained
filters do the narrowing, rather than packing every condition into the
glob:

```rhai
// terraform files under any /AWS account, but not in vendored modules
fs::finder("/AWS/**/*.tf")
  .files()
  .not_glob("/**/.terraform/**")
  .find()

// *.rs sources, two or three levels deep (dot-dirs skipped by default)
fs::finder("*.rs")
  .files()
  .depth(2..=3)
  .find()

// directories named "node_modules" anywhere
fs::finder("node_modules").dirs().find()

// include dotfiles/dot-dirs: every .yml, .git tree and all
fs::finder("*.yml").files().hidden().find()
```

The seed can target the full path (leading `/`, or any `/`/`**`) or a
bare basename — both then accept the same chained filters above. Prefer
expressing each condition as its own filter (`.not_glob`, `.depth`,
`.ext`, …) over a single dense seed glob; it reads better and each
filter is independently adjustable.

## Buffered writes and removes

Writes and removes are **buffered** and applied only when the host
flushes, after the script returns. Each path has a single pending slot,
so `fs::write` and `fs::remove` overwrite each other — the last call to a
path wins:

```rhai
fs::write("/a", "x"); fs::remove("/a"); fs::write("/a", "y");  // ends up writing "y"
fs::remove("/b"); fs::write("/b", "x"); fs::remove("/b");      // ends up removing /b
```

On flush, writes auto-create parent directories (there is no separate
`mkdir`), and removes delete the file if it exists (removing an absent
file is a no-op). Symlinks under the root are not followed.

Scripts **cannot read their own buffered writes** — a `read` always
reflects what is currently on disk, never a pending write:

```rhai
let cfg = fs::read("/config.toml");
fs::write("/config.toml", "updated = true");
fs::read("/config.toml")    // still the original content, not "updated = true"
```

## Flushing and clearing the buffer

The host flushes the buffer automatically after the script returns
(unless `--dry-run`). A script can also act on the buffer mid-run:

| Function | Effect |
| --- | --- |
| `fs::flush()` | Apply all buffered writes/removes to disk **now**, then empty the buffer. Gated behind `--unsafe-fs-flush`. |
| `fs::clear()` | Discard all buffered writes/removes **without** applying them, emptying the buffer. Ungated. |

Both empty the buffer, so after either call there is nothing left for the
host to flush and nothing for `--diff`/`--list-changed` to report — flushed
changes are already on disk; cleared ones are gone. A subsequent
`fs::write`/`fs::remove` starts a fresh buffer as usual.

`fs::flush()` is **off by default**; calling it without `--unsafe-fs-flush`
is an error. Once a write hits disk it is no longer a buffered,
reversible change, which is why it is opt-in.

Under `--dry-run`, `fs::flush()` errors instead of writing **if it would
change any file** (a no-op flush — every buffered write already matches
disk, every buffered remove targets an absent file — is allowed). This
keeps `--dry-run` faithful: a dry run never touches disk.
