//! S-A5: YAML 1.2 Core schema type inference.

use crate::ast::decode::DecodeError;
use crate::ast::scalar::{Scalar, ScalarStyle};

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
}

impl Scalar {
    /// Decode this scalar to a typed `Value` using the YAML 1.2 Core
    /// schema. Only plain scalars are type-inferred; quoted and block
    /// scalars are always `Value::String`.
    pub fn to_value(&self) -> Result<Value, DecodeError> {
        // The zero-width null stand-in (and any styleless scalar) decodes
        // to null directly.
        if self.is_null() {
            return Ok(Value::Null);
        }
        let style = self.style().ok_or(DecodeError::Empty)?;
        let decoded = self.decoded()?;
        match style {
            ScalarStyle::Plain => Ok(infer_plain(&decoded)),
            _ => Ok(Value::String(decoded)),
        }
    }
}

/// Apply the YAML 1.2 Core schema to a plain scalar's decoded text.
pub(crate) fn infer_plain(s: &str) -> Value {
    if s.is_empty() {
        return Value::Null;
    }
    if is_null(s) {
        return Value::Null;
    }
    if let Some(b) = parse_bool(s) {
        return Value::Bool(b);
    }
    if let Some(n) = parse_int(s) {
        return Value::Int(n);
    }
    if let Some(f) = parse_float(s) {
        return Value::Float(f);
    }
    Value::String(s.to_string())
}

fn is_null(s: &str) -> bool {
    matches!(s, "~" | "null" | "Null" | "NULL")
}

fn parse_bool(s: &str) -> Option<bool> {
    match s {
        "true" | "True" | "TRUE" => Some(true),
        "false" | "False" | "FALSE" => Some(false),
        _ => None,
    }
}

fn parse_int(s: &str) -> Option<i64> {
    // Hex: 0x[0-9A-Fa-f]+
    if let Some(rest) = s.strip_prefix("0x") {
        if !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_hexdigit()) {
            return i64::from_str_radix(rest, 16).ok();
        }
        return None;
    }
    // Octal: 0o[0-7]+
    if let Some(rest) = s.strip_prefix("0o") {
        if !rest.is_empty() && rest.bytes().all(|b| (b'0'..=b'7').contains(&b)) {
            return i64::from_str_radix(rest, 8).ok();
        }
        return None;
    }
    // Decimal: -?(0|[1-9][0-9]*)
    let (neg, digits) = if let Some(rest) = s.strip_prefix('-') {
        (true, rest)
    } else if let Some(rest) = s.strip_prefix('+') {
        (false, rest)
    } else {
        (false, s)
    };
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    // Reject leading zeros on multi-digit ("07" is not decimal int in Core).
    if digits.len() > 1 && digits.as_bytes()[0] == b'0' {
        return None;
    }
    let magnitude: i64 = digits.parse().ok()?;
    Some(if neg { -magnitude } else { magnitude })
}

fn parse_float(s: &str) -> Option<f64> {
    // Special values.
    match s {
        ".nan" | ".NaN" | ".NAN" => return Some(f64::NAN),
        ".inf" | ".Inf" | ".INF" | "+.inf" | "+.Inf" | "+.INF" => return Some(f64::INFINITY),
        "-.inf" | "-.Inf" | "-.INF" => return Some(f64::NEG_INFINITY),
        _ => {}
    }
    // Core schema float regex (roughly):
    //   [-+]? ( \.[0-9]+ | [0-9]+ ( \.[0-9]* )? ) ( [eE] [-+]? [0-9]+ )?
    // Must contain at least one of `.` or `e`/`E` to avoid eating ints.
    let bytes = s.as_bytes();
    if bytes.is_empty() {
        return None;
    }
    let mut i = 0;
    if matches!(bytes[i], b'+' | b'-') {
        i += 1;
    }
    let int_start = i;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    let had_int = i > int_start;
    let mut had_dot = false;
    if i < bytes.len() && bytes[i] == b'.' {
        had_dot = true;
        i += 1;
        let frac_start = i;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        let had_frac = i > frac_start;
        if !had_int && !had_frac {
            return None; // just "."
        }
    } else if !had_int {
        return None;
    }
    let mut had_exp = false;
    if i < bytes.len() && matches!(bytes[i], b'e' | b'E') {
        had_exp = true;
        i += 1;
        if i < bytes.len() && matches!(bytes[i], b'+' | b'-') {
            i += 1;
        }
        let exp_start = i;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        if i == exp_start {
            return None;
        }
    }
    if i != bytes.len() {
        return None;
    }
    if !had_dot && !had_exp {
        return None; // pure int — leave for parse_int
    }
    s.parse().ok()
}

#[cfg(test)]
mod tests {
    use crate::ast::{AstNode, Node, Scalar, Stream, Value};

    fn first_scalar(src: &'static str) -> Scalar {
        let tree = crate::parse(src).expect("parse");
        let stream = Stream::cast(tree.root().clone()).expect("stream");
        let doc = stream.documents().next().expect("doc");
        let root = doc.root_node().expect("root");
        fn find(node: Node) -> Option<Scalar> {
            match node {
                Node::Scalar(s) => Some(s),
                Node::BlockMapping(m) => m.entries().next().and_then(|e| e.value()).and_then(find),
                Node::BlockSequence(s) => s.entries().next().and_then(|e| e.value()).and_then(find),
                _ => None,
            }
        }
        find(root).expect("scalar")
    }

    fn v(src: &'static str) -> Value {
        first_scalar(src).to_value().expect("value")
    }

    #[test]
    fn null_tilde() {
        assert_eq!(v("~\n"), Value::Null);
    }
    #[test]
    fn null_lower() {
        assert_eq!(v("null\n"), Value::Null);
    }
    #[test]
    fn null_cap() {
        assert_eq!(v("Null\n"), Value::Null);
    }
    #[test]
    fn null_upper() {
        assert_eq!(v("NULL\n"), Value::Null);
    }

    #[test]
    fn bool_true_lower() {
        assert_eq!(v("true\n"), Value::Bool(true));
    }
    #[test]
    fn bool_true_cap() {
        assert_eq!(v("True\n"), Value::Bool(true));
    }
    #[test]
    fn bool_true_upper() {
        assert_eq!(v("TRUE\n"), Value::Bool(true));
    }
    #[test]
    fn bool_false_lower() {
        assert_eq!(v("false\n"), Value::Bool(false));
    }
    #[test]
    fn bool_false_cap() {
        assert_eq!(v("False\n"), Value::Bool(false));
    }
    #[test]
    fn bool_false_upper() {
        assert_eq!(v("FALSE\n"), Value::Bool(false));
    }

    #[test]
    fn int_zero() {
        assert_eq!(v("0\n"), Value::Int(0));
    }
    #[test]
    fn int_positive() {
        assert_eq!(v("42\n"), Value::Int(42));
    }
    #[test]
    fn int_negative() {
        assert_eq!(v("-7\n"), Value::Int(-7));
    }
    #[test]
    fn int_hex() {
        assert_eq!(v("0x1A\n"), Value::Int(26));
    }
    #[test]
    fn int_octal() {
        assert_eq!(v("0o17\n"), Value::Int(15));
    }

    #[test]
    fn int_overflow_falls_back_to_string() {
        assert_eq!(
            v("99999999999999999999\n"),
            Value::String("99999999999999999999".into())
        );
    }

    #[test]
    fn float_decimal() {
        assert_eq!(v("1.5\n"), Value::Float(1.5));
    }
    #[test]
    fn float_negative() {
        assert_eq!(v("-0.5\n"), Value::Float(-0.5));
    }
    #[test]
    fn float_exponent() {
        assert_eq!(v("1e3\n"), Value::Float(1000.0));
    }
    #[test]
    fn float_leading_dot() {
        assert_eq!(v(".5\n"), Value::Float(0.5));
    }
    #[test]
    fn float_trailing_dot() {
        assert_eq!(v("5.\n"), Value::Float(5.0));
    }

    #[test]
    fn float_nan() {
        match v(".nan\n") {
            Value::Float(f) => assert!(f.is_nan()),
            other => panic!("expected NaN, got {other:?}"),
        }
    }

    #[test]
    fn float_inf() {
        assert_eq!(v(".inf\n"), Value::Float(f64::INFINITY));
    }

    #[test]
    fn float_neg_inf() {
        assert_eq!(v("-.inf\n"), Value::Float(f64::NEG_INFINITY));
    }

    #[test]
    fn string_bareword() {
        assert_eq!(v("hello\n"), Value::String("hello".into()));
    }
    #[test]
    fn string_dash_word() {
        assert_eq!(v("-foo\n"), Value::String("-foo".into()));
    }
    #[test]
    fn string_mixed() {
        assert_eq!(v("3abc\n"), Value::String("3abc".into()));
    }

    #[test]
    fn single_quoted_is_string() {
        assert_eq!(v("'42'\n"), Value::String("42".into()));
    }

    #[test]
    fn double_quoted_is_string() {
        assert_eq!(v("\"true\"\n"), Value::String("true".into()));
    }

    #[test]
    fn literal_block_is_string() {
        assert_eq!(v("|\n  123\n"), Value::String("123\n".into()));
    }

    #[test]
    fn plain_yes_no_not_bool() {
        // YAML 1.2 Core removed 1.1's yes/no/on/off. These stay strings.
        assert_eq!(v("yes\n"), Value::String("yes".into()));
        assert_eq!(v("No\n"), Value::String("No".into()));
        assert_eq!(v("ON\n"), Value::String("ON".into()));
        assert_eq!(v("off\n"), Value::String("off".into()));
    }

    #[test]
    fn plain_empty_is_null() {
        // `key:` followed by EOL → value is empty → null.
        let tree = crate::parse("key:\n").unwrap();
        let stream = Stream::cast(tree.root().clone()).unwrap();
        let doc = stream.documents().next().unwrap();
        let map = match doc.root_node().unwrap() {
            Node::BlockMapping(m) => m,
            _ => panic!(),
        };
        let entry = map.entries().next().unwrap();
        // Empty value now yields an explicit zero-width null node.
        let value = entry.value().expect("value node");
        match value {
            Node::Scalar(s) => {
                assert!(s.is_null());
                assert_eq!(s.to_value().unwrap(), Value::Null);
            }
            other => panic!("expected null scalar, got {other:?}"),
        }
    }
}
