# `re` — regular expressions

Regular expressions over strings — pure computation, no disk access.

| Function | Effect |
| --- | --- |
| `re.is_match(pattern, text)` | `true` if `pattern` matches anywhere in `text`. |
| `re.find(pattern, text)` | First match as a string, or `nil` if none. |
| `re.find_all(pattern, text)` | Every non-overlapping match, as an array of strings. |
| `re.captures(pattern, text)` | First match's groups as a table, or `nil` if none. |
| `re.captures_all(pattern, text)` | Array of capture tables, one per match. |
| `re.replace(pattern, text, rep)` | Replace the first match with `rep`; returns the new string. |
| `re.replace_all(pattern, text, rep)` | Replace every match with `rep`; returns the new string. |
| `re.split(pattern, text)` | Split `text` on `pattern`; array of the pieces between matches. |

```lua
re.is_match("\\d+", "abc123")   -- true
re.find("\\d+", "a12b34")       -- "12"
re.replace_all("\\d", "a1b2", "X")  -- "aXbX"
```

## Conventions

- Patterns use the [`regex` crate](https://docs.rs/regex/latest/regex/#syntax)
  syntax — a superset of POSIX, with **no backreferences and no
  look-around**. This is **not** Lua patterns: Luau's
  `string.find`/`string.gsub` use those instead (see [`string`](string.md)).
- Every function takes the pattern as its **first** argument and
  **compiles it on each call** — there is no separate "compiled pattern"
  object.
- An invalid pattern (bad syntax) is a **script runtime error** on every
  function, not a `nil` return. A valid pattern that simply doesn't match
  returns `nil` (for the single-match functions) or an empty array (for
  the all-matches functions) — never an error.
- Named groups use `(?<name>…)` syntax and additionally appear under
  their name in capture tables (alongside their positional number).
- Matching is unanchored by default: a pattern matches anywhere in
  `text` unless it includes its own `^`/`$` anchors.

## `re.is_match`

Tests whether `pattern` matches anywhere in `text`.

**Signature:** `re.is_match(pattern: string, text: string) -> boolean`

**Parameters:**
- `pattern` — regex pattern (`regex` crate syntax).
- `text` — string to search.

**Returns:** `true` if there is a match anywhere in `text`, `false`
otherwise.

**Errors:** raises if `pattern` fails to compile (invalid syntax).

```lua
re.is_match("\\d+", "abc123")  -- true
re.is_match("\\d+", "abc")     -- false
```

## `re.find`

Returns the first match of `pattern` in `text`.

**Signature:** `re.find(pattern: string, text: string) -> string?`

**Parameters:**
- `pattern` — regex pattern.
- `text` — string to search.

**Returns:** the matched substring, or `nil` if `pattern` does not match.

**Errors:** raises if `pattern` fails to compile.

```lua
re.find("\\d+", "a12b34")  -- "12"
re.find("\\d+", "abc")     -- nil
```

## `re.find_all`

Returns every non-overlapping match of `pattern` in `text`, in order.

**Signature:** `re.find_all(pattern: string, text: string) -> {string}`

**Parameters:**
- `pattern` — regex pattern.
- `text` — string to search.

**Returns:** an array of matched substrings; empty (not `nil`) if there
are no matches.

**Errors:** raises if `pattern` fails to compile.

```lua
re.find_all("\\d+", "a12b34")  -- {"12", "34"}
re.find_all("\\d+", "abc")     -- {}
```

## `re.captures`

Returns the capture groups of the first match of `pattern` in `text`.

**Signature:** `re.captures(pattern: string, text: string) -> table?`

**Parameters:**
- `pattern` — regex pattern, may contain positional `(...)` and/or named
  `(?<name>...)` groups.
- `text` — string to search.

**Returns:** `nil` if there is no match. Otherwise a table where:
- key `"0"` is the whole match,
- numbered keys `"1"`, `"2"`, … are positional groups in order,
- named groups additionally appear under their own name (in *addition*
  to their positional number),
- a group that did not participate in the match (e.g. inside an
  unmatched alternative) is **omitted**, not set to `nil`/`false`.

**Errors:** raises if `pattern` fails to compile.

```lua
local m = re.captures("(?<y>\\d{4})-(\\d{2})", "2026-06")
-- {["0"] = "2026-06", ["1"] = "2026", ["2"] = "06", y = "2026"}

re.captures("\\d+", "abc")  -- nil
```

## `re.captures_all`

Returns the capture groups of every match of `pattern` in `text`.

**Signature:** `re.captures_all(pattern: string, text: string) -> {table}`

**Parameters:**
- `pattern` — regex pattern.
- `text` — string to search.

**Returns:** an array of capture tables (same shape as `re.captures`
produces), one per match, in order; empty if there are no matches.

**Errors:** raises if `pattern` fails to compile.

```lua
local all = re.captures_all("(\\d)(\\w)", "1a2b")
-- { {["0"]="1a", ["1"]="1", ["2"]="a"}, {["0"]="2b", ["1"]="2", ["2"]="b"} }
```

## `re.replace`

Replaces the first match of `pattern` in `text` with `rep`.

**Signature:** `re.replace(pattern: string, text: string, rep: string) -> string`

**Parameters:**
- `pattern` — regex pattern.
- `text` — string to search.
- `rep` — replacement text; supports `$1`/`${1}` (positional) and
  `${name}` (named) group references, and `$$` for a literal `$`. Use
  the `${1}` braced form when the reference is directly followed by a
  name character, so it isn't parsed as part of a longer group name.

**Returns:** a new string with the first match replaced; `text`
unchanged (but still a new string) if there was no match.

**Errors:** raises if `pattern` fails to compile.

```lua
re.replace("\\d", "a1b2", "X")  -- "aXb2"
```

## `re.replace_all`

Replaces every match of `pattern` in `text` with `rep`.

**Signature:** `re.replace_all(pattern: string, text: string, rep: string) -> string`

**Parameters:**
- `pattern` — regex pattern.
- `text` — string to search.
- `rep` — replacement text, same reference syntax as `re.replace`.

**Returns:** a new string with every match replaced.

**Errors:** raises if `pattern` fails to compile.

```lua
re.replace_all("\\d", "a1b2", "X")                -- "aXbX"
re.replace_all("(\\w)(\\d)", "a1 b2", "$2$1")     -- "1a 2b"
```

## `re.split`

Splits `text` on matches of `pattern`.

**Signature:** `re.split(pattern: string, text: string) -> {string}`

**Parameters:**
- `pattern` — regex pattern acting as the separator.
- `text` — string to split.

**Returns:** an array of the pieces of `text` between (non-overlapping)
matches of `pattern`. A match at the very start or end of `text` yields
an empty-string piece there, consistent with the underlying `Regex::split`.

**Errors:** raises if `pattern` fails to compile.

```lua
re.split(",\\s*", "a, b,c")  -- {"a", "b", "c"}
```
