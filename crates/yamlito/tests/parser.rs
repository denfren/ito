use insta::assert_snapshot;

#[test]
fn cst_snapshots() {
    insta::glob!("yaml/**/*.yml", |path| {
        let source = std::fs::read_to_string(path).unwrap();
        let tree = yamlito::parse(&source).expect("parse failed");
        assert_snapshot!(yamlito::debug::cst_string(&tree));
    });
}

#[test]
fn round_trip_emit_equals_source() {
    insta::glob!("yaml/**/*.yml", |path| {
        let source = std::fs::read_to_string(path).unwrap();
        let tree = yamlito::parse(&source).expect("parse failed");
        assert_eq!(
            tree.emit(),
            source,
            "round-trip mismatch for {}",
            path.display()
        );
    });
}

#[test]
fn ast_snapshots() {
    insta::glob!("yaml/**/*.yml", |path| {
        let source = std::fs::read_to_string(path).unwrap();
        let tree = yamlito::parse(&source).expect("parse failed");
        assert_snapshot!(yamlito::debug::ast_string(&tree));
    });
}
