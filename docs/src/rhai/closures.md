# Closures

Closures are anonymous functions written `|params| body`. They are most
often passed to the array helpers.

```rhai
let double = |x| x * 2;
double.call(21)           // 42

[1, 2, 3].map(|x| x * 2)          // [2, 4, 6]
[1, 2, 3].filter(|x| x % 2 == 1)  // [1, 3]
[1, 2, 3].reduce(|sum, x| sum + x, 0)  // 6
```

A multi-statement body uses a block:

```rhai
let names = files.map(|f| {
    let v = json::parse(fs::read(f));
    v.name
});
```

## Captures

Closures capture variables from the enclosing scope by value:

```rhai
let prefix = "v";
let tagged = ["1.0", "2.0"].map(|s| prefix + s);   // ["v1.0", "v2.0"]
```

## With `fs`

A typical pass over files filters and transforms with closures:

```rhai
let configs = fs::glob("/**/*.toml")
    .filter(|p| !p.contains("/target/"))
    .map(|p| toml::parse(fs::read(p)));
```
