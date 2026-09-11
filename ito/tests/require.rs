//! Integration tests for ito's custom `require` resolver
//! (`scripting::require`): confinement to the entry script's directory,
//! candidate ordering, `init` fallback, and the ambiguity / not-found
//! error messages.

use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use std::path::Path;
use tempfile::TempDir;

fn ito() -> Command {
    Command::cargo_bin("ito").unwrap()
}

/// Run `ito <abs script>` with the working dir (`fs`/`-C` root) set to
/// `cwd`.
fn run(cwd: &Path, script: &Path) -> Command {
    let mut cmd = ito();
    cmd.args(["-C", cwd.to_str().unwrap(), script.to_str().unwrap()]);
    cmd
}

#[test]
fn relative_require_resolves_next_to_the_requiring_script() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    fs::write(
        root.join("lib.lua"),
        r#"return { hello = function() return "root" end }"#,
    )
    .unwrap();
    fs::create_dir(root.join("sub")).unwrap();
    fs::write(
        root.join("sub/lib.lua"),
        r#"return { hello = function() return "sub" end }"#,
    )
    .unwrap();
    // The nested script requires the module next to *itself*, not the root.
    fs::write(root.join("sub/s.lua"), "print(require('./lib').hello())").unwrap();

    run(root, &root.join("sub/s.lua"))
        .assert()
        .success()
        .stdout(predicate::str::contains("sub"));
}

#[test]
fn nested_require_resolves_relative_to_the_inner_module() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    fs::write(root.join("main.lua"), "print(require('./sub/a').val())").unwrap();
    fs::write(
        root.join("b.lua"),
        r#"return { val = function() return "root-b" end }"#,
    )
    .unwrap();
    fs::create_dir(root.join("sub")).unwrap();
    // sub/a requires ./b — must find sub/b, never the root b.
    fs::write(
        root.join("sub/a.lua"),
        "return { val = require('./b').val }",
    )
    .unwrap();
    fs::write(
        root.join("sub/b.lua"),
        r#"return { val = function() return "sub-b" end }"#,
    )
    .unwrap();

    run(root, &root.join("main.lua"))
        .assert()
        .success()
        .stdout(predicate::str::contains("sub-b"));
}

#[test]
fn luau_extension_is_preferred_when_unambiguous() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    fs::write(root.join("m.luau"), r#"return { v = "luau" }"#).unwrap();
    fs::write(root.join("main.lua"), "print(require('./m').v)").unwrap();

    run(root, &root.join("main.lua"))
        .assert()
        .success()
        .stdout(predicate::str::contains("luau"));
}

#[test]
fn explicit_extension_loads_that_exact_file() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    fs::write(root.join("m.luau"), r#"return { v = "explicit" }"#).unwrap();
    fs::write(root.join("main.lua"), "print(require('./m.luau').v)").unwrap();

    run(root, &root.join("main.lua"))
        .assert()
        .success()
        .stdout(predicate::str::contains("explicit"));
}

#[test]
fn directory_with_init_is_a_module() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    fs::create_dir(root.join("pkg")).unwrap();
    fs::write(root.join("pkg/init.lua"), r#"return { name = "pkg-init" }"#).unwrap();
    fs::write(root.join("main.lua"), "print(require('./pkg').name)").unwrap();

    run(root, &root.join("main.lua"))
        .assert()
        .success()
        .stdout(predicate::str::contains("pkg-init"));
}

#[test]
fn multiple_matching_candidates_is_an_error_naming_both() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    fs::write(root.join("m.lua"), "return {}").unwrap();
    fs::write(root.join("m.luau"), "return {}").unwrap();
    fs::write(root.join("main.lua"), "require('./m')").unwrap();

    run(root, &root.join("main.lua"))
        .assert()
        .failure()
        .code(3)
        .stderr(predicate::str::contains("ambiguous module"))
        .stderr(predicate::str::contains("m.luau"))
        .stderr(predicate::str::contains("m.lua"));
}

#[test]
fn no_match_is_an_error_listing_checked_paths() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    fs::write(root.join("main.lua"), "require('./nope')").unwrap();

    run(root, &root.join("main.lua"))
        .assert()
        .failure()
        .code(3)
        .stderr(predicate::str::contains("no module found"))
        .stderr(predicate::str::contains("nope.luau"))
        .stderr(predicate::str::contains("nope.lua"));
}

#[test]
fn require_cannot_escape_the_script_root() {
    let outer = TempDir::new().unwrap();
    let secret = outer.path().join("secret.lua");
    fs::write(&secret, r#"return { leaked = true }"#).unwrap();

    let root = outer.path().join("proj");
    fs::create_dir(&root).unwrap();
    fs::write(
        root.join("escape.lua"),
        "print(require('../secret').leaked)",
    )
    .unwrap();

    // Refused at the root boundary. Luau reports the failure to climb in its
    // own words, so the guarantee under test is that the module never loads,
    // not the exact phrasing.
    run(&root, &root.join("escape.lua"))
        .assert()
        .failure()
        .code(3)
        .stdout(predicate::str::contains("true").not());
}

#[test]
fn require_root_is_independent_of_the_fs_root() {
    // Script lives in `proj`; the fs / -C root is a *different* directory.
    // require must read from `proj`, while fs writes go to the -C root.
    let proj = TempDir::new().unwrap();
    fs::write(proj.path().join("lib.lua"), r#"return { v = "from-proj" }"#).unwrap();
    fs::write(
        proj.path().join("main.lua"),
        "print(require('./lib').v)\nfs.write('out.txt', 'hi')",
    )
    .unwrap();

    let fsroot = TempDir::new().unwrap();
    run(fsroot.path(), &proj.path().join("main.lua"))
        .assert()
        .success()
        .stdout(predicate::str::contains("from-proj"));

    // fs.write landed in the -C root, not next to the script.
    assert!(fsroot.path().join("out.txt").is_file());
    assert!(!proj.path().join("out.txt").exists());
}

#[test]
fn module_is_evaluated_once_and_cached() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    // The module prints on load; requiring twice must print only once.
    fs::write(root.join("m.lua"), "print('loading m')\nreturn {}").unwrap();
    fs::write(root.join("main.lua"), "require('./m')\nrequire('./m')").unwrap();

    run(root, &root.join("main.lua"))
        .assert()
        .success()
        .stdout(predicate::function(|s: &str| {
            s.matches("loading m").count() == 1
        }));
}

#[test]
fn alias_from_luaurc_resolves_to_its_target() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    fs::create_dir(root.join("lib")).unwrap();
    fs::write(
        root.join("lib/mod.lua"),
        r#"return { id = function() return "via-alias" end }"#,
    )
    .unwrap();
    fs::write(root.join(".luaurc"), r#"{"aliases":{"lib":"./lib"}}"#).unwrap();
    fs::write(root.join("main.lua"), "print(require('@lib/mod').id())").unwrap();

    run(root, &root.join("main.lua"))
        .assert()
        .success()
        .stdout(predicate::str::contains("via-alias"));
}

#[test]
fn alias_may_name_a_module_file_directly() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    fs::create_dir(root.join("lib")).unwrap();
    fs::write(
        root.join("lib/mod.lua"),
        r#"return { id = function() return "direct" end }"#,
    )
    .unwrap();
    // The alias target is a file, so `@direct` needs no trailing path.
    fs::write(
        root.join(".luaurc"),
        r#"{"aliases":{"direct":"./lib/mod"}}"#,
    )
    .unwrap();
    fs::write(root.join("main.lua"), "print(require('@direct').id())").unwrap();

    run(root, &root.join("main.lua"))
        .assert()
        .success()
        .stdout(predicate::str::contains("direct"));
}

#[test]
fn alias_resolves_from_a_nested_module_too() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    fs::create_dir(root.join("lib")).unwrap();
    fs::write(
        root.join("lib/mod.lua"),
        r#"return { id = function() return "nested-alias" end }"#,
    )
    .unwrap();
    fs::write(root.join(".luaurc"), r#"{"aliases":{"lib":"./lib"}}"#).unwrap();
    fs::create_dir(root.join("sub")).unwrap();
    // The alias is defined at the root but used one level down.
    fs::write(
        root.join("sub/a.lua"),
        "return { v = require('@lib/mod').id() }",
    )
    .unwrap();
    fs::write(root.join("main.lua"), "print(require('./sub/a').v)").unwrap();

    run(root, &root.join("main.lua"))
        .assert()
        .success()
        .stdout(predicate::str::contains("nested-alias"));
}

#[test]
fn alias_cannot_escape_the_script_root() {
    let dir = TempDir::new().unwrap();
    let outside = dir.path().join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(
        outside.join("secret.lua"),
        r#"return { id = function() return "ESCAPED" end }"#,
    )
    .unwrap();
    let root = dir.path().join("proj");
    fs::create_dir(&root).unwrap();
    // An alias is still bounded by the root: config is not authorization.
    fs::write(
        root.join(".luaurc"),
        r#"{"aliases":{"out":"../outside/secret"}}"#,
    )
    .unwrap();
    fs::write(root.join("main.lua"), "print(require('@out').id())").unwrap();

    run(&root, &root.join("main.lua"))
        .assert()
        .failure()
        .stdout(predicate::str::contains("ESCAPED").not());
}

#[test]
fn undefined_alias_is_an_error() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    fs::write(root.join(".luaurc"), r#"{"aliases":{"lib":"./lib"}}"#).unwrap();
    fs::write(root.join("main.lua"), "require('@nosuch/x')").unwrap();

    run(root, &root.join("main.lua")).assert().failure();
}

#[test]
fn luaurc_above_the_root_is_not_consulted() {
    let dir = TempDir::new().unwrap();
    // A config outside the confinement root must not define aliases for a
    // script inside it, or an untrusted parent directory could redirect imports.
    fs::write(
        dir.path().join(".luaurc"),
        r#"{"aliases":{"up":"./elsewhere"}}"#,
    )
    .unwrap();
    fs::create_dir(dir.path().join("elsewhere")).unwrap();
    fs::write(dir.path().join("elsewhere/mod.lua"), "return {}").unwrap();
    let root = dir.path().join("proj");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("main.lua"), "require('@up/mod')").unwrap();

    run(&root, &root.join("main.lua")).assert().failure();
}

#[test]
fn self_alias_addresses_the_requiring_modules_own_directory() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    // `pkg/init.lua` stands in for `pkg`, so `./foo` means the *sibling* of
    // pkg while `@self/foo` means the foo inside it.
    fs::write(
        root.join("foo.lua"),
        r#"return { id = function() return "outer" end }"#,
    )
    .unwrap();
    fs::create_dir(root.join("pkg")).unwrap();
    fs::write(
        root.join("pkg/foo.lua"),
        r#"return { id = function() return "inner" end }"#,
    )
    .unwrap();
    fs::write(
        root.join("pkg/init.lua"),
        "return { rel = require('./foo').id(), slf = require('@self/foo').id() }",
    )
    .unwrap();
    fs::write(
        root.join("main.lua"),
        "local p = require('./pkg')\nprint(p.rel, p.slf)",
    )
    .unwrap();

    run(root, &root.join("main.lua"))
        .assert()
        .success()
        .stdout(predicate::str::contains("outer").and(predicate::str::contains("inner")));
}

#[test]
fn self_alias_is_only_meaningful_for_an_init_module() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    fs::create_dir(root.join("sub")).unwrap();
    fs::write(
        root.join("sub/helper.lua"),
        r#"return { id = function() return "helper" end }"#,
    )
    .unwrap();
    // `@self` names the module's own path, which for a plain module is the
    // file rather than a directory — so `@self/helper` looks below `a` and
    // finds nothing. The alias exists for `init` modules, where that path is
    // the directory the module stands in for.
    fs::write(
        root.join("sub/a.lua"),
        "return { v = require('@self/helper').id() }",
    )
    .unwrap();
    fs::write(root.join("main.lua"), "print(require('./sub/a').v)").unwrap();

    run(root, &root.join("main.lua"))
        .assert()
        .failure()
        .stderr(predicate::str::contains("no module found"))
        .stderr(predicate::str::contains("sub/a/helper.lua"));
}

#[test]
fn a_symlinked_script_imports_next_to_the_real_file() {
    // The `~/.local/bin/<name>` install: the launcher is a symlink and the
    // modules sit beside the file it points at. Anchoring the require root on
    // the link would confine imports to the bin directory instead.
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    let real = root.join("project");
    let bin = root.join("bin");
    fs::create_dir_all(real.join("lib")).unwrap();
    fs::create_dir(&bin).unwrap();
    fs::write(
        real.join("lib/x.lua"),
        r#"return { hello = function() return "from-project" end }"#,
    )
    .unwrap();
    fs::write(real.join("s.lua"), "print(require('./lib/x').hello())").unwrap();
    std::os::unix::fs::symlink(real.join("s.lua"), bin.join("s")).unwrap();

    run(root, &bin.join("s"))
        .assert()
        .success()
        .stdout(predicate::str::contains("from-project"));
}

#[test]
fn the_roots_sibling_file_is_not_loadable() {
    // `<root>.lua` beside the root directory is outside the tree.
    let outer = TempDir::new().unwrap();
    let root = outer.path().join("proj");
    fs::create_dir(&root).unwrap();
    fs::write(outer.path().join("proj.lua"), "return { leaked = true }").unwrap();
    fs::write(root.join("s.lua"), "print(require('./.').leaked)").unwrap();

    run(&root, &root.join("s.lua"))
        .assert()
        .failure()
        .code(3)
        .stdout(predicate::str::contains("true").not());
}

#[test]
fn a_symlink_out_of_the_root_is_rejected() {
    let outer = TempDir::new().unwrap();
    let root = outer.path().join("proj");
    fs::create_dir(&root).unwrap();
    fs::write(outer.path().join("secret.lua"), "return { leaked = true }").unwrap();
    std::os::unix::fs::symlink(outer.path().join("secret.lua"), root.join("link.lua")).unwrap();
    fs::write(root.join("s.lua"), "print(require('./link').leaked)").unwrap();

    run(&root, &root.join("s.lua"))
        .assert()
        .failure()
        .code(3)
        .stderr(predicate::str::contains("outside the script root"));
}

#[test]
fn a_dotted_name_appends_the_extension() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    fs::write(root.join("config.prod.luau"), "return { v = 'prod' }").unwrap();
    fs::write(root.join("config.luau"), "return { v = 'plain' }").unwrap();
    fs::write(root.join("s.luau"), "print(require('./config.prod').v)").unwrap();

    run(root, &root.join("s.luau"))
        .assert()
        .success()
        .stdout(predicate::str::contains("prod"));
}

#[test]
fn require_dir_and_script_anchors_on_fs() {
    let dir = TempDir::new().unwrap();
    let root = dir.path().canonicalize().unwrap();
    fs::create_dir(root.join("pkg")).unwrap();
    fs::write(root.join("pkg/init.luau"), "return { dir = fs.require_dir() }").unwrap();
    fs::write(
        root.join("s.luau"),
        "print('dir=' .. require('./pkg').dir)\nprint('script=' .. fs.script())\nprint('root=' .. fs.require_root())",
    )
    .unwrap();

    run(&root, &root.join("s.luau"))
        .assert()
        .success()
        .stdout(predicate::str::contains(format!("dir={}", root.join("pkg").display())))
        .stdout(predicate::str::contains(format!("script={}", root.join("s.luau").display())))
        .stdout(predicate::str::contains(format!("root={}", root.display())));
}

#[test]
fn an_ambiguous_intermediate_name_does_not_block_the_target() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    fs::create_dir(root.join("sub")).unwrap();
    fs::write(root.join("sub.luau"), "return 1").unwrap();
    fs::write(root.join("sub/lib.luau"), "return { v = 'ok' }").unwrap();
    fs::write(root.join("s.luau"), "print(require('./sub/lib').v)").unwrap();

    run(root, &root.join("s.luau"))
        .assert()
        .success()
        .stdout(predicate::str::contains("ok"));
}
