# Functions

Define functions with `fn`. The last expression is the return value;
`return` exits early.

```rhai
fn bump(version) {
    let parts = version.split(".");
    let patch = parse_int(parts[2]) + 1;
    `${parts[0]}.${parts[1]}.${patch}`   // returned implicitly
}

print(bump("1.2.3"));   // "1.2.4"
```

## Parameters

Parameters are positional and untyped; pass any Rhai value.

```rhai
fn render(name, data) {
    j2::template(fs::read(name), data)
}
```

## Method-call sugar

`x.f(a, b)` is exactly `f(x, a, b)` — the value before the dot becomes
the first argument. This is why the stdlib builders chain so naturally
(`finder.files().find()`), and you can use it on your own functions too:

```rhai
fn shout(s) { s.to_upper() + "!" }

"hi".shout()    // == shout("hi") == "HI!"
```

## The `MAIN` global

`ito` pre-binds `MAIN` to `true` in the script scope, so a script can
guard entry-point-only logic:

```rhai
if MAIN {
    main();
}
```
