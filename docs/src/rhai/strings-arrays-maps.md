# Strings, arrays, maps

The three workhorse data types for inspecting and rewriting config.

## Strings

Use backtick **interpolation** for building strings:

```rhai
let name = "world";
`hello ${name}`           // "hello world"
`1 + 2 = ${1 + 2}`        // "1 + 2 = 3"
```

Common methods:

```rhai
s.len()
s.to_upper()   s.to_lower()
s.trim()
s.contains("x")   s.starts_with("/")   s.ends_with(".rs")
s.replace("a", "b")
s.split("/")              // -> array of parts
s.sub_string(0, 3)
```

In `ito`, the in-place string mutators that Rhai normally returns unit from
instead return the resulting string, so they can be assigned or chained while
still mutating `s` in place: `trim`, `make_upper`, `make_lower`, `clear`,
`truncate`, `crop`, `set`, `pad`, `remove`, and `replace`.

## Arrays

```rhai
let xs = [1, 2, 3];
xs.len()                  // 3
xs.push(4);
xs[0]                     // 1   (negative indexes count from the end)

// functional helpers take closures (see the Closures chapter)
xs.map(|x| x * 2)         // [2, 4, 6]
xs.filter(|x| x > 1)      // [2, 3]
xs.reduce(|a, x| a + x, 0) // 6
xs.contains(2)            // true
```

Iterate with `for` (see [Control flow](control-flow.md)).

## Object maps

Map literals use `#{ ... }`:

```rhai
let m = #{ name: "ito", count: 3 };
m.name                    // "ito"   (dot access)
m["count"]                // 3       (index access)
m.tags = ["a", "b"];      // add / overwrite

m.keys()                  // ["name", "count", "tags"]
m.values()
"name" in m               // true
```

Parsed JSON / TOML / HCL come back as these maps and arrays, so you walk
them the same way:

```rhai
let v = json::parse(fs::read("/package.json"));
for dep in v.dependencies.keys() {
    print(`${dep} = ${v.dependencies[dep]}`);
}
```
