//! `json` module: convert between JSON text and native Rhai values.
//!
//! - `json::parse(text)` deserializes via serde_json into a
//!   `serde_json::Value`, then converts to a Rhai `Dynamic` (maps,
//!   arrays, strings, ints/floats, bools, ()), so scripts can inspect
//!   JSON content with ordinary Rhai indexing and iteration.
//! - `json::to_string(value)` / `json::to_string_pretty(value)`
//!   serialize a Rhai value back to JSON text (compact / 2-space
//!   indented).

use rhai::{Dynamic, Engine, EvalAltResult, ImmutableString, Module};

/// Register the `json` module on `engine`.
pub fn register(engine: &mut Engine) {
    let mut module = Module::new();
    module.set_native_fn(
        "parse",
        |text: ImmutableString| -> Result<Dynamic, Box<EvalAltResult>> {
            let value: serde_json::Value = serde_json::from_str(&text)
                .map_err(|e| -> Box<EvalAltResult> { format!("json parse error: {e}").into() })?;
            rhai::serde::to_dynamic(value)
        },
    );
    module.set_native_fn(
        "to_string",
        |value: Dynamic| -> Result<String, Box<EvalAltResult>> {
            let v = to_json_value(value)?;
            serde_json::to_string(&v).map_err(|e| format!("json serialize error: {e}").into())
        },
    );
    module.set_native_fn(
        "to_string_pretty",
        |value: Dynamic| -> Result<String, Box<EvalAltResult>> {
            let v = to_json_value(value)?;
            serde_json::to_string_pretty(&v)
                .map_err(|e| format!("json serialize error: {e}").into())
        },
    );
    engine.register_static_module("json", module.into());
}

fn to_json_value(value: Dynamic) -> Result<serde_json::Value, Box<EvalAltResult>> {
    rhai::serde::from_dynamic(&value)
}
