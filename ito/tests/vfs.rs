//! Regression tests for VFS change-detection behaviour.

use std::path::PathBuf;

use rhai::Engine;

use ito::scripting::vfs::Vfs;

/// Create a unique temp directory for a test and return its path.
fn tmp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ito-vfs-test-{name}"));
    std::fs::create_dir_all(&dir).expect("create tmp dir");
    dir
}

fn cleanup(dir: &PathBuf) {
    let _ = std::fs::remove_dir_all(dir);
}

/// Build an engine with the `fs` module registered against `vfs` and run
/// `script`. Panics on eval error.
fn run(vfs: &Vfs, script: &str) {
    let mut engine = Engine::new();
    ito::scripting::vfs::register(&mut engine, vfs, false, false);
    engine
        .run(script)
        .unwrap_or_else(|e| panic!("eval failed: {e}\nscript:\n{script}"));
}

// ---------------------------------------------------------------------------
// changed()
// ---------------------------------------------------------------------------

/// Writing "" to a file that does not yet exist must be reported as changed.
#[test]
fn empty_write_to_new_file_is_changed() {
    let dir = tmp_dir("empty-write-new");
    let vfs = Vfs::new(&dir).unwrap();

    run(&vfs, r#"fs::write("/new.txt", "");"#);

    let changed = vfs.changed().unwrap();
    assert!(
        changed.iter().any(|p| p.ends_with("new.txt")),
        "expected new.txt in changed list, got: {changed:?}"
    );
    cleanup(&dir);
}

/// Writing "" to a file that already exists and is empty must NOT be reported.
#[test]
fn empty_write_to_existing_empty_file_is_not_changed() {
    let dir = tmp_dir("empty-write-existing");
    std::fs::write(dir.join("existing.txt"), "").unwrap();
    let vfs = Vfs::new(&dir).unwrap();

    run(&vfs, r#"fs::write("/existing.txt", "");"#);

    let changed = vfs.changed().unwrap();
    assert!(
        changed.is_empty(),
        "expected no changes, got: {changed:?}"
    );
    cleanup(&dir);
}

/// Writing non-empty content to a new file is reported as changed.
#[test]
fn nonempty_write_to_new_file_is_changed() {
    let dir = tmp_dir("nonempty-write-new");
    let vfs = Vfs::new(&dir).unwrap();

    run(&vfs, r#"fs::write("/hello.txt", "hello");"#);

    let changed = vfs.changed().unwrap();
    assert!(
        changed.iter().any(|p| p.ends_with("hello.txt")),
        "expected hello.txt in changed list, got: {changed:?}"
    );
    cleanup(&dir);
}

// ---------------------------------------------------------------------------
// diff()
// ---------------------------------------------------------------------------

/// diff() must include a new-file entry for an empty write to a non-existent path.
#[test]
fn diff_shows_empty_write_to_new_file() {
    let dir = tmp_dir("diff-empty-new");
    let vfs = Vfs::new(&dir).unwrap();

    run(&vfs, r#"fs::write("/new.txt", "");"#);

    let diff = vfs.diff().unwrap();
    assert!(
        diff.contains("new.txt"),
        "expected new.txt in diff output, got: {diff:?}"
    );
    cleanup(&dir);
}

/// diff() must be empty when writing "" over an already-empty file.
#[test]
fn diff_empty_for_no_op_write() {
    let dir = tmp_dir("diff-noop");
    std::fs::write(dir.join("same.txt"), "").unwrap();
    let vfs = Vfs::new(&dir).unwrap();

    run(&vfs, r#"fs::write("/same.txt", "");"#);

    let diff = vfs.diff().unwrap();
    assert!(diff.is_empty(), "expected empty diff, got: {diff:?}");
    cleanup(&dir);
}
