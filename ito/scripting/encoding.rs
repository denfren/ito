//! Luau bindings for binary-to-text encodings, registered as the global
//! `encoding`. Only base64 for now, as `encoding.base64`.
//!
//! ```lua
//! encoding.base64.encode("hi")                  --> "aGk=" (STANDARD, padded)
//! encoding.base64.decode("aGk=")                --> "hi"
//! encoding.base64.engine():alphabet("url_safe"):padding(false):encode(data)
//! ```
//!
//! The defaults are the `base64` crate's STANDARD engine: standard alphabet,
//! padded on encode, **canonical padding required** on decode. The `engine()`
//! builder mirrors that crate's engine configuration — `alphabet`, `padding`
//! (encode-side), `decode_padding` (`"indifferent"` | `"required"` | `"none"`)
//! — and each step returns a *new* immutable builder, so a half-configured one
//! can be shared. Inputs are strings or buffers (bytes, nothing is UTF-8
//! validated); `decode` returns a string, which is 8-bit clean in Luau.
//!
//! **Pure**: bytes in, text out, registered on every binary.
//!
//! Hex is deliberately *not* a public module (D7): [`hex_encode`]/[`hex_decode`]
//! are the one internal helper `hash`, `ansible` and friends share.

use base64::alphabet::{self, Alphabet};
use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
use base64::Engine as _;
use mlua::{Lua, MetaMethod, Result as LuaResult, UserData, UserDataMethods, Value};

/// Lowercase hex of `bytes`; the shared spelling for digests and vault bodies.
pub(crate) fn hex_encode(bytes: impl AsRef<[u8]>) -> String {
    hex::encode(bytes)
}

/// Decode hex (either case); `None` when it is odd-length or not hex.
pub(crate) fn hex_decode(text: &str) -> Option<Vec<u8>> {
    hex::decode(text).ok()
}

/// The bytes behind a string or buffer argument.
fn bytes_of(value: &Value, what: &str) -> LuaResult<Vec<u8>> {
    match value {
        Value::String(s) => Ok(s.as_bytes().to_vec()),
        Value::Buffer(b) => Ok(b.to_vec()),
        other => Err(mlua::Error::runtime(format!(
            "{what}: expected a string or buffer, got {}",
            other.type_name()
        ))),
    }
}

/// One engine configuration; the builder's userdata.
#[derive(Clone)]
struct Base64 {
    alphabet: &'static Alphabet,
    alphabet_name: &'static str,
    padding: bool,
    decode_padding: DecodePaddingMode,
}

impl Base64 {
    fn standard() -> Self {
        Self {
            alphabet: &alphabet::STANDARD,
            alphabet_name: "standard",
            padding: true,
            decode_padding: DecodePaddingMode::RequireCanonical,
        }
    }

    fn engine(&self) -> GeneralPurpose {
        GeneralPurpose::new(
            self.alphabet,
            GeneralPurposeConfig::new()
                .with_encode_padding(self.padding)
                .with_decode_padding_mode(self.decode_padding),
        )
    }
}

fn alphabet_named(name: &str) -> LuaResult<(&'static Alphabet, &'static str)> {
    Ok(match name {
        "standard" => (&alphabet::STANDARD, "standard"),
        "url_safe" => (&alphabet::URL_SAFE, "url_safe"),
        "crypt" => (&alphabet::CRYPT, "crypt"),
        "bcrypt" => (&alphabet::BCRYPT, "bcrypt"),
        "imap_mutf7" => (&alphabet::IMAP_MUTF7, "imap_mutf7"),
        "bin_hex" => (&alphabet::BIN_HEX, "bin_hex"),
        other => {
            return Err(mlua::Error::runtime(format!(
                "encoding.base64: unknown alphabet '{other}' (expected standard, url_safe, \
                 crypt, bcrypt, imap_mutf7 or bin_hex)"
            )));
        }
    })
}

fn decode_padding_named(name: &str) -> LuaResult<DecodePaddingMode> {
    Ok(match name {
        "indifferent" => DecodePaddingMode::Indifferent,
        "required" => DecodePaddingMode::RequireCanonical,
        "none" => DecodePaddingMode::RequireNone,
        other => {
            return Err(mlua::Error::runtime(format!(
                "encoding.base64: unknown decode_padding '{other}' (expected indifferent, \
                 required or none)"
            )));
        }
    })
}

fn decode(lua: &Lua, engine: &Base64, text: &Value) -> LuaResult<mlua::LuaString> {
    let input = bytes_of(text, "encoding.base64.decode")?;
    let out = engine
        .engine()
        .decode(input)
        .map_err(|e| mlua::Error::runtime(format!("encoding.base64.decode: {e}")))?;
    lua.create_string(out)
}

fn encode(engine: &Base64, data: &Value) -> LuaResult<String> {
    Ok(engine.engine().encode(bytes_of(data, "encoding.base64.encode")?))
}

impl UserData for Base64 {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("alphabet", |lua, this, name: String| {
            let (alphabet, alphabet_name) = alphabet_named(&name)?;
            lua.create_userdata(Self { alphabet, alphabet_name, ..this.clone() })
        });
        methods.add_method("padding", |lua, this, on: bool| {
            lua.create_userdata(Self { padding: on, ..this.clone() })
        });
        methods.add_method("decode_padding", |lua, this, mode: String| {
            lua.create_userdata(Self { decode_padding: decode_padding_named(&mode)?, ..this.clone() })
        });
        methods.add_method("encode", |_, this, data: Value| encode(this, &data));
        methods.add_method("decode", |lua, this, text: Value| decode(lua, this, &text));
        methods.add_meta_method(MetaMethod::ToString, |_, this, ()| {
            Ok(format!(
                "base64.engine(alphabet={}, padding={})",
                this.alphabet_name, this.padding
            ))
        });
    }
}

/// Register the `encoding` global on every binary.
pub fn register(lua: &Lua) -> LuaResult<()> {
    let base64 = lua.create_table()?;
    base64.raw_set(
        "encode",
        lua.create_function(|_, data: Value| encode(&Base64::standard(), &data))?,
    )?;
    base64.raw_set(
        "decode",
        lua.create_function(|lua, text: Value| decode(lua, &Base64::standard(), &text))?,
    )?;
    base64.raw_set(
        "engine",
        lua.create_function(|_, ()| Ok(Base64::standard()))?,
    )?;
    let exports = lua.create_table()?;
    exports.raw_set("base64", base64)?;
    lua.globals().raw_set("encoding", exports)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eval(script: &str) -> LuaResult<()> {
        let lua = Lua::new();
        register(&lua)?;
        lua.load(script).exec()
    }

    #[test]
    fn default_pair_is_standard_padded() {
        eval(
            r#"
            assert(encoding.base64.encode("hi") == "aGk=")
            assert(encoding.base64.decode("aGk=") == "hi")
            assert(encoding.base64.encode(buffer.fromstring("hi")) == "aGk=")
            assert(encoding.base64.encode(string.char(251, 255)) == "+/8=")
            local raw = string.char(255, 254, 0, 1)
            assert(encoding.base64.decode(encoding.base64.encode(raw)) == raw)
        "#,
        )
        .unwrap();
    }

    #[test]
    fn default_decode_requires_padding() {
        assert!(eval(r#"encoding.base64.decode("aGk")"#).is_err());
    }

    #[test]
    fn builder_alphabet_and_padding() {
        eval(
            r#"
            local e = encoding.base64.engine():alphabet("url_safe"):padding(false):decode_padding("none")
            assert(e:encode(string.char(251, 255)) == "-_8")
            assert(e:decode("-_8") == string.char(251, 255))
            -- builders are immutable: the base keeps its settings
            local base = encoding.base64.engine()
            local _ = base:padding(false)
            assert(base:encode("hi") == "aGk=")
        "#,
        )
        .unwrap();
    }

    #[test]
    fn decode_padding_modes() {
        eval(
            r#"
            local none = encoding.base64.engine():padding(false):decode_padding("none")
            assert(none:decode("aGk") == "hi")
            assert(not pcall(none.decode, none, "aGk="))
            local any = encoding.base64.engine():decode_padding("indifferent")
            assert(any:decode("aGk") == "hi" and any:decode("aGk=") == "hi")
        "#,
        )
        .unwrap();
    }

    #[test]
    fn rejects_bad_arguments() {
        assert!(eval(r#"encoding.base64.engine():alphabet("nope")"#).is_err());
        assert!(eval(r#"encoding.base64.engine():decode_padding("nope")"#).is_err());
        assert!(eval(r#"encoding.base64.encode(42)"#).is_err());
        assert!(eval(r#"encoding.base64.decode("!!!")"#).is_err());
    }

    #[test]
    fn hex_helper_round_trips() {
        assert_eq!(hex_encode([0xde, 0xad]), "dead");
        assert_eq!(hex_decode("DEad"), Some(vec![0xde, 0xad]));
        assert_eq!(hex_decode("abc"), None);
    }
}
