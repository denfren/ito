# `yaml` — YAML

| Function | Effect |
| --- | --- |
| `yaml::parse(text)` | Parse a single YAML document into native Rhai values. |
| `yaml::to_string(value)` | Serialize a Rhai value to canonical block YAML. |
| `yaml::parse_multi(text)` | Parse a multi-document stream into an array of values. |
| `yaml::to_string_multi(values)` | Serialize an array of values to a multi-document stream. |

`parse` decodes the document with the YAML 1.2 Core schema: mappings →
maps, sequences → arrays, and plain scalars are typed as integers,
floats, booleans, or `null` (`~`/`null`); quoted and block scalars stay
strings. An empty value (`key:`) and an empty stream both decode to
`()`.

```rhai
let cfg = yaml::parse(fs::read("/config.yml"));
print(cfg.name);
for tag in cfg.tags { print(tag); }
```

`parse` handles a single document. A multi-document stream (`---`
separators) is an error — use `parse_multi` for those. Anchor aliases
(`*name`) are an error too; an anchor on a value (`&name`) is accepted
and the value is read normally.

`to_string` emits canonical two-space-indented block YAML (always ending
in a newline), quoting scalars only where a plain scalar would be
misread. `parse` and `to_string` are not a round-trip: `to_string`
writes the library's own canonical layout, so comments, flow style, and
the original spacing are not preserved. They are for *reading* config to
make decisions and *generating* config from scratch — editing a file in
place while keeping its style is a separate, future facility.

```rhai
let out = yaml::to_string(#{ name: "web", ports: [80, 443] });
fs::write("/config.yml", out);
```

## Multiple documents

`parse_multi` returns one element per document, so a single-document
input yields a one-element array and an empty stream yields an empty
array. `to_string_multi` is the inverse shape: it renders each element
as a document, separated by `---`.

```rhai
let docs = yaml::parse_multi(fs::read("/k8s.yml"));
for doc in docs { print(doc.kind); }

let manifest = yaml::to_string_multi([
  #{ kind: "Service", metadata: #{ name: "web" } },
  #{ kind: "Deployment", metadata: #{ name: "web" } },
]);
fs::write("/k8s.yml", manifest);
```
