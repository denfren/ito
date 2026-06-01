# `path` — path manipulation

Pure path manipulation, separate from the [`fs`](fs.md) module. These are
total string functions — no disk access, no sandbox resolution — so their
results feed straight back into `fs::` (mirrors the `std::fs` / `std::path`
split):

| Function | Effect |
| --- | --- |
| `path::join(parts)` / `path::join(a, b)` | Join fragments into one path; see below. |
| `path::product(a, b)` | Cartesian product of two axes, each pair joined; see below. |
| `path::parent(path)` | Parent directory of `path` (`""` if none). |
| `path::file_name(path)` | Final path component (`""` if none). |
| `path::extension(path)` | Extension without the dot (`""` if none). |
| `path::stem(path)` | File name without its extension. |
| `path::capture(path, pattern)` | Match `path` against a named-segment pattern; map of captures, or `()` on no match. |

## Batch overloads — avoid looping in Rhai

`parent`, `file_name`, `extension`, `stem`, and `capture` each take an
**array of paths** as well as a single path, mapping element-wise and
returning an array of the same length (arrays must be flat strings):

```rhai
path::parent(["/a/b", "/c/d.txt"])   // ["/a", "/c"]
path::stem(["/c/d.txt"])             // ["d"]
path::capture(["/AWS/1/web", "/x"], "/AWS/{acct}/{proj}")
// [#{ acct: "1", proj: "web" }, ()]   — one entry per input, () on no match
```

## `path::join` — concatenate fragments

`path::join` flattens its arguments (each a string or an array of
strings) in order and joins them into a single path. Pass one array, or
two arguments in any string/array combination:

```rhai
path::join(["/a", "b", "c"])     // "/a/b/c"
path::join("/a", "b")            // "/a/b"
path::join("/a", ["b", "c"])     // "/a/b/c"
path::join(["/a", "b"], "c")     // "/a/b/c"
path::join(["/a", "b"], ["c"])   // "/a/b/c"
```

## `path::product` — cartesian product of paths

`path::product(a, b)` takes two axes (each a string or an array of
strings) and joins every left×right combination into a path, returning a
flat array. An empty axis yields an empty result:

```rhai
path::product(["/a", "/b"], ["x", "y"])  // ["/a/x", "/a/y", "/b/x", "/b/y"]
path::product("/a", ["x", "y"])          // ["/a/x", "/a/y"]
path::product([], ["x"])                 // []
```

## `path::capture` — named path segments

`path::capture(path, pattern)` matches a path against a segment pattern
and returns a map of the captured segments, or `()` when it does not
match. The pattern is split on `/`; literal segments must match exactly
and placeholders capture:

| Placeholder | Captures |
| --- | --- |
| `{name}` | exactly one segment (required) |
| `{name?}` | zero or one segment (absent ⇒ key omitted) |
| `{name*}` | one or more segments, joined with `/` |
| `{name*?}` | zero or more segments, joined with `/` (empty ⇒ `""`) |

A variable-width capture (`{name*}` / `{name*?}`) is greedy but bounded
by the next literal segment, so it must be **followed by a literal or
end the pattern** — two adjacent variable-width captures are rejected as
ambiguous.

```rhai
path::capture("/AWS/123/web/resources", "/AWS/{account}/{project}/resources")
// #{ account: "123", project: "web" }

path::capture("/AWS/123/web/prod/resources/a/b", "/AWS/{account}/{rest*}/resources/{tail*}")
// #{ account: "123", rest: "web/prod", tail: "a/b" }

path::capture("/AWS/123", "/AWS/{account}/{project}")
// ()  — no match

for d in fs::finder("/AWS/**").dirs().find() {
    let m = path::capture(d, "/AWS/{account}/{project}/resources");
    if m != () { print(`${m.account} / ${m.project}`); }
}
```
