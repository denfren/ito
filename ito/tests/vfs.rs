//! Regression tests for VFS change-detection behaviour and for the rooted
//! `fs.finder`'s containment guarantee.

use std::path::{Path, PathBuf};

use mlua::Lua;

use ito::scripting::vfs::Vfs;

fn tmp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ito-vfs-test-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create tmp dir");
    dir
}

fn cleanup(dir: &PathBuf) {
    let _ = std::fs::remove_dir_all(dir);
}

fn run(vfs: &Vfs, script: &str) {
    let lua = Lua::new();
    ito::scripting::vfs::register(&lua, vfs).expect("register fs");
    lua.load(script)
        .exec()
        .unwrap_or_else(|e| panic!("eval failed: {e}\nscript:\n{script}"));
}

fn eval(vfs: &Vfs, script: &str) -> String {
    let lua = Lua::new();
    ito::scripting::vfs::register(&lua, vfs).expect("register fs");
    lua.load(script)
        .eval::<String>()
        .unwrap_or_else(|e| panic!("eval failed: {e}\nscript:\n{script}"))
}

#[cfg(unix)]
fn symlink(target: &Path, link: &Path) {
    std::os::unix::fs::symlink(target, link).expect("symlink");
}

// ---------------------------------------------------------------------------
// changed()
// ---------------------------------------------------------------------------

/// Writing "" to a file that does not yet exist must be reported as changed.
#[test]
fn empty_write_to_new_file_is_changed() {
    let dir = tmp_dir("empty-write-new");
    let vfs = Vfs::new(&dir).unwrap();

    run(&vfs, r#"fs.write("new.txt", "")"#);

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

    run(&vfs, r#"fs.write("existing.txt", "")"#);

    let changed = vfs.changed().unwrap();
    assert!(changed.is_empty(), "expected no changes, got: {changed:?}");
    cleanup(&dir);
}

/// Writing non-empty content to a new file is reported as changed.
#[test]
fn nonempty_write_to_new_file_is_changed() {
    let dir = tmp_dir("nonempty-write-new");
    let vfs = Vfs::new(&dir).unwrap();

    run(&vfs, r#"fs.write("hello.txt", "hello")"#);

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

    run(&vfs, r#"fs.write("new.txt", "")"#);

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

    run(&vfs, r#"fs.write("same.txt", "")"#);

    let diff = vfs.diff().unwrap();
    assert!(diff.is_empty(), "expected empty diff, got: {diff:?}");
    cleanup(&dir);
}

// ---------------------------------------------------------------------------
// fs.finder containment
//
// The rooted finder must never name a file outside the root, and must never
// name one *by* an outside path. Every escape route gets its own test, because
// each is a different mechanism: the argument, `..`, and symlinks.
// ---------------------------------------------------------------------------

/// Build a root with an outside sibling, an inward link and three outward ones.
#[cfg(unix)]
fn escape_fixture(name: &str) -> (PathBuf, PathBuf, PathBuf) {
    let base = tmp_dir(name);
    let root = base.join("root");
    let outside = base.join("outside");
    std::fs::create_dir_all(root.join("sub")).unwrap();
    std::fs::create_dir_all(outside.join("secret")).unwrap();
    std::fs::write(root.join("a.txt"), "a").unwrap();
    std::fs::write(root.join("sub/b.txt"), "b").unwrap();
    std::fs::write(outside.join("plain.txt"), "p").unwrap();
    std::fs::write(outside.join("secret/leak.txt"), "LEAK").unwrap();
    // Outward: a directory link, a file link, and a relative one.
    symlink(&outside, &root.join("link_out"));
    symlink(&outside.join("plain.txt"), &root.join("link_file.txt"));
    symlink(Path::new("../outside"), &root.join("rel_link_out"));
    (base, root, outside)
}

/// The predicates follow links here too, so `fs.is_file` means the same thing
/// under every binary. Following cannot widen what is reachable: `resolve`
/// tests the *canonical* location, so an outward link raises like any other
/// escape instead of reporting on a file outside the root.
#[cfg(unix)]
#[test]
fn predicates_follow_symlinks_without_escaping_the_root() {
    let (base, root, _outside) = escape_fixture("predicates-follow");
    symlink(Path::new("a.txt"), &root.join("link_in.txt"));
    symlink(Path::new("sub"), &root.join("link_in_dir"));
    let vfs = Vfs::new(&root).unwrap();

    // In-root links resolve to what they name.
    for (expr, want) in [
        (r#"fs.is_file("link_in.txt")"#, "true"),
        (r#"fs.is_dir("link_in_dir")"#, "true"),
        (r#"fs.exists("link_in.txt")"#, "true"),
        (r#"fs.is_symlink("link_in.txt")"#, "true"),
        (r#"fs.is_symlink("a.txt")"#, "false"),
    ] {
        assert_eq!(
            eval(&vfs, &format!(r#"return tostring({expr})"#)),
            want,
            "{expr}"
        );
    }

    // Outward links are refused, not answered — following did not open a hole.
    for expr in [
        r#"fs.is_file("link_file.txt")"#,
        r#"fs.is_dir("link_out")"#,
        r#"fs.exists("link_out")"#,
        r#"fs.is_symlink("rel_link_out")"#,
    ] {
        let lua = Lua::new();
        ito::scripting::vfs::register(&lua, &vfs).expect("register fs");
        let err = lua
            .load(format!(r#"return tostring({expr})"#))
            .eval::<String>()
            .expect_err(&format!("{expr} must be refused"));
        assert!(
            err.to_string().contains("outside") || err.to_string().contains("escape"),
            "{expr} should be refused as an escape, got: {err}"
        );
    }

    cleanup(&base);
}

/// Nothing outside the root may appear, whatever the restriction level. The
/// levels relax ignore-file and hidden rules only — never the boundary.
#[cfg(unix)]
#[test]
fn finder_never_escapes_the_root() {
    let (base, root, _outside) = escape_fixture("finder-escape");
    let vfs = Vfs::new(&root).unwrap();

    for chain in [
        "fs.finder()",
        "fs.finder():unrestricted()",
        "fs.finder():hidden()",
        "fs.finder():no_ignore()",
    ] {
        let out = eval(
            &vfs,
            &format!(r#"return table.concat({chain}:find(), ",")"#),
        );
        for entry in out.split(',').filter(|s| !s.is_empty()) {
            assert!(
                !entry.starts_with('/') && !entry.starts_with(".."),
                "{chain} returned an out-of-root path {entry:?}"
            );
            for leaked in ["outside", "secret", "leak", "plain"] {
                assert!(
                    !entry.contains(leaked),
                    "{chain} leaked {leaked:?} via {entry:?}"
                );
            }
        }
    }
    cleanup(&base);
}

/// The rooted finder takes no start directory, so an argument — absolute, `..`,
/// or an outside path — cannot redirect the walk. Extra arguments are ignored
/// rather than honoured, which is what makes the root the only possible start.
#[cfg(unix)]
#[test]
fn finder_start_directory_cannot_be_redirected() {
    let (base, root, outside) = escape_fixture("finder-start-arg");
    let vfs = Vfs::new(&root).unwrap();

    let expected = eval(&vfs, r#"return table.concat(fs.finder():find(), ",")"#);
    for arg in [
        "\"/\"".to_string(),
        "\"/etc\"".to_string(),
        "\"..\"".to_string(),
        "\"../outside\"".to_string(),
        format!("{:?}", outside.to_str().unwrap()),
    ] {
        let out = eval(
            &vfs,
            &format!(r#"return table.concat(fs.finder({arg}):find(), ",")"#),
        );
        assert_eq!(
            out, expected,
            "fs.finder({arg}) must still walk the root and nothing else"
        );
    }
    cleanup(&base);
}

/// `iq`/`ied` are not symlink-aware: a link is the thing it points at, so an
/// in-root directory link is descended into. The boundary is unaffected, since
/// containment is tested on the resolved location.
#[cfg(unix)]
#[test]
fn finder_resolves_inward_links_but_refuses_outward_ones() {
    let (base, root, _outside) = escape_fixture("finder-inward-links");
    symlink(Path::new("sub"), &root.join("link_in"));
    let vfs = Vfs::new(&root).unwrap();

    let out = eval(&vfs, r#"return table.concat(fs.finder():find(), ",")"#);
    let entries: Vec<&str> = out.split(',').filter(|s| !s.is_empty()).collect();

    // Resolved: the link is walked as the directory it names.
    assert!(
        entries.contains(&"link_in/b.txt"),
        "an inward directory link should be descended into, got {entries:?}"
    );
    // Refused: every outward link is dropped, link and target alike.
    for outward in ["link_out", "rel_link_out", "link_file.txt"] {
        assert!(
            !entries.iter().any(|e| e.starts_with(outward)),
            "outward link {outward:?} must not appear, got {entries:?}"
        );
    }
    cleanup(&base);
}

/// A link pointing at its own ancestor must terminate rather than spin — which
/// matters more now that the rooted walk follows links by default.
#[cfg(unix)]
#[test]
fn finder_terminates_on_a_symlink_cycle() {
    let (base, root, _outside) = escape_fixture("finder-cycle");
    symlink(Path::new(".."), &root.join("sub/up"));
    let vfs = Vfs::new(&root).unwrap();

    let out = eval(&vfs, r#"return tostring(fs.finder():count())"#);
    assert!(
        out.parse::<usize>().is_ok(),
        "a cycle must terminate with a count, got {out:?}"
    );
    cleanup(&base);
}

/// `:follow()` is absent on the rooted finder: links are already resolved, so
/// there is nothing for it to switch on. Absence is the feature test.
#[test]
fn rooted_finder_has_no_follow_method() {
    let dir = tmp_dir("finder-no-follow");
    let vfs = Vfs::new(&dir).unwrap();

    let out = eval(&vfs, r#"return tostring(fs.finder().follow)"#);
    assert_eq!(out, "nil", "rooted finder must not expose :follow()");
    cleanup(&dir);
}
