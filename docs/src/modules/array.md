# `array` — arrays

Arrays are Rhai built-ins. ito adds `join`. Methods marked *(ito)* are
extensions; the rest are standard Rhai. Full built-in reference:
[rhai.rs/book — Arrays](https://rhai.rs/book/language/arrays.html).

## Literals and indexing

```rhai
let xs = [1, 2, 3];
xs[0]                      // 1
xs[-1]                     // 3  (negative indexes count from the end)
```

## Common built-in methods

| Method | Effect |
| --- | --- |
| `xs.len()` | Element count. |
| `xs.is_empty()` | `true` if the array has no elements. |
| `xs.push(val)` | Append `<val>` to the end. |
| `xs.pop()` | Remove and return the last element. |
| `xs.insert(i, val)` | Insert `<val>` before index `<i>`. |
| `xs.remove(i)` | Remove and return element at index `<i>`. |
| `xs.contains(val)` | `true` if `<val>` is present. |
| `xs.reverse()` | Reverse in place. |
| `xs.map(\|x\| …)` | Return new array with the closure applied to each element. |
| `xs.filter(\|x\| …)` | Return new array of elements where the closure returns `true`. |
| `xs.find(\|x\| …)` | First element where closure returns `true`, or `()`. |
| `xs.reduce(\|a, x\| …, init)` | Fold elements into a single value. |
| `xs.for_each(\|x\| …)` | Call closure for each element (side-effects only). |
| `xs.sort()` | Sort in place (elements must be comparable). |
| `xs.dedup()` | Remove consecutive duplicate elements in place. |

## ito extensions

| Method | Effect |
| --- | --- |
| `xs.join(sep)` | Concatenate string elements with `<sep>` between them. *(ito)* |

```rhai
let xs = [1, 2, 3];
xs.map(|x| x * 2)          // [2, 4, 6]
xs.filter(|x| x > 1)       // [2, 3]
xs.reduce(|a, x| a + x, 0) // 6

["a", "b", "c"].join(", ")  // "a, b, c"
["x", "y"].join("")         // "xy"
```

Iterate with `for` (see [Control flow](../rhai/control-flow.md)).
