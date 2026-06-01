# `json` — JSON

| Function | Effect |
| --- | --- |
| `json::parse(text)` | Parse JSON into native Rhai values. |
| `json::to_string(value)` | Serialize a Rhai value to compact JSON. |
| `json::to_string_pretty(value)` | Serialize to 2-space-indented JSON. |

`parse` maps JSON onto Rhai types: objects → maps, arrays → arrays,
strings, integers/floats, booleans, and `null` → `()`. The result is
inspected with ordinary indexing and iteration.

```rhai
let v = json::parse(fs::read("/package.json"));
print(v.name);
for dep in v.dependencies.keys() { print(dep); }
```
