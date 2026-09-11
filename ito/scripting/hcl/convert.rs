//! Conversion between the `hcl::edit` CST expression model and native
//! Lua values, for the `hcl::edit` cursor API. Ported from the `hrs`
//! crate. Only literal expression types convert; computed expressions
//! (`var.x`, function calls, conditionals, …) are rejected.

use super::types::HclError;
use hcl::edit::expr::{Array, Expression, Object, ObjectKey};
use mlua::{Lua, LuaSerdeExt, Value};

/// Convert an edit-CST literal expression into a native Lua value. Goes
/// through `serde_json` (via the shared serde bridge) so nested arrays and
/// objects map onto Lua tables the same way `json`/`toml` do.
pub fn hcl_to_lua(lua: &Lua, expr: &Expression) -> Result<Value, HclError> {
    let json = hcl_to_json(expr)?;
    lua.to_value(&json)
        .map_err(|e| HclError::LuaConversion(e.to_string()))
}

fn hcl_to_json(expr: &Expression) -> Result<serde_json::Value, HclError> {
    use serde_json::Value as J;
    match expr {
        Expression::Null(_) => Ok(J::Null),
        Expression::Bool(b) => Ok(J::Bool(*b.value())),
        Expression::Number(n) => {
            if let Some(i) = n.value().as_i64() {
                Ok(J::from(i))
            } else if let Some(f) = n.value().as_f64() {
                Ok(J::from(f))
            } else {
                Err(HclError::UnsupportedHclType("Number (out of range)"))
            }
        }
        Expression::String(s) => Ok(J::String(s.value().to_string())),
        Expression::Array(arr) => {
            let mut out = Vec::with_capacity(arr.len());
            for item in arr.iter() {
                out.push(hcl_to_json(item)?);
            }
            Ok(J::Array(out))
        }
        Expression::Object(obj) => {
            let mut map = serde_json::Map::new();
            for (key, value) in obj.iter() {
                let key_str = match key {
                    ObjectKey::Ident(ident) => ident.value().as_str().to_string(),
                    ObjectKey::Expression(expr) => match expr {
                        Expression::String(s) => s.value().to_string(),
                        _ => {
                            return Err(HclError::UnsupportedHclType("Object with non-string key"));
                        }
                    },
                };
                map.insert(key_str, hcl_to_json(value.expr())?);
            }
            Ok(J::Object(map))
        }
        Expression::StringTemplate(_) => Err(HclError::UnsupportedHclType("StringTemplate")),
        Expression::HeredocTemplate(_) => Err(HclError::UnsupportedHclType("HeredocTemplate")),
        Expression::Parenthesis(_) => Err(HclError::UnsupportedHclType("Parenthesis")),
        Expression::Variable(_) => Err(HclError::UnsupportedHclType("Variable")),
        Expression::Conditional(_) => Err(HclError::UnsupportedHclType("Conditional")),
        Expression::FuncCall(_) => Err(HclError::UnsupportedHclType("FuncCall")),
        Expression::Traversal(_) => Err(HclError::UnsupportedHclType("Traversal")),
        Expression::UnaryOp(_) => Err(HclError::UnsupportedHclType("UnaryOp")),
        Expression::BinaryOp(_) => Err(HclError::UnsupportedHclType("BinaryOp")),
        Expression::ForExpr(_) => Err(HclError::UnsupportedHclType("ForExpr")),
    }
}

/// Convert a native Lua value into an edit-CST expression. A raw
/// `HclExpr` (from `hcl.expr`/`hcl.ident`) passes through verbatim; any
/// other value goes through `serde_json` and is treated as a literal.
pub fn lua_to_hcl(lua: &Lua, value: Value) -> Result<Expression, HclError> {
    // A raw expression from `hcl.expr`/`hcl.ident` passes through
    // verbatim: bridge the lossy value-model `hcl::Expression` into the
    // edit CST `Expression` (lossless conversion provided by `hcl-rs`).
    if let Value::UserData(ud) = &value
        && ud.is::<super::HclExpr>()
    {
        let expr = ud
            .borrow::<super::HclExpr>()
            .map_err(|e| HclError::UnsupportedLuaType(e.to_string()))?
            .clone()
            .into_inner();
        return Ok(Expression::from(expr));
    }

    let data = crate::scripting::codec::to_data(lua, &value, "hcl write error")
        .map_err(|e| HclError::UnsupportedLuaType(e.to_string()))?;
    data_to_hcl(&data)
}

/// The shared data model as an edit-CST expression. Object keys that are not
/// identifiers are emitted quoted, never rewritten.
pub fn data_to_hcl(value: &yamlito::Yaml) -> Result<Expression, HclError> {
    use hcl::edit::expr::Null;
    use yamlito::Yaml as Y;

    match value {
        Y::Null => Ok(Expression::Null(Null.into())),
        Y::Bool(b) => Ok(Expression::from(*b)),
        Y::Int(i) => Ok(Expression::from(*i)),
        Y::Float(f) => {
            if f.is_finite() {
                Ok(Expression::from(*f))
            } else {
                Err(HclError::UnsupportedLuaType(format!("number {f}")))
            }
        }
        Y::Str(s) => Ok(Expression::from(s.as_str())),
        Y::Seq(arr) => {
            let mut hcl_arr = Array::new();
            for item in arr {
                hcl_arr.push(data_to_hcl(item)?);
            }
            Ok(Expression::from(hcl_arr))
        }
        Y::Map(map) => {
            let mut hcl_obj = Object::new();
            for (key, val) in map {
                hcl_obj.insert(super::types::new_object_key(key), data_to_hcl(val)?);
            }
            Ok(Expression::from(hcl_obj))
        }
    }
}
