# `re` — regular expressions

Regular expressions over strings — pure computation, no disk access.

| Function | Effect |
| --- | --- |
| `re.is_match(pattern, text)` | `true` if `pattern` matches anywhere in `text`. |
| `re.match(pattern, text)` | Capture table if `pattern` matches the **whole** text, else `nil`. |
| `re.find(pattern, text)` | `match, start, end` of the first match (1-based, inclusive), or `nil`. |
| `re.find_all(pattern, text)` | Every non-overlapping match, as an array of strings. |
| `re.count(pattern, text)` | How many non-overlapping matches. |
| `re.captures(pattern, text)` | First match's groups as a table, or `nil` if none. |
| `re.captures_all(pattern, text)` | Array of capture tables, one per match. |
| `re.replace(pattern, text, rep)` | Replace the first match; returns the new string. |
| `re.replace_all(pattern, text, rep[, n])` | Replace every match (at most `n`); returns the new string. |
| `re.split(pattern, text[, n])` | Split `text` on `pattern`; array of the pieces (at most `n`). |
| `re.escape(text)` | `text` with every regex metacharacter escaped. |
| `re.compile(pattern)` | A compiled pattern whose methods are the functions above without the `pattern` argument. |

## Example

```lua
re.is_match([[\d+]], "abc123")          -- true
re.find([[\d+]], "a12b34")              -- "12", 2, 3
re.replace_all([[\d]], "a1b2", "X")     -- "aXbX"
```

## Conventions

- Patterns use the [`regex` crate](https://docs.rs/regex/latest/regex/#syntax)
  syntax — RE2/Perl-style, with **no backreferences and no look-around**.
  This is **not** Lua patterns: Luau's `string.find`/`string.gsub` use
  those instead (see [`string`](string.md)).
- **Write patterns in long brackets** — `[[\d+]]` — so backslashes reach
  the regex untouched; in a quoted string every backslash must be doubled
  (`"\\d+"`).
- Inline flags work: `(?i)` case-insensitive, `(?m)` multi-line anchors,
  `(?s)` dot matches newline, `(?x)` verbose. `\d`, `\w`, `\s` and `\b` are
  **Unicode-aware** (`\d` matches Arabic-Indic digits too); write
  `[0-9]` or `(?-u:\d)` for ASCII only.
- Matching is unanchored by default: a pattern matches anywhere in
  `text` unless it includes its own `^`/`$` anchors (or use `re.match`).
- `pattern` and `text` must be **strings** — a number is not coerced and
  raises. `pattern` must be valid UTF-8; **`text` may be any bytes** (Luau
  strings are byte sequences), and results are byte strings.
- Patterns are compiled once and cached, so calling `re.find(p, s)` in a
  loop does not recompile; `re.compile` gives an explicit handle.
- An invalid pattern is a **script runtime error** naming the function and
  the pattern (`re.find: invalid pattern "(": …`), not a `nil` return. A
  valid pattern that simply doesn't match returns `nil` (single-match
  functions) or an empty array — never an error.
- **Capture tables** have integer keys `0` (the whole match), `1`, `2`, …
  for positional groups and, additionally, the group's name for named
  groups `(?<name>…)`. A group that did not participate is **absent**, so
  `m[1]`, `ipairs` and `unpack` work on the positional part (`unpack`
  leaves `nil` for an absent group). `fs.finder():grep()` reports its
  `captures` in the same shape.
- Replacement templates: `$1`/`${1}` (positional), `${name}` (named), and
  `$$` for a literal `$`. Use the braced form when the reference is followed
  by a name character (`${1}a`). A reference to a group the pattern does not
  have **raises** instead of expanding to `""`. A `$` followed by anything
  else is a literal `$`.

## `re.is_match`

Tests whether `pattern` matches anywhere in `text`.

**Signature:** `re.is_match(pattern: string, text: string) -> boolean`

**Parameters:**
- `pattern` — regex pattern.
- `text` — string to search.

**Returns:** `true` if there is a match anywhere in `text`, `false`
otherwise.

**Errors:** raises on an invalid pattern or a non-string argument.

```lua
re.is_match([[\d+]], "abc123")  -- true
re.is_match([[\d+]], "abc")     -- false
```

## `re.match`

Matches `pattern` against the **entire** text (as if wrapped in `^(?:…)$`,
without the multi-line reading of `^`/`$`).

**Signature:** `re.match(pattern: string, text: string) -> table?`

**Parameters:** as `re.captures`.

**Returns:** the capture table (see Conventions) if the whole text
matches, else `nil`. Unlike `re.captures`, `"ab"` against `a|ab` matches.

**Errors:** raises on an invalid pattern or a non-string argument.

```lua
re.match([[(\d+)-(\d+)]], "10-20")[2]   -- "20"
re.match([[\d+]], "10 apples")          -- nil
```

## `re.find`

Returns the first match of `pattern` in `text`, with its position.

**Signature:** `re.find(pattern: string, text: string) -> (string, number, number)?`

**Parameters:**
- `pattern` — regex pattern.
- `text` — string to search.

**Returns:** the matched substring and its 1-based **inclusive** start and
end byte positions (like `string.find`, so `text:sub(s, e)` is the match),
or `nil` if `pattern` does not match. An empty match has `end == start - 1`.

**Errors:** raises on an invalid pattern or a non-string argument.

```lua
local m, s, e = re.find([[\d+]], "a12b34")  -- "12", 2, 3
re.find([[\d+]], "abc")                      -- nil
```

## `re.find_all`

Returns every non-overlapping match of `pattern` in `text`, in order.

**Signature:** `re.find_all(pattern: string, text: string) -> {string}`

**Parameters:**
- `pattern` — regex pattern.
- `text` — string to search.

**Returns:** an array of matched substrings; empty (not `nil`) if there
are no matches.

**Errors:** raises on an invalid pattern or a non-string argument.

```lua
re.find_all([[\d+]], "a12b34")  -- {"12", "34"}
re.find_all([[\d+]], "abc")     -- {}
```

## `re.count`

**Signature:** `re.count(pattern: string, text: string) -> number`

**Parameters:** as `re.find_all`.

**Returns:** the number of non-overlapping matches (`0` if none).

**Errors:** raises on an invalid pattern or a non-string argument.

```lua
re.count([[\d]], "a1b22")  -- 3
```

## `re.captures`

Returns the capture groups of the first match of `pattern` in `text`.

**Signature:** `re.captures(pattern: string, text: string) -> table?`

**Parameters:**
- `pattern` — regex pattern, may contain positional `(...)` and/or named
  `(?<name>...)` groups.
- `text` — string to search.

**Returns:** `nil` if there is no match. Otherwise a capture table (see
Conventions): `[0]` is the whole match, `[1]`, `[2]`, … the positional
groups, named groups also under their name; a non-participating group is
omitted.

**Errors:** raises on an invalid pattern or a non-string argument.

```lua
local m = re.captures([[(?<y>\d{4})-(\d{2})]], "2026-06")
-- {[0] = "2026-06", [1] = "2026", [2] = "06", y = "2026"}
local y, mo = unpack(m)

re.captures([[\d+]], "abc")  -- nil
```

## `re.captures_all`

Returns the capture groups of every match of `pattern` in `text`.

**Signature:** `re.captures_all(pattern: string, text: string) -> {table}`

**Parameters:** as `re.captures`.

**Returns:** an array of capture tables, one per match, in order; empty if
there are no matches.

**Errors:** raises on an invalid pattern or a non-string argument.

```lua
local all = re.captures_all([[(\d)(\w)]], "1a2b")
-- { {[0]="1a", [1]="1", [2]="a"}, {[0]="2b", [1]="2", [2]="b"} }
```

## `re.replace`

Replaces the first match of `pattern` in `text`.

**Signature:** `re.replace(pattern: string, text: string, rep: string | function) -> string`

**Parameters:**
- `pattern` — regex pattern.
- `text` — string to search.
- `rep` — a replacement template (see Conventions), or a function called
  with the match's capture table and returning the replacement **string**.

**Returns:** a new string with the first match replaced; `text` unchanged
if there was no match.

**Errors:** raises on an invalid pattern, a non-string `text`, a template
referencing an unknown group, a `rep` that is neither string nor function,
or a function that returns a non-string.

```lua
re.replace([[\d]], "a1b2", "X")  -- "aXb2"
```

## `re.replace_all`

Replaces every match of `pattern` in `text`, or the first `n`.

**Signature:** `re.replace_all(pattern: string, text: string, rep: string | function, n: number?) -> string`

**Parameters:**
- `pattern`, `text`, `rep` — as `re.replace`.
- `n` — optional positive integer: replace at most this many matches.

**Returns:** a new string with the matches replaced.

**Errors:** as `re.replace`; also if `n` is not a positive integer.

```lua
re.replace_all([[\d]], "a1b2", "X")                     -- "aXbX"
re.replace_all([[(\w)(\d)]], "a1 b2", "$2$1")           -- "1a 2b"
re.replace_all([[\d]], "1234", "X", 2)                  -- "XX34"
re.replace_all([[\d+]], "a1 b22", function(m) return "<" .. m[0] .. ">" end)
-- "a<1> b<22>"
```

## `re.split`

Splits `text` on matches of `pattern`.

**Signature:** `re.split(pattern: string, text: string, n: number?) -> {string}`

**Parameters:**
- `pattern` — regex pattern acting as the separator.
- `text` — string to split.
- `n` — optional positive integer: at most this many pieces (the last holds
  the unsplit remainder).

**Returns:** an array of the pieces of `text` between (non-overlapping)
matches. Edge cases: a match at the very start or end yields an empty
string piece there; splitting `""` gives `{""}`; text with no match gives
`{text}`; an empty pattern splits between every character (with an empty
piece at each end).

**Errors:** raises on an invalid pattern, a non-string argument, or a bad
`n`.

```lua
re.split([[,\s*]], "a, b,c")     -- {"a", "b", "c"}
re.split(",", ",a,")             -- {"", "a", ""}
re.split(",", "a,b,c", 2)        -- {"a", "b,c"}
```

## `re.escape`

**Signature:** `re.escape(text: string) -> string`

**Parameters:** `text` — literal text (valid UTF-8).

**Returns:** `text` with every regex metacharacter backslash-escaped, so it
can be embedded in a pattern as a literal.

**Errors:** raises on a non-string or non-UTF-8 argument.

```lua
re.escape("a.b*c")                             -- [[a\.b\*c]]
re.is_match("^" .. re.escape(name) .. "$", s)
```

## `re.compile`

Compile a pattern once and reuse it.

**Signature:** `re.compile(pattern: string) -> Re`

**Parameters:** `pattern` — regex pattern.

**Returns:** a handle with the methods `is_match`, `match`, `find`,
`find_all`, `count`, `captures`, `captures_all`, `replace`, `replace_all`
and `split` — each the function of the same name minus the `pattern`
argument (`r:find(text)`).

**Errors:** raises on an invalid pattern.

```lua
local word = re.compile([[\w+]])
for _, line in ipairs(lines) do print(word:count(line)) end
```
