//! `toml` module: convert between TOML text and native Rhai values.
//!
//! - `toml::parse(text)` deserializes via the `toml` crate into a
//!   `toml::Value`, then converts to a Rhai `Dynamic` (maps, arrays,
//!   strings, ints/floats, bools; datetimes become strings), so scripts
//!   can inspect TOML content with ordinary Rhai indexing and iteration.
//! - `toml::to_string(value)` / `toml::to_string_pretty(value)`
//!   serialize a Rhai value back to TOML text. The top-level value must
//!   be a map (TOML has no top-level array/scalar form).

use rhai::{Dynamic, Engine, EvalAltResult, ImmutableString, Module};

/// Register the `toml` module on `engine`.
pub fn register(engine: &mut Engine) {
    let mut module = Module::new();
    module.set_native_fn(
        "parse",
        |text: ImmutableString| -> Result<Dynamic, Box<EvalAltResult>> {
            let value: toml::Value = toml::from_str(&text)
                .map_err(|e| -> Box<EvalAltResult> { format!("toml parse error: {e}").into() })?;
            rhai::serde::to_dynamic(value)
        },
    );
    module.set_native_fn(
        "to_string",
        |value: Dynamic| -> Result<String, Box<EvalAltResult>> {
            let v = to_toml_value(value)?;
            toml::to_string(&v).map_err(|e| format!("toml serialize error: {e}").into())
        },
    );
    module.set_native_fn(
        "to_string_pretty",
        |value: Dynamic| -> Result<String, Box<EvalAltResult>> {
            let v = to_toml_value(value)?;
            toml::to_string_pretty(&v).map_err(|e| format!("toml serialize error: {e}").into())
        },
    );
    engine.register_static_module("toml", module.into());
}

fn to_toml_value(value: Dynamic) -> Result<toml::Value, Box<EvalAltResult>> {
    rhai::serde::from_dynamic(&value)
}
