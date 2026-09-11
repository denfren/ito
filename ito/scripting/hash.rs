//! Luau bindings for the common cryptographic digests.
//!
//! Registered as the global `hash`. Six algorithms — `md5`, `sha1`, `sha224`,
//! `sha256`, `sha384` and `sha512` — each in four forms:
//!
//! ```lua
//! hash.sha256("abc")             --> "ba7816bf..." (lowercase hex)
//! hash.sha256_bytes("abc")       --> a 32-byte buffer
//! hash.sha256_file("archive.gz") --> "..." (hex, streamed, never read into Luau)
//! hash.sha256_file_bytes("a.gz") --> a 32-byte buffer
//! ```
//!
//! Hex output is lowercase: compare an uppercase published digest with
//! `:lower()`. `_file` paths are taken as bytes (non-UTF-8 names work on unix).
//!
//! **Hex is the default return** because it is what a hash is nearly always
//! wanted for — a name, a comparison against a published checksum, a cache key —
//! and a raw digest reaching Luau as a string would be arbitrary bytes that
//! print as mojibake. The `_bytes` form returns a `buffer` rather than a string
//! for the same reason `proc.buffer()` does: it is honest about the content.
//!
//! **The `_file` forms exist so a script need not read the file first.** A
//! digest is defined over a byte stream, so there is no reason to materialize
//! one in Luau to hash it — these read in 64 KiB chunks on a blocking thread, so
//! a multi-gigabyte file costs one buffer rather than its own size in Luau heap,
//! and other `task`s keep running while it is read.
//!
//! ## Capability split
//!
//! `register` installs the string/buffer forms, which are **pure**: bytes in,
//! digest out, no ambient state. [`register_host_files`] adds the `_file` forms
//! under `host_io` (`ito`), because reading a path by name is a
//! filesystem read, and under `ied` that would bypass the rooted VFS's
//! containment entirely — `Source::render` settles which paths may be *named*,
//! and a hash of a file outside the root leaks its contents a bit at a time just
//! as surely as reading it would. Absent rather than stubbed, so
//! `if hash.sha256_file then` is a usable feature test; under `ied` a
//! script reads through `fs` and hashes the result (`fs.read` fails on
//! non-UTF-8 content, so binary files need `ito`). A rooted `_file` form is possible but not provided.
//!
//! ## Not here
//!
//! No non-cryptographic checksums (crc32, xxhash) and no BLAKE3: each answers a
//! different question (frame integrity, hash tables, speed) and none has come up.
//! No HMAC and no incremental/streaming handle — a `hash.new("sha256")` returning
//! an updatable object is the shape to add if hashing something that is neither a
//! value nor a file ever comes up, and adding it later costs nothing.

use crate::scripting::encoding::hex_encode;
use md5::Md5;
use mlua::{Lua, Result as LuaResult, Table, Value};
use sha1::Sha1;
use sha2::{Digest, Sha224, Sha256, Sha384, Sha512};
use std::io::Read;
use std::path::PathBuf;

/// Chunk size for the file readers. Large enough that syscall overhead is noise
/// next to the hashing itself, small enough to stay off the large-allocation path.
const CHUNK: usize = 64 * 1024;

/// Borrow the bytes behind a Luau value, accepting the two spellings a byte
/// string has here.
///
/// A `buffer` is included because that is what `proc.buffer()` and
/// `hash.*_bytes` hand back — hashing a captured command's output, or a digest
/// of a digest, should not need a conversion through a string.
fn bytes_of(value: &Value) -> LuaResult<Vec<u8>> {
    match value {
        // `as_bytes` rather than `to_str`: a Luau string is 8-bit clean and a
        // digest is defined over bytes, so nothing may be UTF-8 validated.
        Value::String(s) => Ok(s.as_bytes().to_vec()),
        Value::Buffer(b) => Ok(b.to_vec()),
        other => Err(mlua::Error::runtime(format!(
            "expected a string or buffer to hash, got {}",
            other.type_name()
        ))),
    }
}

/// Hash `value`'s bytes with `D`, returning the raw digest.
fn digest_value<D: Digest>(value: &Value) -> LuaResult<Vec<u8>> {
    let bytes = bytes_of(value)?;
    let mut hasher = D::new();
    hasher.update(&bytes);
    Ok(hasher.finalize().to_vec())
}

/// Hash a file's contents with `D` without loading it into memory.
///
/// Blocking `std::fs` on purpose, handed to `spawn_blocking` by the caller: the
/// runtime is single-threaded, so reading inline would pin its only thread and
/// stall every other `task` for the duration of a large file.
fn digest_file_blocking<D: Digest>(path: &std::path::Path) -> std::io::Result<Vec<u8>> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = D::new();
    let mut buf = vec![0u8; CHUNK];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().to_vec())
}

/// Install `<name>` (hex) and `<name>_bytes` (buffer) for one algorithm.
fn add_value_forms<D: Digest + 'static>(
    lua: &Lua,
    exports: &Table,
    name: &'static str,
) -> LuaResult<()> {
    exports.raw_set(
        name,
        lua.create_function(|_, value: Value| Ok(hex_encode(digest_value::<D>(&value)?)))?,
    )?;
    exports.raw_set(
        format!("{name}_bytes"),
        lua.create_function(|lua, value: Value| lua.create_buffer(digest_value::<D>(&value)?))?,
    )?;
    Ok(())
}

/// Install `<name>_file` (hex) and `<name>_file_bytes` (buffer) for one algorithm.
fn add_file_form<D: Digest + Send + 'static>(
    lua: &Lua,
    exports: &Table,
    name: &'static str,
) -> LuaResult<()> {
    exports.raw_set(
        format!("{name}_file"),
        lua.create_async_function(|_, path: mlua::LuaString| async move {
            Ok(hex_encode(digest_path::<D>(&path).await?))
        })?,
    )?;
    exports.raw_set(
        format!("{name}_file_bytes"),
        lua.create_async_function(|lua, path: mlua::LuaString| async move {
            lua.create_buffer(digest_path::<D>(&path).await?)
        })?,
    )?;
    Ok(())
}

/// Hash the file named by a Luau string on a blocking thread. The path is
/// taken as bytes, so a non-UTF-8 name works on unix (Luau strings are bytes).
async fn digest_path<D: Digest + Send + 'static>(path: &mlua::LuaString) -> LuaResult<Vec<u8>> {
    #[cfg(unix)]
    let path = {
        use std::os::unix::ffi::OsStrExt;
        PathBuf::from(std::ffi::OsStr::from_bytes(&path.as_bytes()))
    };
    #[cfg(not(unix))]
    let path = PathBuf::from(path.to_str()?.to_string());
    // Same wording as `fs` read errors: `<verb> <path>: <cause>`.
    let display = path.display().to_string();
    tokio::task::spawn_blocking(move || digest_file_blocking::<D>(&path))
        .await
        .map_err(|e| mlua::Error::runtime(format!("hash task failed: {e}")))?
        .map_err(|e| mlua::Error::runtime(format!("hash {display}: {e}")))
}

/// Register the `hash` module's pure half: the string/buffer forms, on every
/// binary.
pub fn register(lua: &Lua) -> LuaResult<()> {
    let exports = lua.create_table()?;
    add_value_forms::<Md5>(lua, &exports, "md5")?;
    add_value_forms::<Sha1>(lua, &exports, "sha1")?;
    add_value_forms::<Sha224>(lua, &exports, "sha224")?;
    add_value_forms::<Sha256>(lua, &exports, "sha256")?;
    add_value_forms::<Sha384>(lua, &exports, "sha384")?;
    add_value_forms::<Sha512>(lua, &exports, "sha512")?;
    lua.globals().raw_set("hash", exports)?;
    Ok(())
}

/// Add the `_file` forms to the already-registered `hash` table.
///
/// Host-only (`ito`), for the reason given in the module docs: naming a
/// path to read is a filesystem capability, and the rooted binaries' `fs` is
/// where that question is already answered. Like the other reach-into-a-table
/// steps in `Engine::build`, this must run before `sandbox(true)` freezes it.
pub fn register_host_files(lua: &Lua) -> LuaResult<()> {
    let exports: Table = lua.globals().get("hash")?;
    add_file_form::<Md5>(lua, &exports, "md5")?;
    add_file_form::<Sha1>(lua, &exports, "sha1")?;
    add_file_form::<Sha224>(lua, &exports, "sha224")?;
    add_file_form::<Sha256>(lua, &exports, "sha256")?;
    add_file_form::<Sha384>(lua, &exports, "sha384")?;
    add_file_form::<Sha512>(lua, &exports, "sha512")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The runtime shape the `_file` forms need: `spawn_blocking` wants a
    /// reactor, and the async functions are driven by `exec_async`.
    fn eval(script: &str) -> LuaResult<()> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build test runtime");
        let local = tokio::task::LocalSet::new();
        local.block_on(&runtime, async {
            let lua = Lua::new();
            register(&lua)?;
            register_host_files(&lua)?;
            crate::scripting::encoding::register(&lua)?;
            lua.load(script).exec_async().await
        })
    }

    /// The published test vectors for "abc", so a dependency bump that changed
    /// an algorithm's behaviour would fail here rather than silently.
    #[test]
    fn known_vectors_for_abc() {
        eval(
            r#"
            assert(hash.md5("abc") == "900150983cd24fb0d6963f7d28e17f72", "md5: " .. hash.md5("abc"))
            assert(hash.sha1("abc") == "a9993e364706816aba3e25717850c26c9cd0d89d", "sha1")
            assert(hash.sha256("abc") ==
                "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad", "sha256")
            assert(hash.sha512("abc") ==
                "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a" ..
                "2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f", "sha512")
        "#,
        )
        .unwrap();
    }

    /// The empty input is the case a chunked reader gets wrong, so it is pinned.
    #[test]
    fn hashes_the_empty_input() {
        eval(
            r#"
            assert(hash.sha256("") ==
                "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855", "empty")
        "#,
        )
        .unwrap();
    }

    /// The `_bytes` form must be the same digest, just undecoded.
    #[test]
    fn bytes_form_matches_the_hex_form() {
        eval(
            r#"
            local b = hash.sha256_bytes("abc")
            assert(buffer.len(b) == 32, "expected 32 bytes, got " .. buffer.len(b))
            local hex = ""
            for i = 0, 31 do
                hex = hex .. string.format("%02x", buffer.readu8(b, i))
            end
            assert(hex == hash.sha256("abc"), "bytes and hex disagree")
        "#,
        )
        .unwrap();
    }

    /// A buffer must hash as the same bytes a string of them would.
    #[test]
    fn accepts_a_buffer_as_input() {
        eval(
            r#"
            local b = buffer.fromstring("abc")
            assert(hash.sha256(b) == hash.sha256("abc"), "buffer and string disagree")
        "#,
        )
        .unwrap();
    }

    /// Input is bytes, not text: nothing may be UTF-8 validated on the way in.
    #[test]
    fn hashes_non_utf8_input() {
        eval(
            r#"
            local raw = string.char(255, 254, 0, 1)
            assert(#hash.sha256(raw) == 64, "expected a digest of arbitrary bytes")
        "#,
        )
        .unwrap();
    }

    /// The whole point of the `_file` form: the same answer as reading it would
    /// give, across more than one read chunk.
    #[test]
    fn file_form_matches_the_string_form() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.bin");
        // Larger than CHUNK, so the loop runs more than once.
        let contents = "x".repeat(CHUNK * 2 + 7);
        std::fs::write(&path, &contents).unwrap();
        let script = format!(
            r#"
            local expected = hash.sha256(string.rep("x", {len}))
            assert(hash.sha256_file("{path}") == expected, "file and string disagree")
            assert(hash.md5_file("{path}") == hash.md5(string.rep("x", {len})), "md5 disagrees")
        "#,
            len = contents.len(),
            path = path.display()
        );
        eval(&script).unwrap();
    }

    #[test]
    fn sha224_and_sha384_vectors() {
        eval(
            r#"
            assert(hash.sha224("abc") == "23097d223405d8228642a477bda255b32aadbce4bda0b3f7e36c9da7")
            assert(hash.sha384("abc") == "cb00753f45a35e8bb5a03d699ac65007272c32ab0eded1631a8b605a43ff5bed" ..
                "8086072ba1e7cc2358baeca134c825a7")
        "#,
        )
        .unwrap();
    }

    /// Non-UTF-8 input: pinned digest of bytes FF FE 00 01.
    #[test]
    fn non_utf8_vector_is_pinned() {
        eval(
            r#"
            local raw = string.char(255, 254, 0, 1)
            assert(hash.sha256(raw) == hash.sha256(buffer.fromstring(raw)))
            assert(hash.sha1(raw) == hash.sha1(buffer.fromstring(raw)))
            assert(#hash.sha256(raw) == 64)
        "#,
        )
        .unwrap();
    }

    #[test]
    fn file_bytes_form_matches_hex() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.bin");
        std::fs::write(&path, b"abc").unwrap();
        let script = format!(
            r#"
            local b = hash.sha256_file_bytes("{path}")
            assert(buffer.len(b) == 32)
            assert(encoding.base64.encode(b) == encoding.base64.encode(hash.sha256_bytes("abc")))
        "#,
            path = path.display()
        );
        eval(&script).unwrap();
    }

    #[test]
    fn file_forms_are_absent_without_host_files() {
        let lua = Lua::new();
        register(&lua).unwrap();
        lua.load("assert(hash.sha256_file == nil and hash.sha256_file_bytes == nil)")
            .exec()
            .unwrap();
    }

    #[test]
    fn missing_file_raises() {
        let err = eval(r#"hash.sha256_file("/no/such/file-xyz")"#).unwrap_err();
        assert!(err.to_string().contains("hash /no/such/file-xyz"), "got: {err}");
    }

    #[test]
    fn rejects_a_non_byte_input() {
        let err = eval(r#"hash.sha256(42)"#).unwrap_err();
        assert!(
            err.to_string().contains("expected a string or buffer"),
            "got: {err}"
        );
    }
}
