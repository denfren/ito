//! Tests for the host-filesystem `fs` (`ito`'s), in `scripting::rfs`.
//!
//! The contract is an equivalence plus a delta: every path the rooted VFS would
//! have accepted resolves to the same file here, and escapes — `..`, absolute
//! paths, outward symlinks — resolve instead of erroring.
//!
//! Some of that equivalence is only observable relative to the work directory,
//! which is the process cwd. Those tests take [`CWD_LOCK`] and restore the cwd
//! afterwards; the rest use absolute paths and need no lock.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use mlua::Lua;

/// Serializes the tests that `chdir`, which is process-wide state.
static CWD_LOCK: Mutex<()> = Mutex::new(());

/// Enter `dir` as the work directory, restoring the previous cwd on drop.
struct WorkDir {
    _guard: MutexGuard<'static, ()>,
    previous: PathBuf,
}

impl WorkDir {
    fn enter(dir: &Path) -> Self {
        let guard = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let previous = std::env::current_dir().expect("getcwd");
        std::env::set_current_dir(dir).expect("chdir");
        Self {
            _guard: guard,
            previous,
        }
    }
}

impl Drop for WorkDir {
    fn drop(&mut self) {
        let _ = std::env::set_current_dir(&self.previous);
    }
}

fn tmp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ito-rfs-test-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create tmp dir");
    // macOS puts the temp dir behind a `/var -> private/var` link, and the
    // module reports absolute paths as it finds them. Canonicalize so the
    // expectations below can be written against real paths.
    dir.canonicalize().expect("canonicalize tmp dir")
}

fn cleanup(dir: &Path) {
    let _ = std::fs::remove_dir_all(dir);
}

fn eval(script: &str) -> mlua::Result<String> {
    let lua = Lua::new();
    ito::scripting::rfs::register(&lua).expect("register fs");
    ito::scripting::path::register(&lua).expect("register path");
    lua.load(script).eval::<String>()
}

fn run(script: &str) {
    let lua = Lua::new();
    ito::scripting::rfs::register(&lua).expect("register fs");
    ito::scripting::path::register(&lua).expect("register path");
    lua.load(script)
        .exec()
        .unwrap_or_else(|e| panic!("eval failed: {e}\nscript:\n{script}"));
}

#[cfg(unix)]
fn symlink(target: &str, link: &Path) {
    std::os::unix::fs::symlink(target, link).expect("symlink");
}

// ---------------------------------------------------------------------------
// Equivalence with the rooted VFS: what worked before still works, and names
// the same file.
// ---------------------------------------------------------------------------

/// A relative path resolves against the work directory, exactly as the rooted
/// VFS resolved it against its root.
#[test]
fn relative_path_resolves_against_the_work_directory() {
    let dir = tmp_dir("relative-equivalence");
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::write(dir.join("sub/x.txt"), "content").unwrap();
    let _cwd = WorkDir::enter(&dir);

    assert_eq!(eval(r#"return fs.read("sub/x.txt")"#).unwrap(), "content");
    run(r#"fs.write("sub/y.txt", "written")"#);
    assert_eq!(
        std::fs::read_to_string(dir.join("sub/y.txt")).unwrap(),
        "written"
    );

    assert_eq!(
        eval(r#"return tostring(fs.is_dir("sub")) .. tostring(fs.exists("sub/x.txt"))"#).unwrap(),
        "truetrue"
    );

    drop(_cwd);
    cleanup(&dir);
}

/// `fs.walk()` with no argument covers exactly the work-directory tree, in the
/// `os.walk` shape the rooted VFS established.
#[test]
fn walk_without_argument_covers_the_work_directory() {
    let dir = tmp_dir("walk-default-start");
    std::fs::create_dir_all(dir.join("a/b")).unwrap();
    std::fs::write(dir.join("top.txt"), "t").unwrap();
    std::fs::write(dir.join("a/mid.txt"), "m").unwrap();
    std::fs::write(dir.join("a/b/leaf.txt"), "l").unwrap();
    let _cwd = WorkDir::enter(&dir);

    let out = eval(
        r#"
        local parts = {}
        for _, s in ipairs(fs.walk()) do
            table.insert(parts, s.path .. "[" .. table.concat(s.dirs, ",")
                .. "][" .. table.concat(s.files, ",") .. "]")
        end
        return table.concat(parts, " ")
        "#,
    )
    .expect("walk");

    // Work-directory-relative, so the walk reads the same as the rooted one:
    // the start is `.`, and everything below it is named from there.
    assert_eq!(out, ".[a][top.txt] a[b][mid.txt] a/b[][leaf.txt]");

    drop(_cwd);
    cleanup(&dir);
}

/// `fs.finder()` with no start directory covers exactly the work directory,
/// reporting the same relative paths the rooted finder does.
#[test]
fn finder_without_start_dir_covers_the_work_directory() {
    let dir = tmp_dir("finder-default-start");
    std::fs::create_dir_all(dir.join("a")).unwrap();
    std::fs::write(dir.join("one.txt"), "1").unwrap();
    std::fs::write(dir.join("a/two.txt"), "2").unwrap();
    std::fs::write(dir.join("a/skip.md"), "3").unwrap();
    let _cwd = WorkDir::enter(&dir);

    let out =
        eval(r#"return table.concat(fs.finder():ext("txt"):files():find(), ",")"#).expect("find");

    assert_eq!(out, "a/two.txt,one.txt");

    drop(_cwd);
    cleanup(&dir);
}

/// A symlink counts as what it points at, but keeps its own name: a link and
/// its target are two entries, not the same path twice.
#[cfg(unix)]
#[test]
fn finder_reports_symlink_under_its_own_name() {
    let dir = tmp_dir("symlink-identity");
    std::fs::write(dir.join("one.txt"), "one").unwrap();
    symlink("one.txt", &dir.join("link.txt"));

    let out = eval(&format!(
        r#"return table.concat(fs.finder("{}"):files():find(), ",")"#,
        dir.display()
    ))
    .expect("find");

    let d = dir.display();
    assert_eq!(
        out,
        format!("{d}/link.txt,{d}/one.txt"),
        "link and target must each appear once, under their own names"
    );
    cleanup(&dir);
}

// ---------------------------------------------------------------------------
// The delta: escapes resolve instead of erroring.
// ---------------------------------------------------------------------------

/// `..` climbs out of the work directory rather than erroring, which is the
/// whole point of the host filesystem.
#[test]
fn parent_traversal_leaves_the_work_directory() {
    let dir = tmp_dir("parent-traversal");
    std::fs::create_dir_all(dir.join("inner")).unwrap();
    std::fs::write(dir.join("outer.txt"), "OUTER").unwrap();
    let _cwd = WorkDir::enter(&dir.join("inner"));

    assert_eq!(eval(r#"return fs.read("../outer.txt")"#).unwrap(), "OUTER");
    run(r#"fs.write("../made.txt", "made")"#);
    assert_eq!(
        std::fs::read_to_string(dir.join("made.txt")).unwrap(),
        "made"
    );

    drop(_cwd);
    cleanup(&dir);
}

/// An absolute path means the host root, not the work directory. This is the
/// sharp edge of the change: under the rooted VFS `/x.txt` meant `<root>/x.txt`.
#[test]
fn absolute_path_reaches_outside_the_work_directory() {
    let dir = tmp_dir("absolute-outside");
    let outside = tmp_dir("absolute-outside-target");
    std::fs::write(outside.join("secret.txt"), "SECRET").unwrap();
    let _cwd = WorkDir::enter(&dir);

    let out = eval(&format!(
        r#"return fs.read("{}/secret.txt")"#,
        outside.display()
    ))
    .expect("read");
    assert_eq!(out, "SECRET");

    drop(_cwd);
    cleanup(&dir);
    cleanup(&outside);
}

/// A symlink resolving outside the start directory is *followed*, the inverse
/// of the rooted VFS's assertion that it is skipped.
#[cfg(unix)]
#[test]
fn traversal_follows_symlink_leaving_the_start_dir() {
    let dir = tmp_dir("symlink-outward");
    let outside = tmp_dir("symlink-outward-target");
    std::fs::write(outside.join("secret.txt"), "SECRET").unwrap();
    std::fs::write(dir.join("inside.txt"), "ok").unwrap();
    symlink(
        outside.join("secret.txt").to_str().unwrap(),
        &dir.join("escape.txt"),
    );

    let out = eval(&format!(
        r#"return table.concat(fs.finder("{}"):files():find(), ",")"#,
        dir.display()
    ))
    .expect("find");

    let d = dir.display();
    assert_eq!(
        out,
        format!("{d}/escape.txt,{d}/inside.txt"),
        "an outward link is followed, not skipped"
    );

    // ...and reading through it works, rather than being refused.
    let read = eval(&format!(
        r#"return fs.read("{}/escape.txt")"#,
        dir.display()
    ))
    .expect("read through link");
    assert_eq!(read, "SECRET");

    cleanup(&dir);
    cleanup(&outside);
}

/// A link pointing back up its own ancestry is reported but not descended
/// into, so a cycle still terminates.
#[cfg(unix)]
#[test]
fn walk_terminates_on_symlink_cycle() {
    let dir = tmp_dir("symlink-cycle");
    std::fs::create_dir_all(dir.join("d")).unwrap();
    std::fs::write(dir.join("d/f.txt"), "x").unwrap();
    symlink("..", &dir.join("d/up"));

    let out = eval(&format!(
        r#"
        local names = {{}}
        for _, step in ipairs(fs.walk("{}")) do table.insert(names, step.path) end
        return table.concat(names, "|")
        "#,
        dir.display()
    ))
    .expect("walk");

    let d = dir.display();
    assert_eq!(
        out,
        format!("{d}|{d}/d"),
        "cycle must not be descended into"
    );
    cleanup(&dir);
}

/// `fs.walk`/`fs.finder` take an explicit start directory, which may be
/// anywhere on the host.
#[test]
fn explicit_start_dir_walks_elsewhere() {
    let dir = tmp_dir("explicit-start");
    let elsewhere = tmp_dir("explicit-start-elsewhere");
    std::fs::write(elsewhere.join("far.txt"), "far").unwrap();
    let _cwd = WorkDir::enter(&dir);

    let out = eval(&format!(
        r#"return table.concat(fs.finder("{}"):ext("txt"):files():find(), ",")"#,
        elsewhere.display()
    ))
    .expect("find");
    assert_eq!(out, format!("{}/far.txt", elsewhere.display()));

    drop(_cwd);
    cleanup(&dir);
    cleanup(&elsewhere);
}

/// An absolute start directory reports absolute paths — the spelling it was
/// asked in, and the only one available for a path that may lie anywhere.
#[test]
fn list_reports_absolute_paths() {
    let dir = tmp_dir("absolute-output");
    std::fs::write(dir.join("a.txt"), "a").unwrap();
    std::fs::create_dir_all(dir.join("b")).unwrap();

    let out = eval(&format!(
        r#"return table.concat(fs.list("{}"), ",")"#,
        dir.display()
    ))
    .expect("list");

    let d = dir.display();
    assert_eq!(out, format!("{d}/a.txt,{d}/b"));
    cleanup(&dir);
}

// ---------------------------------------------------------------------------
// fs.cwd
// ---------------------------------------------------------------------------

/// `fs.cwd()` reports the work directory absolutely — the one path the
/// traversals cannot spell, since they would render it `"."`.
#[test]
fn cwd_reports_the_work_directory_absolutely() {
    let dir = tmp_dir("cwd");
    let _work = WorkDir::enter(&dir);

    let got = eval("return fs.cwd()").expect("fs.cwd");
    assert_eq!(Path::new(&got), dir.as_path());
    assert!(Path::new(&got).is_absolute(), "cwd must be absolute: {got}");

    drop(_work);
    cleanup(&dir);
}

/// It tracks the work directory rather than the directory the process started
/// in, so `-C` (which `chdir`s before the engine is built) is what it reports.
#[test]
fn cwd_follows_the_work_directory() {
    let dir = tmp_dir("cwd-follows");
    let nested = dir.join("sub");
    std::fs::create_dir_all(&nested).unwrap();
    let nested = nested.canonicalize().unwrap();

    let outer = {
        let _work = WorkDir::enter(&dir);
        eval("return fs.cwd()").expect("fs.cwd")
    };
    let inner = {
        let _work = WorkDir::enter(&nested);
        eval("return fs.cwd()").expect("fs.cwd")
    };

    assert_eq!(Path::new(&outer), dir.as_path());
    assert_eq!(Path::new(&inner), nested.as_path());
    cleanup(&dir);
}

/// The rooted VFS has no `fs.cwd` at all: there the root is a boundary rather
/// than a location, so `if fs.cwd then` is a feature test for the host model.
#[test]
fn cwd_is_absent_from_the_rooted_vfs() {
    let dir = tmp_dir("cwd-absent");
    let vfs = ito::scripting::Vfs::new(&dir).unwrap();

    let lua = Lua::new();
    ito::scripting::vfs::register(&lua, &vfs).expect("register fs");
    let got: String = lua
        .load(r#"return typeof(fs.cwd)"#)
        .eval()
        .expect("typeof fs.cwd");
    assert_eq!(got, "nil", "the rooted fs must not expose a host cwd");

    cleanup(&dir);
}

/// A path built from `fs.cwd()` feeds straight back into the disk-touching
/// functions, which is the point of returning a usable absolute path.
#[test]
fn cwd_round_trips_through_the_other_functions() {
    let dir = tmp_dir("cwd-round-trip");
    let _work = WorkDir::enter(&dir);

    run(r#"fs.write(path.join(fs.cwd(), "a.txt"), "payload")"#);
    assert_eq!(
        std::fs::read_to_string(dir.join("a.txt")).unwrap(),
        "payload"
    );

    // Read back through the absolute form, and confirm the traversal still
    // reports the short spelling for the same file.
    let got = eval(r#"return fs.read(path.join(fs.cwd(), "a.txt"))"#).expect("read");
    assert_eq!(got, "payload");
    let listed = eval(r#"return table.concat(fs.glob("**/*.txt"), ",")"#).expect("glob");
    assert_eq!(listed, "a.txt");

    drop(_work);
    cleanup(&dir);
}

// ---------------------------------------------------------------------------
// fs.canonicalize
// ---------------------------------------------------------------------------

/// It resolves a relative path against the work directory, collapses `.`/`..`,
/// and answers absolutely — so the result is directly comparable with
/// `fs.cwd()`, which is what asking for it is usually for.
#[test]
fn canonicalize_resolves_against_the_work_directory() {
    let dir = tmp_dir("canonicalize");
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::write(dir.join("sub/a.txt"), "x").unwrap();
    let _work = WorkDir::enter(&dir);

    let got = eval(r#"return fs.canonicalize("sub/a.txt")"#).expect("canonicalize");
    assert_eq!(Path::new(&got), dir.join("sub/a.txt").as_path());

    let dotted = eval(r#"return fs.canonicalize("./sub/../sub/a.txt")"#).expect("canonicalize");
    assert_eq!(Path::new(&dotted), dir.join("sub/a.txt").as_path());

    drop(_work);
    cleanup(&dir);
}

/// Unlike the traversals — which report a symlink under its own name — this
/// resolves the link, that being the whole point of asking.
#[test]
fn canonicalize_resolves_symlinks() {
    let dir = tmp_dir("canonicalize-link");
    std::fs::write(dir.join("real.txt"), "x").unwrap();
    symlink("real.txt", &dir.join("link.txt"));
    let _work = WorkDir::enter(&dir);

    let got = eval(r#"return fs.canonicalize("link.txt")"#).expect("canonicalize");
    assert_eq!(Path::new(&got), dir.join("real.txt").as_path());

    drop(_work);
    cleanup(&dir);
}

/// A path that does not exist is an error, not a lexical answer: resolving a
/// symlink means reading it, and there is nothing to read.
#[test]
fn canonicalize_errors_on_a_missing_path() {
    let dir = tmp_dir("canonicalize-missing");
    let _work = WorkDir::enter(&dir);

    let err = eval(r#"return fs.canonicalize("nope.txt")"#).expect_err("must fail");
    let msg = err.to_string();
    assert!(
        msg.contains("canonicalize") && msg.contains("nope.txt"),
        "error should name the operation and the path: {msg}"
    );

    drop(_work);
    cleanup(&dir);
}

/// Host-only, for the same reason `fs.cwd()` is: under the rooted model the
/// root is a boundary rather than a location, so there is no host path to hand
/// back. `if fs.canonicalize then` is a feature test.
#[test]
fn canonicalize_is_absent_from_the_rooted_vfs() {
    let dir = tmp_dir("canonicalize-absent");
    let vfs = ito::scripting::Vfs::new(&dir).unwrap();

    let lua = Lua::new();
    ito::scripting::vfs::register(&lua, &vfs).expect("register fs");
    let got: String = lua
        .load(r#"return typeof(fs.canonicalize)"#)
        .eval()
        .expect("typeof fs.canonicalize");
    assert_eq!(got, "nil", "the rooted fs must not resolve host paths");

    cleanup(&dir);
}

/// The pairing the two additions exist for: canonicalize a path, then express
/// it relative to the work directory with `path.relative_to`.
#[test]
fn canonicalize_pairs_with_relative_to() {
    let dir = tmp_dir("canonicalize-relative");
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::write(dir.join("sub/a.txt"), "x").unwrap();
    let _work = WorkDir::enter(&dir);

    let got = eval(
        r#"return path.relative_to(fs.canonicalize("./sub/a.txt"), fs.cwd()) or "not underneath""#,
    )
    .expect("relative_to");
    assert_eq!(got, "sub/a.txt");

    let outside =
        eval(r#"return path.relative_to(fs.canonicalize("/"), fs.cwd()) or "not underneath""#)
            .expect("relative_to");
    assert_eq!(outside, "not underneath");

    drop(_work);
    cleanup(&dir);
}

// ---------------------------------------------------------------------------
// Symlink predicates and the link API
// ---------------------------------------------------------------------------

/// The predicates follow links, matching what the traversals do and what
/// `fs.read` will actually get: a link to a file *is* a file.
#[test]
#[cfg(unix)]
fn predicates_follow_symlinks() {
    let dir = tmp_dir("predicates-follow");
    std::fs::write(dir.join("real.txt"), "x").unwrap();
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    symlink("real.txt", &dir.join("link_file"));
    symlink("sub", &dir.join("link_dir"));
    let _work = WorkDir::enter(&dir);

    for (expr, want) in [
        (r#"fs.is_file("link_file")"#, "true"),
        (r#"fs.is_dir("link_file")"#, "false"),
        (r#"fs.is_dir("link_dir")"#, "true"),
        (r#"fs.is_file("link_dir")"#, "false"),
        (r#"fs.exists("link_file")"#, "true"),
        // ...and the link is still identifiable as one.
        (r#"fs.is_symlink("link_file")"#, "true"),
        (r#"fs.is_symlink("real.txt")"#, "false"),
        // The composition the pair exists for: a real dir, not a link to one.
        (
            r#"fs.is_dir("link_dir") and not fs.is_symlink("link_dir")"#,
            "false",
        ),
        (r#"fs.is_dir("sub") and not fs.is_symlink("sub")"#, "true"),
    ] {
        let got = eval(&format!(r#"return tostring({expr})"#)).expect(expr);
        assert_eq!(got, want, "{expr}");
    }

    drop(_work);
    cleanup(&dir);
}

/// A dangling link is the one case where `is_symlink` and `exists` disagree:
/// the entry is there, its target is not.
#[test]
#[cfg(unix)]
fn a_dangling_symlink_exists_as_a_link_only() {
    let dir = tmp_dir("predicates-dangling");
    symlink("nowhere.txt", &dir.join("dangling"));
    let _work = WorkDir::enter(&dir);

    let got = eval(
        r#"return tostring(fs.is_symlink("dangling")) .. " " .. tostring(fs.exists("dangling"))
             .. " " .. tostring(fs.is_file("dangling"))"#,
    )
    .expect("dangling");
    assert_eq!(got, "true false false");

    // ...and it is still readable *as a link*, which is what `readlink` is for.
    let target = eval(r#"return fs.readlink("dangling")"#).expect("readlink");
    assert_eq!(target, "nowhere.txt");

    drop(_work);
    cleanup(&dir);
}

/// `fs.readlink` gives the target verbatim — one hop, unresolved, relative if
/// that is how it was written. `fs.canonicalize` is the resolving counterpart.
#[test]
#[cfg(unix)]
fn readlink_reports_the_target_verbatim() {
    let dir = tmp_dir("readlink");
    std::fs::write(dir.join("real.txt"), "x").unwrap();
    symlink("real.txt", &dir.join("rel"));
    symlink(dir.join("real.txt").to_str().unwrap(), &dir.join("abs"));
    let _work = WorkDir::enter(&dir);

    assert_eq!(
        eval(r#"return fs.readlink("rel")"#).expect("rel"),
        "real.txt"
    );
    assert_eq!(
        Path::new(&eval(r#"return fs.readlink("abs")"#).expect("abs")),
        dir.join("real.txt").as_path()
    );

    // A non-link has no target to name.
    let err = eval(r#"return fs.readlink("real.txt")"#).expect_err("must fail");
    let msg = err.to_string();
    assert!(
        msg.contains("readlink") && msg.contains("real.txt"),
        "error should name the operation and the path: {msg}"
    );

    drop(_work);
    cleanup(&dir);
}

/// `fs.symlink(target, path)` takes its arguments `ln -s`-style, creates the
/// parents of `path`, and stores `target` without resolving it — so a link may
/// be created before the thing it points at.
#[test]
#[cfg(unix)]
fn symlink_creates_links_target_first() {
    let dir = tmp_dir("symlink-create");
    std::fs::write(dir.join("real.txt"), "hi").unwrap();
    let _work = WorkDir::enter(&dir);

    eval(r#"fs.symlink("real.txt", "link"); return """#).expect("symlink");
    assert_eq!(
        std::fs::read_link(dir.join("link")).unwrap(),
        Path::new("real.txt")
    );
    // Reading through it reaches the target's contents.
    assert_eq!(eval(r#"return fs.read("link")"#).expect("read"), "hi");

    // Parents are created, as `fs.write` does.
    eval(r#"fs.symlink("../../real.txt", "a/b/up"); return """#).expect("nested");
    assert!(dir.join("a/b/up").is_symlink());
    assert_eq!(eval(r#"return fs.read("a/b/up")"#).expect("read up"), "hi");

    // A target that does not exist yet is allowed: dangling now, valid later.
    eval(r#"fs.symlink("later.txt", "early"); return """#).expect("dangling");
    assert_eq!(
        eval(r#"return tostring(fs.exists("early"))"#).unwrap(),
        "false"
    );
    std::fs::write(dir.join("later.txt"), "now").unwrap();
    assert_eq!(
        eval(r#"return fs.read("early")"#).expect("read later"),
        "now"
    );

    // An occupied path is an error rather than a silent replace.
    let err = eval(r#"fs.symlink("real.txt", "link"); return """#).expect_err("must fail");
    assert!(
        err.to_string().contains("symlink"),
        "error should name the operation: {err}"
    );

    drop(_work);
    cleanup(&dir);
}

/// `readlink`/`symlink` are host-only, like `fs.cwd`/`fs.canonicalize`: the
/// rooted model has writes it must be able to preview and a boundary to keep.
/// `fs.is_symlink` is *not* host-only — it is a predicate, not a capability.
#[test]
fn the_link_api_is_absent_from_the_rooted_vfs() {
    let dir = tmp_dir("link-api-absent");
    let vfs = ito::scripting::Vfs::new(&dir).unwrap();

    let lua = Lua::new();
    ito::scripting::vfs::register(&lua, &vfs).expect("register fs");
    let got: String = lua
        .load(
            r#"return typeof(fs.readlink) .. " " .. typeof(fs.symlink)
                 .. " " .. typeof(fs.is_symlink)"#,
        )
        .eval()
        .expect("typeof");
    assert_eq!(got, "nil nil function");

    cleanup(&dir);
}

// ---------------------------------------------------------------------------
// The write half, unrooted.
// ---------------------------------------------------------------------------

/// `fs.mkdir` makes parents and is idempotent; `fs.remove_all` is recursive and
/// treats an absent path as a no-op.
#[test]
fn mkdir_and_remove_all_round_trip() {
    let dir = tmp_dir("mkdir-remove_all");
    let d = dir.display();

    run(&format!(r#"fs.mkdir("{d}/a/b/c") fs.mkdir("{d}/a/b/c")"#));
    assert!(dir.join("a/b/c").is_dir());

    run(&format!(
        r#"fs.write("{d}/a/b/c/x.txt", "x") fs.remove_all("{d}/a") fs.remove_all("{d}/gone")"#
    ));
    assert!(!dir.join("a").exists(), "remove_all must remove the tree");
    cleanup(&dir);
}

/// `fs.mkdir`/`fs.rename`/`fs.copy` reach outside the work directory, where the
/// rooted VFS would have refused.
#[test]
fn write_operations_reach_outside_the_work_directory() {
    let dir = tmp_dir("write-outside");
    let outside = tmp_dir("write-outside-target");
    std::fs::write(dir.join("src.txt"), "payload").unwrap();
    let _cwd = WorkDir::enter(&dir);

    let o = outside.display();
    run(&format!(
        r#"
        fs.mkdir("{o}/made/deep")
        fs.copy("src.txt", "{o}/made/copied.txt")
        fs.rename("{o}/made/copied.txt", "{o}/made/deep/moved.txt")
        "#
    ));

    assert!(outside.join("made/deep").is_dir());
    assert!(!outside.join("made/copied.txt").exists());
    assert_eq!(
        std::fs::read_to_string(outside.join("made/deep/moved.txt")).unwrap(),
        "payload"
    );

    drop(_cwd);
    cleanup(&dir);
    cleanup(&outside);
}

/// `fs.copy` on a directory copies the tree, materializing every link as what
/// it stands for — including one pointing outside, which the rooted copy left
/// behind.
#[cfg(unix)]
#[test]
fn copy_directory_recurses_and_resolves_outward_links() {
    let dir = tmp_dir("copy-tree");
    let outside = tmp_dir("copy-tree-target");
    std::fs::write(outside.join("secret.txt"), "SECRET").unwrap();
    std::fs::create_dir_all(dir.join("src/a")).unwrap();
    std::fs::write(dir.join("src/one.txt"), "one").unwrap();
    std::fs::write(dir.join("src/a/two.txt"), "two").unwrap();
    symlink("one.txt", &dir.join("src/flink.txt"));
    symlink("a", &dir.join("src/dlink"));
    symlink(
        outside.join("secret.txt").to_str().unwrap(),
        &dir.join("src/outward.txt"),
    );

    let d = dir.display();
    run(&format!(r#"fs.copy("{d}/src", "{d}/dst")"#));

    assert_eq!(
        std::fs::read_to_string(dir.join("dst/one.txt")).unwrap(),
        "one"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("dst/a/two.txt")).unwrap(),
        "two"
    );
    // A link to a file arrives as the file's contents...
    assert_eq!(
        std::fs::read_to_string(dir.join("dst/flink.txt")).unwrap(),
        "one"
    );
    // ...and a link to a directory as that directory's tree.
    assert_eq!(
        std::fs::read_to_string(dir.join("dst/dlink/two.txt")).unwrap(),
        "two"
    );
    // The outward link contributes its target's contents too -- the delta from
    // the rooted copy, which skipped it.
    assert_eq!(
        std::fs::read_to_string(dir.join("dst/outward.txt")).unwrap(),
        "SECRET"
    );

    cleanup(&dir);
    cleanup(&outside);
}

/// Copying a directory into itself would never terminate, so it is refused
/// before anything is created.
#[test]
fn copy_directory_into_itself_is_refused() {
    let dir = tmp_dir("copy-self");
    std::fs::create_dir_all(dir.join("d")).unwrap();
    std::fs::write(dir.join("d/f.txt"), "x").unwrap();

    let d = dir.display();
    let err = eval(&format!(r#"fs.copy("{d}/d", "{d}/d/inner") return "" "#))
        .expect_err("copying into itself must fail");
    assert!(
        err.to_string().contains("inside the source"),
        "unexpected error: {err}"
    );
    assert!(!dir.join("d/inner").exists(), "nothing should be created");
    cleanup(&dir);
}

// ---------------------------------------------------------------------------
// How reported paths are spelled
// ---------------------------------------------------------------------------

/// A traversal that stays inside the work directory reports relative paths —
/// the same spelling the rooted VFS uses, and what `ls` would print.
#[test]
fn traversals_report_relative_paths() {
    let dir = tmp_dir("out-relative");
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::write(dir.join("a.txt"), "a").unwrap();
    std::fs::write(dir.join("sub/b.txt"), "b").unwrap();
    let _cwd = WorkDir::enter(&dir);

    let out = eval(r#"return table.concat(fs.glob("**/*.txt"), ",")"#).expect("glob");
    assert_eq!(out, "a.txt,sub/b.txt");

    let listed = eval(r#"return table.concat(fs.list("."), ",")"#).expect("list");
    assert_eq!(listed, "a.txt,sub");

    drop(_cwd);
    cleanup(&dir);
}

/// An absolute start directory keeps its absolute spelling, and a `"."` one
/// reports bare descents — each listing answering in the terms it was asked in.
#[test]
fn paths_outside_the_work_directory_stay_absolute() {
    let dir = tmp_dir("out-absolute");
    std::fs::create_dir_all(dir.join("inside")).unwrap();
    let _cwd = WorkDir::enter(&dir);

    // `/` is never under a temp work directory.
    let out = eval(r#"return fs.list("/")[1]"#).expect("list /");
    assert!(
        out.starts_with('/'),
        "a path outside the work directory should be absolute, got {out:?}"
    );

    // The work directory itself, reached from outside, is `.` rather than "".
    let listed = eval(r#"return table.concat(fs.list("."), ",")"#).expect("list");
    assert_eq!(listed, "inside");

    drop(_cwd);
    cleanup(&dir);
}

/// Shortening is component-wise, so a sibling whose name merely *starts with*
/// the work directory's is not mistaken for a child and mangled.
#[test]
fn a_sibling_with_a_shared_name_prefix_is_not_relativized() {
    let base = tmp_dir("prefix-guard");
    let work = base.join("app");
    let sibling = base.join("app2");
    std::fs::create_dir_all(&work).unwrap();
    std::fs::create_dir_all(&sibling).unwrap();
    std::fs::write(sibling.join("f.txt"), "x").unwrap();
    let _cwd = WorkDir::enter(&work);

    let s = sibling.display();
    let out = eval(&format!(r#"return table.concat(fs.list("{s}"), ",")"#)).expect("list sibling");
    assert!(
        out.ends_with("/app2/f.txt"),
        "a sibling sharing a name prefix must stay absolute, got {out:?}"
    );

    drop(_cwd);
    cleanup(&base);
}

/// With relative output, a glob is matched against the short path, so an
/// ordinary directory-anchored pattern works without a leading `**`.
#[test]
fn globs_match_the_relative_path() {
    let dir = tmp_dir("glob-anchor");
    std::fs::create_dir_all(dir.join("src/deep")).unwrap();
    std::fs::write(dir.join("src/a.rs"), "a").unwrap();
    std::fs::write(dir.join("src/deep/b.rs"), "b").unwrap();
    std::fs::write(dir.join("other.rs"), "o").unwrap();
    let _cwd = WorkDir::enter(&dir);

    let out = eval(r#"return table.concat(fs.finder():path("src/**/*.rs"):files():find(), ",")"#)
        .expect("find");
    assert_eq!(out, "src/a.rs,src/deep/b.rs");

    drop(_cwd);
    cleanup(&dir);
}

// ---------------------------------------------------------------------------
// Output spelling: one traversal, one spelling
// ---------------------------------------------------------------------------

/// A `".."` start directory reports `../`-prefixed paths, the way `find ..` and
/// `fd` do, rather than switching to absolute for the entries that happen to sit
/// outside the work directory.
///
/// The regression: the spelling used to be chosen per entry by testing each path
/// against the work directory, so this one listing returned three shapes at once
/// -- an absolute start directory, a bare `"."` for the cwd, a relative name for
/// its child -- and nothing at all relative to what was asked for.
#[test]
fn an_ascending_start_dir_reports_ascending_paths() {
    let base = tmp_dir("ascend-spelling");
    std::fs::create_dir_all(base.join("a/x")).unwrap();
    std::fs::create_dir_all(base.join("b")).unwrap();
    std::fs::write(base.join("b/y.txt"), "y").unwrap();
    let _cwd = WorkDir::enter(&base.join("a"));

    let walked = eval(
        r#"local out = {}
           for _, s in fs.walk("..") do table.insert(out, s.path) end
           return table.concat(out, ",")"#,
    )
    .expect("walk ..");
    assert_eq!(walked, "..,../a,../a/x,../b");

    // Every path is prefixed with the start directory as given -- no absolute
    // path, and no bare `.` for the work directory, in the whole listing.
    for path in walked.split(',') {
        assert!(
            path == ".." || path.starts_with("../"),
            "every entry should keep the `..` it was asked with, got {path:?}"
        );
    }

    let listed = eval(r#"return table.concat(fs.list(".."), ",")"#).expect("list ..");
    assert_eq!(listed, "../a,../b");

    let found = eval(r#"return table.concat(fs.finder(".."):ext("txt"):files():find(), ",")"#)
        .expect("find");
    assert_eq!(found, "../b/y.txt");

    drop(_cwd);
    cleanup(&base);
}

/// A start directory's spelling decides the output, so the same absolute-seeded
/// search answers the same from any working directory.
///
/// The regression this pins: because the reported spelling used to depend on the
/// cwd, and a seed glob is matched against the reported path, an absolute
/// `**`-anchored pattern silently lost exactly the entries that lay under the
/// cwd -- a plausible, non-zero, wrong count rather than an error.
#[test]
fn an_absolute_seed_finds_the_same_set_from_any_cwd() {
    let base = tmp_dir("seed-cwd-independence");
    let deep = base.join("aws/prod/phoenix/packer/resources");
    let other = base.join("aws/prod/other/packer/resources");
    std::fs::create_dir_all(&deep).unwrap();
    std::fs::create_dir_all(&other).unwrap();
    std::fs::write(deep.join("f1.json"), "1").unwrap();
    std::fs::write(other.join("f2.json"), "2").unwrap();

    let script = format!(
        r#"return table.concat(fs.finder("{b}"):path("{b}/aws/prod/**/packer/resources/*"):files():find(), ",")"#,
        b = base.display()
    );
    let expected = format!("{}/f2.json,{}/f1.json", other.display(), deep.display());

    // From the tree's root, from deep inside it, and from outside it entirely.
    for from in [base.clone(), deep.clone(), std::env::temp_dir()] {
        let _cwd = WorkDir::enter(&from);
        let got = eval(&script).expect("find");
        drop(_cwd);
        assert_eq!(
            got,
            expected,
            "the same absolute-seeded search must answer the same from {}",
            from.display()
        );
    }

    cleanup(&base);
}

// ---------------------------------------------------------------------------
// fs.finder: restriction levels, filter composition and terminals.
// ---------------------------------------------------------------------------

/// A tree with a `.gitignore`, an ignored directory and hidden entries, so each
/// restriction level has something of its own to reveal.
fn restriction_fixture(name: &str) -> PathBuf {
    let dir = tmp_dir(name);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("target")).unwrap();
    std::fs::write(dir.join(".gitignore"), "target/\n").unwrap();
    std::fs::write(dir.join("src/main.rs"), "m").unwrap();
    std::fs::write(dir.join("target/junk.rs"), "j").unwrap();
    std::fs::write(dir.join(".hidden.rs"), "h").unwrap();
    dir
}

/// The default is the smallest honest set: ignore files respected, hidden
/// entries skipped. Each level adds exactly one thing back, and `:unrestricted()`
/// is both — the `fd` model.
#[test]
fn finder_restriction_levels_widen_one_step_at_a_time() {
    let dir = restriction_fixture("finder-restrictions");
    let _cwd = WorkDir::enter(&dir);

    let find = |chain: &str| {
        eval(&format!(
            r#"return table.concat(fs.finder():ext("rs"){chain}:find(), ",")"#
        ))
        .expect("find")
    };

    assert_eq!(find(""), "src/main.rs", "default: no ignored, no hidden");
    assert_eq!(
        find(":hidden()"),
        ".hidden.rs,src/main.rs",
        ":hidden() adds dotfiles but not ignored files"
    );
    assert_eq!(
        find(":no_ignore()"),
        "src/main.rs,target/junk.rs",
        ":no_ignore() adds ignored files but not dotfiles"
    );
    assert_eq!(
        find(":unrestricted()"),
        ".hidden.rs,src/main.rs,target/junk.rs",
        ":unrestricted() adds both"
    );

    drop(_cwd);
    cleanup(&dir);
}

/// Patterns within one call OR; separate calls AND. This is the disjunction the
/// old all-globs-must-match shape could not express without a dense seed glob.
#[test]
fn finder_filters_or_within_a_call_and_and_across_calls() {
    let dir = tmp_dir("finder-or-and");
    std::fs::write(dir.join("alpha.rs"), "a").unwrap();
    std::fs::write(dir.join("beta.toml"), "b").unwrap();
    std::fs::write(dir.join("gamma.md"), "g").unwrap();
    let _cwd = WorkDir::enter(&dir);

    assert_eq!(
        eval(r#"return table.concat(fs.finder():ext("rs", "toml"):find(), ",")"#).unwrap(),
        "alpha.rs,beta.toml",
        "alternatives within one call are alternatives"
    );
    assert_eq!(
        eval(r#"return table.concat(fs.finder():ext("rs", "toml"):name("a*"):find(), ",")"#)
            .unwrap(),
        "alpha.rs",
        "a second filter narrows the first"
    );

    drop(_cwd);
    cleanup(&dir);
}

/// The empty string is the extension a file without one has, so `:ext("")`
/// selects `Makefile` and `README` — and it composes with the ordinary
/// alternatives, which is the only way to ask for "`.log` or no extension" at
/// all: a glob cannot say "no dot".
#[test]
fn finder_ext_empty_string_means_no_extension() {
    let dir = tmp_dir("finder-ext-empty");
    std::fs::write(dir.join("Makefile"), "m").unwrap();
    std::fs::write(dir.join("README"), "r").unwrap();
    std::fs::write(dir.join("app.log"), "l").unwrap();
    std::fs::write(dir.join("main.rs"), "s").unwrap();
    // A trailing dot is a name ending in `.`, not an absent extension.
    std::fs::write(dir.join("odd."), "o").unwrap();
    let _cwd = WorkDir::enter(&dir);

    assert_eq!(
        eval(r#"return table.concat(fs.finder():ext(""):files():find(), ",")"#).unwrap(),
        "Makefile,README",
        r#":ext("") matches exactly the files with no extension"#
    );
    assert_eq!(
        eval(r#"return table.concat(fs.finder():ext("log", ""):files():find(), ",")"#).unwrap(),
        "Makefile,README,app.log",
        "it ORs with the ordinary alternatives in the same call"
    );
    assert_eq!(
        eval(r#"return table.concat(fs.finder():ext("."):files():find(), ",")"#).unwrap(),
        "Makefile,README",
        r#"a bare dot is the same request, since `:ext()` strips a leading dot"#
    );

    drop(_cwd);
    cleanup(&dir);
}

/// `:name()` matches the basename and `:path()` the path — always, with no
/// auto-detection. The old finder decided from the pattern's shape, so
/// `:name("src/main.rs")` silently became a path match.
#[test]
fn finder_name_and_path_do_not_swap_meanings() {
    let dir = tmp_dir("finder-name-vs-path");
    std::fs::create_dir_all(dir.join("src/deep")).unwrap();
    std::fs::write(dir.join("src/main.rs"), "m").unwrap();
    std::fs::write(dir.join("src/deep/main.rs"), "d").unwrap();
    let _cwd = WorkDir::enter(&dir);

    assert_eq!(
        eval(r#"return tostring(#fs.finder():name("src/main.rs"):find())"#).unwrap(),
        "0",
        ":name() never matches a path, however the pattern is spelled"
    );
    assert_eq!(
        eval(r#"return table.concat(fs.finder():name("main.rs"):find(), ",")"#).unwrap(),
        "src/deep/main.rs,src/main.rs",
        ":name() matches the basename at any depth"
    );
    assert_eq!(
        eval(r#"return table.concat(fs.finder():path("src/*.rs"):find(), ",")"#).unwrap(),
        "src/main.rs",
        ":path() honours the separator, so `*` stays at one level"
    );
    assert_eq!(
        eval(r#"return table.concat(fs.finder():path("src/**/*.rs"):find(), ",")"#).unwrap(),
        "src/deep/main.rs,src/main.rs",
        "`**` is how a path pattern spans depths"
    );

    drop(_cwd);
    cleanup(&dir);
}

/// An exclude prunes: the matched directory *and its subtree* are gone. The
/// regression is a filter-only exclude, which drops `target` but still reports
/// `target/debug/junk.rs`.
#[test]
fn finder_exclude_prunes_the_whole_subtree() {
    let dir = tmp_dir("finder-exclude-prunes");
    std::fs::create_dir_all(dir.join("target/debug")).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("src/keep.rs"), "k").unwrap();
    std::fs::write(dir.join("target/junk.rs"), "j").unwrap();
    std::fs::write(dir.join("target/debug/deep.rs"), "d").unwrap();
    let _cwd = WorkDir::enter(&dir);

    assert_eq!(
        eval(r#"return table.concat(fs.finder():ext("rs"):exclude("target"):find(), ",")"#)
            .unwrap(),
        "src/keep.rs",
        "a bare name excludes the directory and everything under it"
    );
    assert_eq!(
        eval(r#"return table.concat(fs.finder():ext("rs"):exclude("**/debug/**"):find(), ",")"#)
            .unwrap(),
        "src/keep.rs,target/junk.rs",
        "a pattern with a separator excludes a region"
    );

    drop(_cwd);
    cleanup(&dir);
}

/// Depth has three methods with one meaning each: exact, floor and ceiling.
#[test]
fn finder_depth_bounds_are_separate_methods() {
    let dir = tmp_dir("finder-depth");
    std::fs::create_dir_all(dir.join("a/b/c")).unwrap();
    std::fs::write(dir.join("top.rs"), "t").unwrap();
    std::fs::write(dir.join("a/one.rs"), "1").unwrap();
    std::fs::write(dir.join("a/b/two.rs"), "2").unwrap();
    std::fs::write(dir.join("a/b/c/three.rs"), "3").unwrap();
    let _cwd = WorkDir::enter(&dir);

    let find = |chain: &str| {
        eval(&format!(
            r#"return table.concat(fs.finder():ext("rs"){chain}:find(), ",")"#
        ))
        .unwrap()
    };

    assert_eq!(find(":depth(1)"), "top.rs", ":depth(n) is the exact depth");
    assert_eq!(find(":depth(2, 3)"), "a/b/two.rs,a/one.rs", "a range");
    assert_eq!(find(":max_depth(2)"), "a/one.rs,top.rs", "a ceiling");
    assert_eq!(
        find(":min_depth(3)"),
        "a/b/c/three.rs,a/b/two.rs",
        "a floor"
    );

    drop(_cwd);
    cleanup(&dir);
}

/// `:sort()` orders and `:limit()` truncates, so "the biggest few" is one chain
/// rather than a `table.sort` afterwards.
#[test]
fn finder_sorts_and_limits() {
    let dir = tmp_dir("finder-sort");
    std::fs::write(dir.join("small.txt"), "x").unwrap();
    std::fs::write(dir.join("medium.txt"), "x".repeat(50)).unwrap();
    std::fs::write(dir.join("large.txt"), "x".repeat(500)).unwrap();
    let _cwd = WorkDir::enter(&dir);

    assert_eq!(
        eval(r#"return table.concat(fs.finder():ext("txt"):sort("size"):find(), ",")"#).unwrap(),
        "small.txt,medium.txt,large.txt",
        "size sorts ascending"
    );
    assert_eq!(
        eval(r#"return table.concat(fs.finder():ext("txt"):sort("size"):limit(2):find(), ",")"#)
            .unwrap(),
        "small.txt,medium.txt",
        "a limit truncates after the sort, not before"
    );
    assert_eq!(
        eval(r#"return table.concat(fs.finder():ext("txt"):find(), ",")"#).unwrap(),
        "large.txt,medium.txt,small.txt",
        "the default order is by path"
    );

    drop(_cwd);
    cleanup(&dir);
}

/// `:count()` and `:any()` answer without building the array.
#[test]
fn finder_count_and_any() {
    let dir = tmp_dir("finder-count-any");
    std::fs::write(dir.join("a.rs"), "a").unwrap();
    std::fs::write(dir.join("b.rs"), "b").unwrap();
    let _cwd = WorkDir::enter(&dir);

    assert_eq!(
        eval(r#"return tostring(fs.finder():ext("rs"):count())"#).unwrap(),
        "2"
    );
    assert_eq!(
        eval(r#"return tostring(fs.finder():ext("rs"):any())"#).unwrap(),
        "true"
    );
    assert_eq!(
        eval(r#"return tostring(fs.finder():ext("nope"):any())"#).unwrap(),
        "false"
    );
    assert_eq!(
        eval(r#"return tostring(fs.finder():ext("nope"):count())"#).unwrap(),
        "0"
    );

    drop(_cwd);
    cleanup(&dir);
}

/// A link counts as what it points at, so `:files()` keeps a link to a file
/// rather than dropping it for having type "symlink". `:type("symlink")` is how
/// a script asks for the links themselves.
#[cfg(unix)]
#[test]
fn finder_types_read_through_a_link() {
    let dir = tmp_dir("finder-link-types");
    std::fs::create_dir_all(dir.join("real_dir")).unwrap();
    std::fs::write(dir.join("real.txt"), "r").unwrap();
    symlink("real.txt", &dir.join("to_file.txt"));
    symlink("real_dir", &dir.join("to_dir"));
    let _cwd = WorkDir::enter(&dir);

    assert_eq!(
        eval(r#"return table.concat(fs.finder():files():find(), ",")"#).unwrap(),
        "real.txt,to_file.txt",
        "a link to a file is a file"
    );
    assert_eq!(
        eval(r#"return table.concat(fs.finder():dirs():find(), ",")"#).unwrap(),
        "real_dir,to_dir",
        "a link to a directory is a directory"
    );
    assert_eq!(
        eval(r#"return table.concat(fs.finder():type("symlink"):find(), ",")"#).unwrap(),
        "to_dir,to_file.txt",
        "the links themselves are still reachable"
    );

    drop(_cwd);
    cleanup(&dir);
}

// ---------------------------------------------------------------------------
// `fs.write_new`: the atomic create-if-absent primitive.
// ---------------------------------------------------------------------------

/// It creates the file, reports `true`, and writes the content it was given.
/// A second call to the same path reports `false` and leaves the first
/// content alone — an existing file is the answer, not an error.
#[test]
fn write_new_creates_once_and_then_reports_false() {
    let dir = tmp_dir("write-new");
    std::fs::create_dir_all(&dir).unwrap();
    let _cwd = WorkDir::enter(&dir);

    assert_eq!(
        eval(r#"return tostring(fs.write_new("lock", "first"))"#).unwrap(),
        "true",
        "the creating call wins"
    );
    assert_eq!(
        eval(r#"return tostring(fs.write_new("lock", "second"))"#).unwrap(),
        "false",
        "losing the race is a return value, not an error"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("lock")).unwrap(),
        "first",
        "the loser does not overwrite the holder's content"
    );

    drop(_cwd);
    cleanup(&dir);
}

/// `content` is optional: the no-argument form claims the name with an empty
/// file, which is all a lock needs.
#[test]
fn write_new_content_is_optional() {
    let dir = tmp_dir("write-new-empty");
    std::fs::create_dir_all(&dir).unwrap();
    let _cwd = WorkDir::enter(&dir);

    run(r#"assert(fs.write_new("lock"))"#);
    assert_eq!(std::fs::read_to_string(dir.join("lock")).unwrap(), "");

    drop(_cwd);
    cleanup(&dir);
}

/// Missing parents are created, as `fs.write` does — so a lock under a
/// not-yet-existing directory does not need an `fs.mkdir` first.
#[test]
fn write_new_creates_parents() {
    let dir = tmp_dir("write-new-parents");
    std::fs::create_dir_all(&dir).unwrap();
    let _cwd = WorkDir::enter(&dir);

    run(r#"assert(fs.write_new("run/locks/a.lock", "pid"))"#);
    assert_eq!(
        std::fs::read_to_string(dir.join("run/locks/a.lock")).unwrap(),
        "pid"
    );

    drop(_cwd);
    cleanup(&dir);
}

/// `fs.remove` is the release: after it, the name can be claimed again.
#[test]
fn write_new_round_trips_through_remove() {
    let dir = tmp_dir("write-new-release");
    std::fs::create_dir_all(&dir).unwrap();
    let _cwd = WorkDir::enter(&dir);

    run(r#"
        assert(fs.write_new("lock", "a"))
        assert(fs.write_new("lock", "b") == false)
        fs.remove("lock")
        assert(fs.write_new("lock", "b"))
    "#);
    assert_eq!(std::fs::read_to_string(dir.join("lock")).unwrap(), "b");

    drop(_cwd);
    cleanup(&dir);
}

/// A directory in the way is *not* "already exists, you lost the race": it is
/// a name `fs.remove` can never release, so a caller waiting for it would spin
/// forever. It raises instead, and says what is in the way.
#[test]
fn write_new_errors_when_a_non_file_is_in_the_way() {
    let dir = tmp_dir("write-new-dir");
    std::fs::create_dir_all(dir.join("lock")).unwrap();
    let _cwd = WorkDir::enter(&dir);

    let msg = eval(r#"return tostring(fs.write_new("lock"))"#)
        .unwrap_err()
        .to_string();
    assert!(
        msg.contains("write_new lock") && msg.contains("not a regular file"),
        "the error names the operation, the path and the reason: {msg}"
    );

    drop(_cwd);
    cleanup(&dir);
}

// ---------------------------------------------------------------------------
// `fs.finder():grep()` / `:grep_files()` -- the content terminals. Host-only,
// so they are tested here and have no VFS counterpart yet.
// ---------------------------------------------------------------------------

/// A tree with matches spread over several files, plus one file that matches
/// nothing and one that is binary.
fn grep_fixture(name: &str) -> PathBuf {
    let dir = tmp_dir(name);
    std::fs::write(dir.join("a.txt"), "alpha\nTODO(dennis) one\nbeta\n").unwrap();
    std::fs::write(dir.join("b.txt"), "TODO(ana) two\nTODO(bo) three\n").unwrap();
    std::fs::write(dir.join("c.txt"), "nothing to see\n").unwrap();
    // A NUL in the first bytes is what the searcher's binary detection keys on.
    std::fs::write(dir.join("d.bin"), b"\x00\x00TODO(zed) hidden\n").unwrap();
    dir
}

/// The basics: one entry per matching line, in walk order, carrying the path,
/// the 1-based line number and the line's text without its newline.
#[test]
fn grep_reports_every_matching_line() {
    let dir = grep_fixture("grep-lines");
    let _cwd = WorkDir::enter(&dir);

    let out = eval(
        r#"
        local parts = {}
        for _, m in fs.finder():ext("txt"):grep("TODO") do
            table.insert(parts, m.path .. ":" .. m.line .. ":" .. m.text)
        end
        return table.concat(parts, " ")
        "#,
    )
    .unwrap();
    assert_eq!(
        out, "a.txt:2:TODO(dennis) one b.txt:1:TODO(ana) two b.txt:2:TODO(bo) three",
        "flat, in path order, newline trimmed"
    );

    drop(_cwd);
    cleanup(&dir);
}

/// Capture groups come out positionally *and* by name, with group 0 absent --
/// the whole match is already `m.text`.
#[test]
fn grep_reports_capture_groups() {
    let dir = grep_fixture("grep-captures");
    let _cwd = WorkDir::enter(&dir);

    let out = eval(
        r#"
        local parts = {}
        for _, m in fs.finder():name("a.txt"):grep("TODO\\((?<who>\\w+)\\)") do
            table.insert(parts, m.captures[1] .. "/" .. m.captures.who .. "/" .. #m.captures)
        end
        return table.concat(parts, " ")
        "#,
    )
    .unwrap();
    // `#m.captures` counts the positional half only: the named key is not an
    // array index, which is what keeps `[1]` and `#` in agreement.
    assert_eq!(out, "dennis/dennis/1");

    drop(_cwd);
    cleanup(&dir);
}

/// A group-less pattern still gets a table, so indexing it is never an error on
/// the caller's side.
#[test]
fn grep_captures_is_empty_not_nil_without_groups() {
    let dir = grep_fixture("grep-nogroups");
    let _cwd = WorkDir::enter(&dir);

    assert_eq!(
        eval(
            r#"
            local m = fs.finder():name("a.txt"):grep("TODO")[1]
            return type(m.captures) .. ":" .. #m.captures .. ":" .. tostring(m.captures[1])
            "#
        )
        .unwrap(),
        "table:0:nil"
    );

    drop(_cwd);
    cleanup(&dir);
}

/// `:limit(n)` bounds *matches* for `:grep`, not files -- the one place a limit
/// changes meaning between terminals. `:find()` keeps counting paths.
#[test]
fn grep_limit_bounds_matches_while_find_limit_bounds_paths() {
    let dir = grep_fixture("grep-limit");
    let _cwd = WorkDir::enter(&dir);

    // Three matches live across two files; a limit of 2 cuts mid-file.
    assert_eq!(
        eval(r#"return tostring(#fs.finder():ext("txt"):grep("TODO"))"#).unwrap(),
        "3"
    );
    let out = eval(
        r#"
        local parts = {}
        for _, m in fs.finder():ext("txt"):limit(2):grep("TODO") do
            table.insert(parts, m.path .. ":" .. m.line)
        end
        return table.concat(parts, " ")
        "#,
    )
    .unwrap();
    assert_eq!(out, "a.txt:2 b.txt:1", "two matches, not two files' worth");

    // The same limit on `:find()` is still a path count.
    assert_eq!(
        eval(r#"return tostring(#fs.finder():ext("txt"):limit(2):find())"#).unwrap(),
        "2"
    );

    drop(_cwd);
    cleanup(&dir);
}

/// `:grep_files` is `rg -l`: the paths only, one entry per file however many
/// times it matches.
#[test]
fn grep_files_reports_each_matching_file_once() {
    let dir = grep_fixture("grep-files");
    let _cwd = WorkDir::enter(&dir);

    assert_eq!(
        eval(r#"return table.concat(fs.finder():ext("txt"):grep_files("TODO"), ",")"#).unwrap(),
        "a.txt,b.txt",
        "b.txt matches twice but is named once"
    );

    drop(_cwd);
    cleanup(&dir);
}

/// A binary file is abandoned at its first NUL, so a match inside one is never
/// reported as a line of text.
#[test]
fn grep_skips_binary_files() {
    let dir = grep_fixture("grep-binary");
    let _cwd = WorkDir::enter(&dir);

    assert_eq!(
        eval(r#"return table.concat(fs.finder():grep_files("TODO"), ",")"#).unwrap(),
        "a.txt,b.txt",
        "d.bin holds the pattern but is binary"
    );

    drop(_cwd);
    cleanup(&dir);
}

/// Directories in the hit list are skipped rather than failing the search: an
/// unfiltered finder reports them, and a survey must not die on one.
#[test]
fn grep_skips_directories_in_the_hit_list() {
    let dir = grep_fixture("grep-dirs");
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::write(dir.join("sub/e.txt"), "TODO(deep) four\n").unwrap();
    let _cwd = WorkDir::enter(&dir);

    // No `:files()`: `sub` itself is a hit, and the walk still reaches into it.
    assert_eq!(
        eval(r#"return table.concat(fs.finder():grep_files("TODO"), ",")"#).unwrap(),
        "a.txt,b.txt,sub/e.txt"
    );

    drop(_cwd);
    cleanup(&dir);
}

/// The filters compose with the content search: the grep only ever sees the
/// files the chain already matched.
#[test]
fn grep_only_searches_the_filtered_files() {
    let dir = grep_fixture("grep-filtered");
    let _cwd = WorkDir::enter(&dir);

    assert_eq!(
        eval(r#"return table.concat(fs.finder():name("b.txt"):grep_files("TODO"), ",")"#).unwrap(),
        "b.txt"
    );
    assert_eq!(
        eval(r#"return tostring(#fs.finder():name("c.txt"):grep("TODO"))"#).unwrap(),
        "0",
        "a file that does not match yields no entries, not an error"
    );

    drop(_cwd);
    cleanup(&dir);
}

/// A malformed pattern is the script's mistake and raises at the call, naming
/// the pattern -- not a silently empty result.
#[test]
fn grep_rejects_an_invalid_pattern() {
    let dir = grep_fixture("grep-badpattern");
    let _cwd = WorkDir::enter(&dir);

    let msg = eval(r#"return tostring(fs.finder():grep("("))"#)
        .unwrap_err()
        .to_string();
    assert!(
        msg.contains("grep: invalid pattern") && msg.contains('('),
        "the error names the operation and the pattern: {msg}"
    );

    drop(_cwd);
    cleanup(&dir);
}

// ---------------------------------------------------------------------------
// Review backlog (FS-*): glob prefix, finder strings, missing starts, new fns.
// ---------------------------------------------------------------------------

/// An absolute or `../` pattern starts at its literal prefix, and a
/// metacharacter in a directory name stays literal.
#[test]
fn glob_starts_at_the_literal_prefix_and_escapes_it() {
    let dir = tmp_dir("glob-prefix");
    let meta = tmp_dir("glob-meta[x]");
    std::fs::write(meta.join("m.txt"), "m").unwrap();
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::write(dir.join("sub/a.txt"), "a").unwrap();
    std::fs::write(dir.join("b.txt"), "b").unwrap();
    let d = dir.to_string_lossy().into_owned();
    let out = eval(&format!(r#"return table.concat(fs.glob("{d}/**/*.txt"), ",")"#)).unwrap();
    assert_eq!(out, format!("{d}/b.txt,{d}/sub/a.txt"));
    let _cwd = WorkDir::enter(&dir.join("sub"));
    let out = eval(r#"return table.concat(fs.glob("../*.txt"), ",")"#).unwrap();
    assert_eq!(out, "../b.txt");
    let out = eval(r#"return table.concat(fs.glob("*.txt"), ",")"#).unwrap();
    assert_eq!(out, "a.txt");
    drop(_cwd);
    // A metacharacter in the work directory's own name stays literal.
    let _cwd = WorkDir::enter(&meta);
    assert_eq!(eval(r#"return table.concat(fs.glob("*.txt"), ",")"#).unwrap(), "m.txt");
    drop(_cwd);
    cleanup(&meta);
    cleanup(&dir);
}

/// `:exclude` and `:path` see the same string, including the start prefix.
#[test]
fn finder_exclude_and_path_agree_on_the_prefix() {
    let dir = tmp_dir("finder-prefix");
    std::fs::create_dir_all(dir.join("src/skip")).unwrap();
    std::fs::write(dir.join("src/a.rs"), "").unwrap();
    std::fs::write(dir.join("src/skip/b.rs"), "").unwrap();
    let d = dir.to_string_lossy().into_owned();
    let out = eval(&format!(
        r#"return table.concat(fs.finder("{d}/src"):files():exclude("{d}/src/skip/**"):find(), ",")"#
    ))
    .unwrap();
    assert_eq!(out, format!("{d}/src/a.rs"));
    cleanup(&dir);
}

#[test]
fn missing_start_directories_raise() {
    let dir = tmp_dir("missing-start");
    let d = dir.to_string_lossy().into_owned();
    for s in [
        format!(r#"return #fs.walk("{d}/nope")"#),
        format!(r#"return fs.finder("{d}/nope"):count()"#),
        format!(r#"return #fs.list("{d}/nope")"#),
    ] {
        assert!(eval(&s).is_err(), "{s}");
    }
    cleanup(&dir);
}

#[test]
fn stat_append_chmod_and_temp() {
    let dir = tmp_dir("stat-append");
    let d = dir.to_string_lossy().into_owned();
    run(&format!(
        r#"fs.append("{d}/a.txt", "x") fs.append("{d}/a.txt", "y")
           assert(fs.read("{d}/a.txt") == "xy")
           local s = fs.stat("{d}/a.txt") assert(s.kind == "file" and s.size == 2)
           assert(fs.stat("{d}/nope") == nil and fs.stat("{d}").kind == "dir")
           fs.chmod("{d}/a.txt", 0x1A4) assert(fs.stat("{d}/a.txt").mode == 0x1A4)
           local t = fs.temp_dir("ito-t") assert(fs.is_dir(t)) fs.remove_all(t)
           local f = fs.temp_file() assert(fs.is_file(f)) fs.remove(f)"#
    ));
    cleanup(&dir);
}

#[test]
fn binary_read_write_round_trips() {
    let dir = tmp_dir("binary");
    let d = dir.to_string_lossy().into_owned();
    run(&format!(
        r#"fs.write("{d}/b", "\xff\xfe\0z") assert(fs.read("{d}/b") == "\xff\xfe\0z")"#
    ));
    assert_eq!(std::fs::read(dir.join("b")).unwrap(), [0xff, 0xfe, 0, b'z']);
    cleanup(&dir);
}

#[test]
fn remove_refuses_a_directory_and_copy_onto_itself_raises() {
    let dir = tmp_dir("remove-dir");
    std::fs::create_dir_all(dir.join("d")).unwrap();
    std::fs::write(dir.join("f"), "keep").unwrap();
    let d = dir.to_string_lossy().into_owned();
    let e = eval(&format!(r#"fs.remove("{d}/d") return "x""#)).unwrap_err().to_string();
    assert!(e.contains("is a directory"), "{e}");
    let e = eval(&format!(r#"fs.copy("{d}/f", "{d}/f") return "x""#)).unwrap_err().to_string();
    assert!(e.contains("same"), "{e}");
    assert_eq!(std::fs::read_to_string(dir.join("f")).unwrap(), "keep");
    cleanup(&dir);
}
