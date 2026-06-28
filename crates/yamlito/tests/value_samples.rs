/// Test suite driven by the YAML Test Suite (yaml/yaml-test-suite).
///
/// Each pair in `tests/yaml-value-samples/` is a `<id>.yaml` (input, stored
/// byte-for-byte with the suite's visual conventions) and a `<id>.json`
/// (expected JSON value). The test:
///
///  1. Decodes the visual conventions in the YAML input to real bytes.
///  2. Parses with `yamlito::parse`.
///  3. Converts the parse tree to a `Yaml` value via `Yaml::from_tree` /
///     `Yaml::from_stream` (multi-document streams → array).
///  4. Compares against the expected value deserialized from the JSON file via
///     `serde_json`.
///
/// Visual convention decoding (from the suite's README):
///   ␣  (U+2423) → ASCII space
///   ↵  (U+21B5) → annotation marker for a trailing newline that is
///                 *already present* in the file (the real `\n` follows the
///                 marker). It only makes an otherwise-invisible trailing
///                 newline visible, so it is stripped, NOT turned into an
///                 extra `\n` — doubling it is wrong (e.g. two `↵` empty
///                 lines in a `|+` block are two newlines, not four).
///   ∎  (U+220E) → end-of-input marker: strip the one trailing newline
///   ←  (U+2190) → carriage return `\r`
///   ⇔  (U+21D4) → BOM (U+FEFF, encoded as UTF-8 EF BB BF)
///   Tab variants (longest first): a run of `n` em-dashes followed by `»`
///   renders one TAB whose display width is `n + 1` columns. The suite uses
///   up to 4 em-dashes (5-column tab); decode every length down to a bare `»`.
///     ————» (4×U+2014 + U+00BB) → TAB
///     ———»  (3×U+2014 + U+00BB) → TAB
///     ——»   (2×U+2014 + U+00BB) → TAB
///     —»    (1×U+2014 + U+00BB) → TAB
///     »     (U+00BB)            → TAB

fn decode_visual(src: &str) -> String {
    // Tab variants: collapse any run of em-dashes followed by `»`, plus a
    // bare `»`, into a single TAB. Strip the em-dash padding first so the
    // count of em-dashes (which only encodes visual width) doesn't matter.
    let mut s = src.to_string();
    while s.contains("—»") {
        s = s.replace("—»", "»");
    }
    let s = s
        .replace("»", "\t")
        // Other visual markers
        .replace('␣', " ")
        .replace('↵', "")
        .replace('←', "\r")
        .replace('⇔', "\u{FEFF}");

    // ∎ means "no final newline": remove the marker and strip one trailing newline.
    if s.contains('∎') {
        let mut s = s.replace('∎', "");
        if s.ends_with('\n') {
            s.pop();
        }
        s
    } else {
        s
    }
}

/// Convert a `serde_json::Value` to a `yamlito::Yaml`.
///
/// The YAML test suite's JSON expectations use `null`, `true`/`false`,
/// integers, floats, strings, arrays, and objects — all map directly.
fn json_to_yaml(v: &serde_json::Value) -> yamlito::Yaml {
    use serde_json::Value;
    use yamlito::Yaml;
    match v {
        Value::Null => Yaml::Null,
        Value::Bool(b) => Yaml::Bool(*b),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Yaml::Int(i)
            } else {
                Yaml::Float(n.as_f64().unwrap())
            }
        }
        Value::String(s) => Yaml::Str(s.clone()),
        Value::Array(items) => Yaml::Seq(items.iter().map(json_to_yaml).collect()),
        Value::Object(map) => Yaml::Map(
            map.iter()
                .map(|(k, v)| (k.clone(), json_to_yaml(v)))
                .collect(),
        ),
    }
}

/// Sort mapping keys recursively. YAML mappings are unordered, so the
/// expected JSON fixtures (which use alphabetical key order) and the
/// parser's insertion-order maps must be normalized before comparison.
fn sort_keys(v: &yamlito::Yaml) -> yamlito::Yaml {
    use yamlito::Yaml;
    match v {
        Yaml::Seq(items) => Yaml::Seq(items.iter().map(sort_keys).collect()),
        Yaml::Map(pairs) => {
            let mut sorted: Vec<(String, Yaml)> = pairs
                .iter()
                .map(|(k, v)| (k.clone(), sort_keys(v)))
                .collect();
            sorted.sort_by(|a, b| a.0.cmp(&b.0));
            Yaml::Map(sorted)
        }
        other => other.clone(),
    }
}

/// Sample IDs to exclude from the run. Empty: the whole corpus passes.
/// Add an ID here (with a note) only as a temporary quarantine for a newly
/// surfaced parser bug, and remove it once fixed.
const SKIP: &[&str] = &[];

#[test]
fn yaml_value_samples() {
    use std::path::PathBuf;

    let samples_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/yaml-value-samples");

    let mut yaml_files: Vec<PathBuf> = std::fs::read_dir(&samples_dir)
        .expect("yaml-value-samples dir missing")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("yaml"))
        .collect();
    yaml_files.sort();

    assert!(
        !yaml_files.is_empty(),
        "no .yaml files found in yaml-value-samples"
    );

    let mut passed = 0usize;
    let mut failed = 0usize;
    let mut failures: Vec<String> = Vec::new();

    for yaml_path in &yaml_files {
        let stem = yaml_path.file_stem().unwrap().to_string_lossy();
        if SKIP.contains(&stem.as_ref()) {
            continue;
        }
        let json_path = yaml_path.with_extension("json");

        let raw = std::fs::read_to_string(yaml_path)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", yaml_path.display()));
        let json_src = std::fs::read_to_string(&json_path)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", json_path.display()));

        let input = decode_visual(&raw);

        let tree = match yamlito::parse(&input) {
            Ok(t) => t,
            Err(e) => {
                failed += 1;
                failures.push(format!("{stem}: parse error: {e:?}"));
                continue;
            }
        };

        // Expected value(s) from JSON. Multi-document streams are stored as
        // newline-separated JSON values (not a JSON array), so we try parsing
        // as a single value first, then fall back to a stream of values.
        let expected: yamlito::Yaml = {
            let trimmed = json_src.trim();
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) {
                json_to_yaml(&v)
            } else {
                // Try as newline-delimited stream of JSON values.
                let docs: Result<Vec<serde_json::Value>, _> =
                    serde_json::Deserializer::from_str(trimmed)
                        .into_iter()
                        .collect();
                match docs {
                    Ok(vs) => yamlito::Yaml::Seq(vs.iter().map(json_to_yaml).collect()),
                    Err(e) => panic!("{stem}: bad JSON fixture: {e}"),
                }
            }
        };

        // For multi-doc expected (Seq from stream), use from_stream; otherwise from_tree.
        let got: yamlito::Yaml = {
            let stream_docs = yamlito::Yaml::from_stream(&tree);
            match stream_docs {
                Err(e) => {
                    failed += 1;
                    failures.push(format!("{stem}: from_stream error: {e}"));
                    continue;
                }
                Ok(docs) if docs.len() == 1 => docs.into_iter().next().unwrap(),
                Ok(docs) => yamlito::Yaml::Seq(docs),
            }
        };

        let got = sort_keys(&got);
        let expected = sort_keys(&expected);

        if got == expected {
            passed += 1;
        } else {
            failed += 1;
            failures.push(format!(
                "{stem}: value mismatch\n  expected: {expected:?}\n  got:      {got:?}"
            ));
        }
    }

    if !failures.is_empty() {
        eprintln!("\n{passed} passed, {failed} failed:\n");
        for f in &failures {
            eprintln!("  FAIL {f}\n");
        }
        panic!("{failed} yaml-value-sample(s) failed (see stderr)");
    }
}
