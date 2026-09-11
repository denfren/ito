# `ansible` — Ansible Vault

| Function | Effect |
| --- | --- |
| `ansible.vault.decrypt(<vault>, <password>)` | the plaintext string |
| `ansible.vault.is_vault(<text>)` | `true` if `text` starts with a vault header |
| `ansible.vault.encrypt(<plain>, <password>)` | vault-1.1 text (with `$ANSIBLE_VAULT;1.1;AES256` header) |
| `ansible.vault.load_password_file(<path>)`, async | the password read from or produced by `path` |

## Example

```lua
local encrypted = ansible.vault.encrypt("s3cr3t-value", "my-vault-password")
local plain = ansible.vault.decrypt(encrypted, "my-vault-password")
```

`ansible.vault.decrypt`/`is_vault` are available in both programs —
like [`hash`](hash.md), they are a pure cipher over their arguments: bytes
and a password in, bytes out, no ambient state and no filesystem access.
`ansible.vault.encrypt` (a fresh random salt per call) and
`ansible.vault.load_password_file` (reads or executes a path) are not
available in `ied`, whose runs must be reproducible
(see [reproducibility](../binaries/ied.md#reproducibility)). They are absent,
not stubbed, so `if ansible.vault.encrypt then` is a usable feature test.

The module is **vault only**: there is no inventory parsing.

## Conventions

**Vault format 1.1 written, 1.1 and 1.2 read.** The 1.1 format is
`$ANSIBLE_VAULT;1.1;AES256` — PBKDF2-HMAC-SHA256 (10000 rounds) over a
random 32-byte salt derives 80 bytes, split into a 32-byte AES key, a
32-byte HMAC key and a 16-byte CTR IV; the plaintext is PKCS7-padded and
encrypted with AES-256-CTR, then authenticated with HMAC-SHA256 over the
ciphertext. A 1.2 file (`$ANSIBLE_VAULT;1.2;AES256;<label>`, written with
`--vault-id <label>@...`) has the identical body and is accepted by
`decrypt`; the label is ignored, since there is exactly one password per
call. `encrypt` always writes 1.1.

**A password, not a vault-id.** `password` is the raw vault password string —
the same thing you would type at the reference tool's prompt, or read from a
vault-password file with `load_password_file`. There is no vault-id lookup
and no keyring integration: those are how a password *reaches* a script,
which is the script's own business. An **empty password raises** (an unset
variable or empty file is a mistake, never a key).

**Ciphertext round-trips as vault text, not a value.** Both directions work
on the full on-disk representation — header line plus the 80-column wrapped
hex body — so `encrypt`'s output can be written straight to a file with
[`fs.write`](fs.md), and `decrypt` reads back whatever the reference tool
would have written, including re-wrapped or re-indented text. The header
line is checked (surrounding whitespace trimmed); the body lines are trimmed
and re-joined, so indentation, any wrap width and trailing blank lines do not
matter. That makes an inline YAML secret usable too: a value written as
`!vault |` followed by an indented vault block is just a string once parsed,
and the indentation is tolerated —

```lua
local doc = yaml.parse(fs.read("vars.yml"))
local secret = ansible.vault.decrypt(doc.db_password, pw)
```

(the `!vault` tag is not interpreted by [`yaml`](yaml.md); the block scalar's
text is what `decrypt` receives.)

## `ansible.vault.encrypt`

Encrypt a plaintext string into vault-1.1 text. Not available in `ied`.

**Signature:** `ansible.vault.encrypt(plain, password) -> string`

**Parameters:**

- `plain` — the plaintext to encrypt, as a string. Not UTF-8 validated:
  a Luau string is 8-bit clean, so arbitrary binary round-trips.
- `password` — the vault password, as a string; must not be empty.

**Returns:** the vault-1.1 text — the `$ANSIBLE_VAULT;1.1;AES256` header
line, followed by the hex payload wrapped at 80 columns. A fresh random
32-byte salt is drawn on every call, so encrypting the same plaintext under
the same password twice gives two different (but equally valid) outputs.

**Errors:** raises `"ansible.vault: empty password"`; a wrong argument type
raises the usual `bad argument` error.

```lua
local encrypted = ansible.vault.encrypt("super-secret-value\n", "hunter2")
fs.write("secrets.yml.vault", encrypted)
```

## `ansible.vault.decrypt`

Decrypt vault text back to its plaintext string.

**Signature:** `ansible.vault.decrypt(vault, password) -> string`

**Parameters:**

- `vault` — the vault text (1.1, or 1.2 with a label): the header line plus
  the wrapped hex body, as `ansible.vault.encrypt` returns it or as the
  reference tool writes it to a file. Line width and leading whitespace on
  the body lines are not checked. Must be valid UTF-8 (it is hex, so any real
  vault is).
- `password` — the vault password; must not be empty.

**Returns:** the plaintext, as a string (8-bit clean, not UTF-8 validated).

**Errors:**

- `"ansible.vault: empty input"` — `vault` is empty or its first line is blank.
- `"ansible.vault: not a vault (expected '$ANSIBLE_VAULT;1.1;AES256' or
  '$ANSIBLE_VAULT;1.2;AES256;<label>' header)"` — the first line (trimmed) is
  neither header.
- `"ansible.vault: input is not valid UTF-8"`.
- `"ansible.vault: malformed vault payload"` — the body is not three
  hex-encoded lines (salt, HMAC, ciphertext); text after them is ignored.
- `"ansible.vault: HMAC verification failed (wrong key?)"` — the password is
  wrong or the payload was tampered with.
- `"ansible.vault: invalid padding (wrong key?)"` — the HMAC verified but the
  padding did not (defense in depth).
- `"ansible.vault: empty password"`.

```lua
local plain = ansible.vault.decrypt(fs.read("secrets.yml.vault"), "hunter2")
```

## `ansible.vault.is_vault`

Whether a text looks like a vault, without decrypting it.

**Signature:** `ansible.vault.is_vault(text) -> boolean`

**Parameters:** `text` — any string.

**Returns:** `true` if the first line (whitespace trimmed) is a 1.1 header or
a 1.2 header with a label; `false` otherwise, including for empty or
non-UTF-8 input. A header sniff only — the body is not validated.

```lua
if ansible.vault.is_vault(text) then text = ansible.vault.decrypt(text, pw) end
```

## `ansible.vault.load_password_file`

Resolve a vault password the way the reference tool's
`--vault-password-file` does: if `path` is executable, run it and take its
stdout as the password; otherwise read the file's contents directly.
Not available in `ied`.

**Signature:** `ansible.vault.load_password_file(path) -> string`, async

**Parameters:**

- `path` — either a plain-text file containing the password, or an
  executable script that prints it to stdout. Relative paths resolve against
  the `-C` working directory, as everywhere in [`fs`](fs.md), and are made
  absolute before running, so `pw.sh` runs *that* file rather than a
  `pw.sh` found on `$PATH`. **`~` is not expanded** — use
  `os.getenv("HOME")` and `path.join`. The script gets no stdin
  (`/dev/null`); its stderr is passed through to the terminal.

**Returns:** the password, with leading and trailing whitespace stripped —
the same strip the reference tool applies to both a plain password file and
an executable one's stdout, so a trailing newline from a text editor or
`echo` never becomes part of the password. Whitespace *inside* the
remaining text is kept verbatim.

**Errors:** raises `"ansible.vault.load_password_file: <path>: <reason>"`
when `path` does not exist or cannot be read; `"... running '<path>':
<reason>"` when an executable fails to spawn; `"... '<path>' exited with
<status>"` when it exits non-zero; and `"... '<path>' yielded an empty
password"` when the result is empty after stripping.

```lua
local pw = ansible.vault.load_password_file(path.join(os.getenv("HOME"), ".vault_pass"))
local plain = ansible.vault.decrypt(fs.read("secrets.yml.vault"), pw)
```
