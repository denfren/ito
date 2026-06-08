//! Conversion between the `hcl::edit` CST expression model and native
//! Rhai `Dynamic` values, for the `hcl::edit` cursor API. Ported from
//! the `hrs` crate. Only literal expression types convert; computed
//! expressions (`var.x`, function calls, conditionals, …) are rejected.

use super::types::HclError;
use hcl::edit::Ident;
use hcl::edit::expr::{Array, Expression, Object, ObjectKey};
use rhai::Dynamic;

pub fn hcl_to_rhai(expr: &Expression) -> Result<Dynamic, HclError> {
    match expr {
        Expression::Null(_) => Ok(Dynamic::UNIT),
        Expression::Bool(b) => Ok(Dynamic::from(*b.value())),
        Expression::Number(n) => {
            if let Some(i) = n.value().as_i64() {
                Ok(Dynamic::from(i))
            } else if let Some(f) = n.value().as_f64() {
                Ok(Dynamic::from(f))
            } else {
                Err(HclError::UnsupportedHclType("Number (out of range)"))
            }
        }
        Expression::String(s) => Ok(Dynamic::from(s.value().to_string())),
        Expression::Array(arr) => {
            let mut rhai_arr = rhai::Array::new();
            for item in arr.iter() {
                rhai_arr.push(hcl_to_rhai(item)?);
            }
            Ok(Dynamic::from(rhai_arr))
        }
        Expression::Object(obj) => {
            let mut map = rhai::Map::new();
            for (key, value) in obj.iter() {
                let key_str = match key {
                    ObjectKey::Ident(ident) => ident.value().as_str().into(),
                    ObjectKey::Expression(expr) => match expr {
                        Expression::String(s) => s.value().into(),
                        _ => {
                            return Err(HclError::UnsupportedHclType("Object with non-string key"));
                        }
                    },
                };
                map.insert(key_str, hcl_to_rhai(value.expr())?);
            }
            Ok(Dynamic::from(map))
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

pub fn rhai_to_hcl(value: Dynamic) -> Result<Expression, HclError> {
    use hcl::edit::expr::Null;

    // A raw expression from `hcl::expr`/`hcl::ident` passes through
    // verbatim: bridge the lossy value-model `hcl::Expression` into the
    // edit CST `Expression` (lossless conversion provided by `hcl-rs`).
    if value.is::<super::HclExpr>() {
        return Ok(Expression::from(
            value.cast::<super::HclExpr>().into_inner(),
        ));
    }
    if value.is_unit() {
        return Ok(Expression::Null(Null.into()));
    }
    if value.is_bool() {
        return Ok(Expression::from(value.as_bool().unwrap()));
    }
    if value.is_int() {
        return Ok(Expression::from(value.as_int().unwrap()));
    }
    if value.is_float() {
        return Ok(Expression::from(value.as_float().unwrap()));
    }
    if value.is_string() {
        return Ok(Expression::from(value.into_string().unwrap().as_str()));
    }
    if value.is_array() {
        let arr: rhai::Array = value.cast();
        let mut hcl_arr = Array::new();
        for item in arr {
            hcl_arr.push(rhai_to_hcl(item)?);
        }
        return Ok(Expression::from(hcl_arr));
    }
    if value.is_map() {
        let map: rhai::Map = value.cast();
        let mut hcl_obj = Object::new();
        for (key, val) in map {
            let obj_key = ObjectKey::Ident(Ident::new_sanitized(key.as_str()).into());
            hcl_obj.insert(obj_key, rhai_to_hcl(val)?);
        }
        return Ok(Expression::from(hcl_obj));
    }

    Err(HclError::UnsupportedRhaiType(value.type_name().to_string()))
}
