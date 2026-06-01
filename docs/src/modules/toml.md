# `toml` — TOML

| Function | Effect |
| --- | --- |
| `toml::parse(text)` | Parse TOML into native Rhai values. |
| `toml::to_string(value)` | Serialize a Rhai value to TOML. |
| `toml::to_string_pretty(value)` | Serialize to pretty TOML. |

The top-level value for serialization must be a map (TOML has no
top-level array/scalar form).

```rhai
let cfg = toml::parse(fs::read("/Cargo.toml"));
print(cfg.package.version);
```
