# Method naming conventions

ito's stdlib follows a consistent prefix convention so a method's name
tells you, up front, whether it mutates its receiver and what it returns.
There are three prefixes — `to_*`, `make_*`, and `with_*` — distinguished
by two axes: **does it mutate the receiver?** and **what does it return?**

| Prefix | Mutates receiver? | Returns | Typical use |
| --- | --- | --- | --- |
| `to_*` | **No** — receiver is untouched | a **new value** (the result) | pure transformations |
| `make_*` | **Yes** — in place | **nothing** (the edit *is* the result) | mutate a value you own |
| `with_*` | **Yes** — in place | the **same receiver** (`self`) | fluent builders |

The distinction between `make_*` and `with_*` is one of intent and
return: `make_*` is a statement — "make this value uppercase" edits the
value and yields nothing, so it does **not** chain. `with_*` reads as "a
task with this timeout" — it configures a builder and hands the builder
back, so a whole object reads as one chain. As a rule:

- **value transforms** (strings, …) use `to_*` / `make_*`;
- **builder configuration** (assembling an object step by step) uses
  `with_*`.

A `with_*` method returns the same receiver it was called on, so calls
chain; a `make_*` method returns nothing — call it as a statement, then
keep using the variable it mutated.

## `to_*` — pure, returns a new value

Never touches the receiver; computes a fresh result.

```rhai
let s = "  hello  ";
let t = s.to_trimmed();   // t == "hello", s is still "  hello  "
```

## `make_*` — mutate in place, return nothing

Edits the receiver in place and returns nothing, so each call stands on
its own line; the variable carries the result forward.

```rhai
let s = "  hello  ";
s.make_trimmed();   // s == "hello"
s.make_upper();     // s == "HELLO"
```

## `with_*` — fluent builders

Builders assemble an object incrementally. Each `with_*` call applies one
piece of configuration to the builder and returns the same builder, so a
whole object reads as a single chain. The chain is finished by a
**terminal** method that is *not* `with_*` — it returns something other
than the builder (e.g. `.to_string()`, `.run()`, `.template(...)`).

```rhai
// hcl — build a document (terminal: .to_string())
let doc = hcl::builder()
            .with_attribute("region", "us-east-1")
            .with_block(
                hcl::block("resource")
                    .with_label("aws_instance").with_label("web")
                    .with_attribute("ami", "ami-123")
            );
fs::write("/main.tf", doc.to_string());

// j2 — configure an engine (terminal: .template(...))
let text = j2::engine()
             .with_undefined("strict")
             .with_trim_blocks(true)
             .template("Hello {{ name }}", #{ name: "world" });

// proc — build and run a batch (terminal: .run()), --unsafe-proc
let results = proc::runner()
                .with_job(proc::task(["echo", "hi"]).with_capture())
                .with_concurrency(3)
                .run();
```

Builders carry shared state, so chaining and aliasing observe the same
object — `let b = hcl::builder(); b.with_attribute(...);` mutates `b` even
though the return value is discarded.
