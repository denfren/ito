//! Property-based round-trip tests for the `yamlito` value model and
//! parser (FINDINGS.md §6).
//!
//! Two properties, both driven by *generated* data so they exercise far
//! more shapes than the static corpus can:
//!
//! 1. **Builder round-trip** — for any `Yaml` value `v`,
//!    `Yaml::from_tree(parse(v.to_yaml_string())) == v`. The value
//!    survives emit → parse → decode. Strings are drawn from a hostile
//!    alphabet (control chars, indicators, YAML 1.1 hazard words, empty),
//!    pinning the B1/B2/B3 quoting fixes and the A4 quoting policy.
//!
//! 2. **Parser emit-stability** — for YAML *text* produced by serializing
//!    a generated value, `parse(s).emit() == s`. This is the same
//!    `emit() == source` invariant the corpus checks, but over generated
//!    structures. Generating from canonical builder output keeps the
//!    input well-formed (we are not fuzzing arbitrary bytes, which would
//!    hit the known D1–D6 parser gaps).

use proptest::prelude::*;
use yamlito::Yaml;

/// Hostile string fragments: structural indicators, control characters,
/// whitespace, quote characters, and YAML 1.1 / Core-schema hazard words.
/// Any generated string is a join of these, so the quoting policy is
/// stressed at every position (leading, trailing, embedded).
const HOSTILE: &[&str] = &[
    // control characters (B1)
    "\n", "\r", "\t",
    // structural / indicators / whitespace
    ",", "]", "}", "[", "{", ":", "#", " ", "-", "?", "&", "*", "!", "|",
    ">", "'", "\"", "%", "@", "`", "\\", ": ", " #",
    // YAML 1.1 / Core-schema hazard words (A4)
    "yes", "no", "on", "off", "true", "false", "null", "Yes", "No", "On",
    "Off", "True", "False", "Null", "YES", "NO", "ON", "OFF", "TRUE",
    "FALSE", "NULL", "~",
    // number-like (A4)
    "0", "010", "42", "-7", "1.5", "1:30", "12:34:56", ".inf", ".nan",
    // plain-safe filler
    "a", "b", "hello", "key_name", "it's", "http://x",
];

/// A string assembled from 0–6 hostile fragments. The empty join yields
/// `""`, which the policy must also handle (B2).
fn hostile_string() -> impl Strategy<Value = String> {
    prop::collection::vec(prop::sample::select(HOSTILE), 0..6)
        .prop_map(|parts| parts.concat())
}

/// A finite, exactly-representable float. We avoid NaN (which breaks
/// `==`) and rely on `f64::to_string` producing a shortest round-trip
/// representation that the Core schema reads back as the same value.
fn finite_float() -> impl Strategy<Value = f64> {
    prop_oneof![
        (-1.0e9..1.0e9f64),
        Just(0.0),
        Just(-0.0),
        Just(1.5),
        Just(f64::INFINITY),
        Just(f64::NEG_INFINITY),
    ]
}

/// A leaf `Yaml` value (no nesting).
fn leaf() -> impl Strategy<Value = Yaml> {
    prop_oneof![
        Just(Yaml::Null),
        any::<bool>().prop_map(Yaml::Bool),
        any::<i64>().prop_map(Yaml::Int),
        finite_float().prop_map(Yaml::Float),
        hostile_string().prop_map(Yaml::Str),
    ]
}

/// An arbitrary `Yaml` value: leaves, plus block sequences and
/// string-keyed mappings nested up to a few levels deep. Mapping keys are
/// hostile strings (the builder only emits string keys).
fn yaml_value() -> impl Strategy<Value = Yaml> {
    leaf().prop_recursive(4, 32, 6, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..5).prop_map(Yaml::Seq),
            prop::collection::vec((hostile_string(), inner), 0..5).prop_map(Yaml::Map),
        ]
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    /// Builder round-trip: a value survives emit → parse → decode.
    /// Pins B1/B2/B3 and the A4 quoting policy.
    #[test]
    fn builder_value_round_trips(v in yaml_value()) {
        let text = v.to_yaml_string();
        let tree = yamlito::parse(&text)
            .unwrap_or_else(|e| panic!("parse failed for {text:?}: {e:?}"));
        let decoded = Yaml::from_tree(&tree)
            .unwrap_or_else(|e| panic!("from_tree failed for {text:?}: {e}"));
        prop_assert_eq!(decoded, v, "round-trip mismatch via {:?}", text);
    }

    /// Parser emit-stability on well-formed input: text emitted by the
    /// builder re-emits byte-for-byte after a parse. Guards the
    /// `emit() == source` invariant over generated structures.
    #[test]
    fn parse_emit_is_stable(v in yaml_value()) {
        let text = v.to_yaml_string();
        let tree = yamlito::parse(&text)
            .unwrap_or_else(|e| panic!("parse failed for {text:?}: {e:?}"));
        prop_assert_eq!(tree.emit(), text.clone(), "emit != source for {:?}", text);
    }
}
