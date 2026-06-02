# `string` — string methods

Method overrides and extensions for Rhai's built-in string type.
Rhai's built-in string mutators return unit; these overrides return the
resulting string instead, so they work as both in-place mutations and
in assignment or chained calls. `join` is added to arrays.

| Method | Effect |
| --- | --- |
| `s.trim()` | Strip leading and trailing whitespace; return result. |
| `s.trim_start()` | Strip leading whitespace only; return result. |
| `s.trim_end()` | Strip trailing whitespace only; return result. |
| `s.make_upper()` | Convert to uppercase; return result. |
| `s.make_lower()` | Convert to lowercase; return result. |
| `s.clear()` | Set to empty string; return `""`. |
| `s.truncate(len)` | Keep the first `<len>` characters; return result. |
| `s.crop(start)` | Keep from `<start>` to end (negative counts from end); return result. |
| `s.crop(start, len)` | Keep `<len>` characters from `<start>`; return result. |
| `s.set(index, ch)` | Replace character at `<index>` with `<ch>` (negative counts from end); return result. |
| `s.pad(len, fill)` | Right-pad to `<len>` characters using `<fill>` (char or string); return result. |
| `s.remove(sub)` | Remove all occurrences of `<sub>` (char or string); return result. |
| `s.replace(from, to)` | Replace all occurrences of `<from>` with `<to>` (char or string on both sides); return result. |
| `arr.join(sep)` | Concatenate array elements into a string separated by `<sep>`. |

```rhai
"  hello  ".trim()         // "hello"
"  hello  ".trim_start()   // "hello  "
"  hello  ".trim_end()     // "  hello"

["a", "b", "c"].join(", ")  // "a, b, c"
["x", "y"].join("")         // "xy"
```
