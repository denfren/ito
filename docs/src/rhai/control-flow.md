# Control flow

## `if` / `else`

`if` is also an expression — it yields a value:

```rhai
if x > 0 {
    print("positive");
} else if x == 0 {
    print("zero");
} else {
    print("negative");
}

let label = if verbose { "loud" } else { "quiet" };
```

## `switch`

Matches a value against literals, with an optional default (`_`):

```rhai
let kind = switch ext {
    "json" => "data",
    "tf"   => "hcl",
    _      => "other",
};
```

## Loops

```rhai
// for over a range, array, or anything iterable
for i in 0..3 { print(i); }
for x in [10, 20, 30] { print(x); }

// for over fs results — the common ito idiom
for f in fs::glob("/**/*.toml") {
    let cfg = toml::parse(fs::read(f));
    print(`${f}: ${cfg.package.version}`);
}

// for with index: `for (value, idx) in arr`
for (f, i) in fs::list("/") {
    print(`${i}: ${f}`);
}

// while / loop
let n = 0;
while n < 3 { n += 1; }

loop {
    n -= 1;
    if n == 0 { break; }
}
```

## `break`, `continue`, `return`

```rhai
for f in fs::glob("/**/*.json") {
    if f.contains("node_modules") { continue; }   // skip
    if f.ends_with("stop.json")   { break; }       // stop the loop
}

fn first_match(items, needle) {
    for x in items {
        if x == needle { return true; }   // early return
    }
    false   // last expression is the return value
}
```
