# yamlito YAML 1.2 dialect notes

yamlito targets [YAML 1.2]. This document records where its behavior is
worth calling out explicitly: input it accepts beyond the common subset
(in line with the spec), and the places where it deliberately falls
short. It is not a full grammar reference — only the deviations a caller
might trip over.

[YAML 1.2]: https://yaml.org/spec/1.2.2/

## Accepted per spec — beyond the common subset

These cases follow YAML 1.2 even though a stricter "common subset"
parser might reject them. They are accepted and implemented:

- **Multi-line plain scalar continuation at the document root.** A plain
  scalar may continue on following lines at column 0; the lines fold into
  a single space-joined value. `"a\nb\n"` is one scalar `"a b"` (the
  document-level indent is treated as −1). A following line that looks
  like a mapping key (`b:`) or a block indicator (`-`, `?`), or a
  document marker (`---`/`...`), is *not* folded — it ends the scalar.
- **Bare document after `...`.** A document-end marker (`...`) may be
  followed by a new document without a `---` directives-end marker, so
  `"a\n...\nb\n"` is a valid two-document stream.
- **Multi-line plain scalars in flow context.** A plain scalar inside a
  flow collection may span lines and fold, so `"[a\nb]"` is a sequence of
  one scalar `"a b"`. The flow terminators `,`, `]`, `}` and the `: `
  key/value separator still bound the scalar.
- **Anchor/tag before a key attaches to the key scalar.** In `&a k: v`
  the node properties (`&a`, `!tag`) belong to the *key* scalar `k`, not
  to the enclosing mapping — matching the spec and libyaml/serde_yaml.
  `properties()` reports them on the key node.
- **Empty (implicit-null) mapping keys at block scope.** A block mapping
  entry may have an empty key, written `: value` (the key is the implicit
  null). This matches the flow (`{: v}`) and explicit (`? \n: v`) empty-
  key forms; all three expose a null key via `key()`.

## Known limitations

These are spec-valid inputs yamlito does not (yet) handle. None of them
round-trips incorrectly — they are rejected outright (or, for duplicate
keys, resolved deterministically).

- **BOM only at stream start.** A byte-order mark is accepted only at the
  very start of the stream. The spec additionally allows a BOM before
  each document in a multi-document stream; yamlito rejects those.
- **Complex/non-scalar keys require the explicit `? key` form.** Only the
  explicit-key syntax (`? <key>`) supports non-scalar or alias keys.
  Implicit alias keys (`*a : v`) and implicit non-scalar keys
  (`{a: 1}: v`) are rejected. This is acceptable for the config-editing
  use case yamlito serves.
- **Duplicate mapping keys: first wins.** YAML 1.2 calls duplicate keys
  in a mapping an error. yamlito does not reject them; `Mapping::get`
  returns the value of the *first* matching key.
