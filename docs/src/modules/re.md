# `re` — regular expressions

Regular expressions over strings — pure computation, no disk access.
Patterns use the [`regex` crate](https://docs.rs/regex/latest/regex/#syntax)
syntax (a superset of POSIX; no backreferences or look-around). Every
function takes the pattern as its first argument and compiles it on each
call; an invalid pattern is a runtime error.

| Function | Effect |
| --- | --- |
| `re::is_match(pattern, text)` | `true` if `pattern` matches anywhere in `text`. |
| `re::find(pattern, text)` | First match as a string, or `()` if none. |
| `re::find_all(pattern, text)` | Every non-overlapping match, as an array of strings. |
| `re::captures(pattern, text)` | First match's groups as a map (see below), or `()`. |
| `re::captures_all(pattern, text)` | Array of capture maps, one per match. |
| `re::replace(pattern, text, rep)` | Replace the first match; returns the new string. |
| `re::replace_all(pattern, text, rep)` | Replace every match; returns the new string. |
| `re::split(pattern, text)` | Split `text` on `pattern`; array of the pieces. |

## Captures

`re::captures` / `re::captures_all` return a map per match. Key `"0"` is
the whole match; numbered keys (`"1"`, `"2"`, …) are positional groups;
named groups (`(?<name>…)`) also appear under their name. Groups that did
not participate in the match are omitted.

```rhai
re::is_match("\\d+", "abc123")        // true
re::find("\\d+", "a12b34")            // "12"
re::find_all("\\d+", "a12b34")        // ["12", "34"]
re::split(",\\s*", "a, b,c")          // ["a", "b", "c"]

let m = re::captures("(?<y>\\d{4})-(\\d{2})", "2026-06");
// #{ "0": "2026-06", "1": "2026", "2": "06", "y": "2026" }
```

## Replacement references

`rep` supports `$1` / `${name}` group references (use `${1}` when the
following character is a name character, and `$$` for a literal `$`):

```rhai
re::replace_all("(\\w)(\\d)", "a1 b2", "$2$1")   // "1a 2b"
re::replace("\\d", "a1b2", "X")                  // "aXb2"
```
