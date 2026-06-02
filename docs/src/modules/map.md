# `map` — object maps

Object maps are Rhai built-ins (no ito patches). Full built-in
reference:
[rhai.rs/book — Object Maps](https://rhai.rs/book/language/object-maps.html).

## Literals and access

```rhai
let m = #{ name: "ito", count: 3 };
m.name                     // "ito"  (dot access)
m["count"]                 // 3      (index access)
m.tags = ["a", "b"];       // add or overwrite a key
```

## Common built-in methods

| Method | Effect |
| --- | --- |
| `m.len()` | Number of key-value pairs. |
| `m.is_empty()` | `true` if the map has no entries. |
| `m.keys()` | Array of all keys (strings). |
| `m.values()` | Array of all values. |
| `m.contains(key)` | `true` if `<key>` exists. |
| `m.remove(key)` | Remove `<key>` and return its value (or `()`). |
| `"key" in m` | `true` if `"key"` is present (operator form). |

```rhai
let m = #{ a: 1, b: 2 };
m.keys()                   // ["a", "b"]
"a" in m                   // true
m.remove("a");             // m is now #{ b: 2 }
```

Parsed JSON / TOML / HCL / YAML come back as maps and arrays, so you
walk them the same way:

```rhai
let v = json::parse(fs::read("package.json"));
for dep in v.dependencies.keys() {
    print(`${dep} = ${v.dependencies[dep]}`);
}
```
