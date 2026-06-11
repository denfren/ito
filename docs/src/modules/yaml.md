# `yaml` — YAML

| Function | Effect |
| --- | --- |
| `yaml::parse(text)` | Parse a single YAML document into native Rhai values. |
| `yaml::to_string(value)` | Serialize a Rhai value to canonical block YAML. |
| `yaml::parse_multi(text)` | Parse a multi-document stream into an array of values. |
| `yaml::to_string_multi(values)` | Serialize an array of values to a multi-document stream. |
| `yaml::edit(text)` | Parse into a format-preserving, editable cursor over the source. |

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
make decisions and *generating* config from scratch — to edit a file in
place while keeping its style, use `yaml::edit` instead.

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

## Format-preserving edits

`yaml::edit(text)` is the in-place alternative to the lossy
`parse`/`to_string` pair. It parses the source into a concrete syntax
tree and returns a **stream cursor** over the root. Navigation drills
into the tree; edits mutate it surgically; `.to_string()` renders the
result with the original layout, flow style, and comments intact.

```rhai
let stream = yaml::edit(fs::read("/config.yml"));
let doc = stream.doc;
doc["database"]["port"].write(5433);
doc["database"]["host"].set_comment("primary replica");
fs::write("/config.yml", stream.to_string());
```

`yaml::edit` always returns a stream cursor. Its `.doc` property steps to
the first document's root cursor — the usual entry point for a
single-document file. `.doc` errors on an empty stream (no documents, or
a single empty document) rather than handing back a null value to check
for.

Unlike `parse`, a cursor is *live*: indexing resolves the step against
the current tree and errors immediately if the path is missing (`key not
found: …`, `index out of range: …`), so there is no lazy "missing"
chain to check for. Each node kind is its own cursor type, so calling a
method that doesn't apply (e.g. `.read()` on a mapping) fails at dispatch
with a clear "method not found for YamlMapping".

### Cursor kinds

From the stream cursor you reach mappings, sequences, and scalars.

- **Stream** (`YamlStream`) — the whole document stream. `.doc` (first
  document's root, errors if empty), `len()` (document count), `s[i]`
  (i-th document's root), and iteration over document roots.
- **Mapping** (`YamlMapping`) — `m[key]`, `m.len()`, `m.keys()`,
  `m.values()`, `m.contains(key)`, `m.rename_key(old, new)` (returns
  `false` if absent), the structural edits `m.append(key, value)` /
  `m.append_or_update(key, value)` / `m.insert_at(index, key, value)` /
  `m.remove(key)` / `m.remove_at(index)`, and iteration yielding entries.
- **Sequence** (`YamlSequence`) — `s[i]`, `s.len()`, plus `s.mappings()`
  / `s.sequences()` / `s.scalars()` (elements filtered by kind),
  iteration over elements, and the structural edits `s.append(value)` /
  `s.insert_at(index, value)` / `s.remove_at(index)`.
- **Scalar** (`YamlScalar`) — `read()` (decode to a native value),
  `write(value)` (replace the value, re-quoting as needed), and the
  quoting helpers below.

All cursors share one underlying tree, so a mutation through one cursor
is visible through every other cursor (and through the original stream
handle).

### Reading and writing scalars

`read()` decodes a scalar with the same YAML 1.2 Core schema as `parse`
(ints, floats, bools, `null`, strings). `write(value)` replaces it,
choosing plain or single-quoted output by the same safety rules
`to_string` uses — writing a string that looks like a bool or number
quotes it automatically.

```rhai
let doc = yaml::edit(fs::read("/svc.yml")).doc;
let replicas = doc["spec"]["replicas"].read();   // 3
doc["spec"]["replicas"].write(replicas + 1);
doc["spec"]["image"].write("nginx:1.27");
```

### Adding and removing sequence items

A sequence cursor grows and shrinks in place, preserving the existing
layout — block dashes stay block, flow brackets stay flow, and a
multiline flow list keeps its per-item indentation and trailing-comma
style.

| Method | Effect |
| --- | --- |
| `append(value)` | Append `value` after the last item. |
| `insert_at(index, value)` | Insert at item position `index` (clamped to the count, so an out-of-range index appends). |
| `remove_at(index)` | Remove the item at `index` (out-of-range is a no-op). |

`value` can be a scalar, an array, or a map — nested collections are
serialized and spliced so the result always reparses. Removing the last
remaining item of a *block* sequence collapses it to a flow `[]` (a block
sequence cannot be empty); the cursor then points at that flow node, so a
following `append` grows it. Negative indices are an error.

Each of these edits replaces the underlying sequence node, but the cursor
refreshes itself to the new node, so the same handle stays valid across a
run of edits:

```rhai
let stream = yaml::edit(fs::read("/config.yml"));
let ports = stream.doc["service"]["ports"];
ports.append(8443);
ports.insert_at(0, 80);
ports.remove_at(2);
fs::write("/config.yml", stream.to_string());
```

### Adding and removing mapping entries

A mapping cursor edits entries the same way, preserving layout — block
stays block, flow stays flow. The add methods **error on a duplicate
key** (a YAML mapping cannot repeat a key); to change an existing entry
use `append_or_update` or `m[key].write(value)`.

| Method | Effect |
| --- | --- |
| `append(key, value)` | Append a new `key: value` entry after the last one. Errors if `key` already exists. |
| `insert_at(index, key, value)` | Insert a new entry at entry position `index` (clamped, so out-of-range appends). Errors if `key` already exists. |
| `append_or_update(key, value)` | Replace `key`'s value in place if it exists (keeping its position), else append. Never duplicates a key. |
| `remove(key)` | Remove the entry for `key` (returns `true` if found, `false` if absent). |
| `remove_at(index)` | Remove the entry at position `index` (out-of-range is a no-op). |

`value` can be a scalar, an array, or a map — nested collections are
serialized and spliced so the result always reparses. Removing the last
remaining entry of a *block* mapping collapses it to a flow `{}` (a block
mapping cannot be empty); the cursor then points at that flow node, so a
following `append` grows it. Negative indices are an error.

Like the sequence edits, each replaces the underlying mapping node while
refreshing the cursor, so the same handle stays valid across edits:

```rhai
let stream = yaml::edit(fs::read("/svc.yml"));
let meta = stream.doc["metadata"];
meta.append("namespace", "prod");            // new entry (errors on dup)
meta.append_or_update("name", "frontend");   // set, whether or not present
meta.remove("annotations");
fs::write("/svc.yml", stream.to_string());
```

### Iteration

Iterating a mapping yields **entries** (`YamlEntry`) with `.key()`
(decoded string), `.value()` (a value cursor), and `.key_node()` (a
scalar cursor at the key itself). Iterating a sequence or stream yields
value cursors directly.

```rhai
let doc = yaml::edit(fs::read("/config.yml")).doc;
for entry in doc {
  print(`${entry.key()} = ${entry.value().to_string()}`);
}
```

### Comments

Every node exposes its surrounding comments. The *leading* comment is the
`#` block attached above the enclosing entry (or document, at the root);
the *trailing* comment is the same-line `#` after it.

| Method | Effect |
| --- | --- |
| `comment()` | Read the leading comment block (empty string if none). |
| `set_comment(text)` | Replace it; each `\n`-separated segment becomes one `#` line. |
| `clear_comment()` | Remove the leading comment. |
| `trailing_comment()` | Read the same-line trailing comment. |
| `set_trailing_comment(text)` | Set it (errors if `text` contains `\n`). |
| `clear_trailing_comment()` | Remove it. |

`set_comment` errors inside flow-style constructs (`{…}`/`[…]`), which
have nowhere to hang a comment. At the stream level, `comment()` /
`set_comment()` target the block before the first `---` and require an
explicit `---` marker.

### Scalar quote style

A scalar cursor can switch quoting style in place. The `try_*` variants
return `false` when the conversion isn't possible (rather than erroring);
the plain variants error instead.

| Method | Effect |
| --- | --- |
| `make_unquoted()` / `try_make_unquoted()` | Drop quotes when the value is safe as a plain scalar. |
| `make_single_quoted()` / `try_make_single_quoted()` | Convert to a single-quoted scalar. |
