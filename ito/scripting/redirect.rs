//! Stream redirect targets, shared by the `proc` and `ssh` modules.
//!
//! A separate Rust module because both command families use these, but *not* a
//! separate Lua module: they are installed onto the `proc` table, since a
//! redirect target is only ever an argument to `:stdout()`/`:stderr()`. `ssh`
//! takes the very same values, so there is one spelling for local and remote:
//!
//! ```lua
//! proc.command({ "ls" }):stdout(proc.string()):run()
//! ssh.connect("host"):command({ "ls" }):stdout(proc.string()):run()
//! ```
//!
//! Targets are handle objects rather than strings so that passing the *same*
//! handle to both `:stdout()` and `:stderr()` merges the two streams.
//!
//! `proc.file(path)` always names a file on the machine running the script, even
//! for a remote command: the target is a file descriptor here, so a remote
//! command's output travels back over the connection to be written locally. To
//! write on the far side, redirect inside the remote command itself
//! (`{"sh", "-c", "cmd > /remote/path"}`).

use mlua::{Lua, Result as LuaResult, Table, UserData, Value};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Where a stream should go. Cloning a `Redirect` shares the underlying target,
/// which is what makes merging work.
///
/// `SameAsOther` never survives into a built command: `:stdout()`/`:stderr()`
/// substitute the other stream's current target on assignment, and both streams
/// start out as `Null`.
#[derive(Clone)]
pub(crate) enum Redirect {
    /// Discard the output.
    Null,
    /// Pass through to one of *this* process's own streams.
    ///
    /// `Same` is `proc.INHERIT`: whichever of our streams matches the slot it is
    /// assigned to, which is what `Stdio::inherit()` does. The other two name a
    /// stream outright (`proc.OUT`, `proc.ERR`), and that is the whole reason
    /// this carries a field: `Stdio::inherit()` cannot express a child's stdout
    /// going to *our stderr*, so the shell's `1>&2` had no spelling here.
    Inherit(InheritTarget),
    /// Write to a local file. The `File` is shared between clones, so two
    /// streams pointing at one handle write through a single offset.
    File {
        path: PathBuf,
        file: Arc<Mutex<Option<File>>>,
    },
    /// Capture for later retrieval, decoded as the given kind.
    Capture { kind: CaptureKind, id: usize },
    /// The other stream's target: `proc.STDOUT` on stderr (`2>&1`) or `proc.STDERR`
    /// on stdout (`1>&2`). Bound on assignment, not when the command runs.
    SameAsOther,
}

/// Which of this process's streams an `Inherit` target names.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum InheritTarget {
    /// The slot's own counterpart, i.e. plain `Stdio::inherit()`.
    Same,
    /// This process's stdout, whichever slot it is assigned to.
    Stdout,
    /// This process's stderr, whichever slot it is assigned to.
    Stderr,
}

impl InheritTarget {
    /// A duplicate of the named descriptor, or `None` for `Same`.
    ///
    /// `Same` has no descriptor to name — it means "whatever matches the slot",
    /// which each `Stdio` type spells as its own `inherit()`. The other two
    /// duplicate the descriptor by hand, because that is precisely what
    /// `inherit()` will not do: it matches the slot rather than the name, so a
    /// child's stdout could never be pointed at *our* stderr.
    ///
    /// A fresh `OwnedFd` per call rather than a shared one: each child needs its
    /// own, and `Stdio` consumes what it is given.
    pub(crate) fn dup(self) -> LuaResult<Option<std::os::fd::OwnedFd>> {
        use std::os::fd::AsFd;
        let borrowed = match self {
            Self::Same => return Ok(None),
            Self::Stdout => std::io::stdout().as_fd().try_clone_to_owned(),
            Self::Stderr => std::io::stderr().as_fd().try_clone_to_owned(),
        };
        borrowed.map(Some).map_err(|e| {
            mlua::Error::runtime(format!("failed to duplicate this process's stream: {e}"))
        })
    }

    /// The `std::process::Stdio` for this target, for local children.
    pub(crate) fn to_stdio(self) -> LuaResult<std::process::Stdio> {
        Ok(match self.dup()? {
            Some(fd) => std::process::Stdio::from(fd),
            None => std::process::Stdio::inherit(),
        })
    }
}

/// How captured bytes should be surfaced to Luau.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum CaptureKind {
    /// A Luau `buffer` -- honest about output being arbitrary bytes.
    Buffer,
    /// A Luau string. Still 8-bit clean, so binary survives intact.
    String,
}

impl UserData for Redirect {}

/// Identity for capture handles, so that sharing one handle between both streams
/// is distinguishable from two separate handles of the same kind.
static NEXT_CAPTURE_ID: Mutex<usize> = Mutex::new(0);

fn next_capture_id() -> usize {
    let mut guard = NEXT_CAPTURE_ID.lock().unwrap();
    *guard += 1;
    *guard
}

/// Pull a `Redirect` out of a Luau value, accepting the sentinel userdata.
pub(crate) fn redirect_from_value(value: &Value) -> LuaResult<Redirect> {
    match value {
        Value::UserData(ud) => Ok(ud.borrow::<Redirect>()?.clone()),
        other => Err(mlua::Error::runtime(format!(
            "expected a redirect target (proc.INHERIT, proc.OUT, proc.ERR, proc.NULL, \
             proc.STDOUT, proc.STDERR, proc.file(path), proc.buffer() or proc.string()), got {}",
            other.type_name()
        ))),
    }
}

/// Open a file target on first use and hand out `try_clone()`s thereafter, so
/// two streams sharing one handle write through a single file descriptor -- one
/// write offset, correctly interleaved output.
pub(crate) fn open_shared_file(path: &Path, file: &Arc<Mutex<Option<File>>>) -> LuaResult<File> {
    let mut slot = file.lock().unwrap();
    if slot.is_none() {
        let opened = File::create(path).map_err(|e| {
            mlua::Error::runtime(format!("failed to open '{}': {e}", path.display()))
        })?;
        *slot = Some(opened);
    }
    slot.as_ref()
        .expect("just opened")
        .try_clone()
        .map_err(|e| {
            mlua::Error::runtime(format!(
                "failed to duplicate handle for '{}': {e}",
                path.display()
            ))
        })
}

/// True when both streams write to the same path through *different* file
/// handles, which would give each its own write offset and clobber the file.
pub(crate) fn conflicting_files(stdout: &Redirect, stderr: &Redirect) -> Option<PathBuf> {
    if let (Redirect::File { path: p1, file: f1 }, Redirect::File { path: p2, file: f2 }) =
        (stdout, stderr)
        && p1 == p2
        && !Arc::ptr_eq(f1, f2)
    {
        return Some(p1.clone());
    }
    None
}

/// The capture kind when one handle serves both streams, meaning the two are
/// meant to merge into a single value.
pub(crate) fn merged_capture(stdout: &Redirect, stderr: &Redirect) -> Option<CaptureKind> {
    match (stdout, stderr) {
        (Redirect::Capture { kind, id: id1 }, Redirect::Capture { id: id2, .. }) if id1 == id2 => {
            Some(*kind)
        }
        _ => None,
    }
}

/// Materialize captured bytes as the Luau type the caller asked for.
pub(crate) fn capture_to_value(lua: &Lua, kind: CaptureKind, bytes: &[u8]) -> LuaResult<Value> {
    match kind {
        // No UTF-8 validation: output may be arbitrary bytes.
        CaptureKind::Buffer => Ok(Value::Buffer(lua.create_buffer(bytes)?)),
        CaptureKind::String => Ok(Value::String(lua.create_string(bytes)?)),
    }
}

/// Resolve a newly assigned target against the stream it may alias.
///
/// `proc.STDOUT`/`proc.STDERR` bind to the other stream's *current* target, matching
/// the shell: `cmd >f 2>&1 >/dev/null` still sends stderr to the file.
pub(crate) fn bind_alias(assigned: Redirect, other: &Redirect) -> Redirect {
    match assigned {
        Redirect::SameAsOther => other.clone(),
        concrete => concrete,
    }
}

/// Add the redirect targets to `exports` (the `proc` module table).
///
/// These are not a module of their own: a redirect target is only ever an
/// argument to `:stdout()`/`:stderr()`, so it lives beside the commands that
/// take it rather than in a global whose whole contents are other modules'
/// arguments. `ssh` shares them — the values are the same `Redirect`, so
/// `proc.string()` configures a remote command exactly as it does a local one.
pub fn install(lua: &Lua, exports: &Table) -> LuaResult<()> {
    exports.raw_set("NULL", Redirect::Null)?;
    exports.raw_set("INHERIT", Redirect::Inherit(InheritTarget::Same))?;
    // This process's own streams, named rather than matched by slot. `proc.ERR`
    // on stdout is the shell's `1>&2`; `proc.OUT` on stderr is `2>&1` *to our
    // stdout*, which is a different thing from `proc.STDOUT` below (that one
    // merges the child's two streams with each other, wherever they point).
    exports.raw_set("OUT", Redirect::Inherit(InheritTarget::Stdout))?;
    exports.raw_set("ERR", Redirect::Inherit(InheritTarget::Stderr))?;
    // Cross-links between the *child's* streams: STDOUT on stderr is `2>&1`,
    // STDERR on stdout is `1>&2` -- both resolved against whatever the other
    // stream currently targets, at the moment of assignment.
    exports.raw_set("STDOUT", Redirect::SameAsOther)?;
    exports.raw_set("STDERR", Redirect::SameAsOther)?;

    // Each call yields a distinct handle; reuse one handle to merge streams.
    // The path is always local, even for a remote command.
    exports.raw_set(
        "file",
        lua.create_function(|_, path: String| {
            Ok(Redirect::File {
                path: PathBuf::from(path),
                file: Arc::new(Mutex::new(None)),
            })
        })?,
    )?;

    exports.raw_set(
        "buffer",
        lua.create_function(|_, ()| {
            Ok(Redirect::Capture {
                kind: CaptureKind::Buffer,
                id: next_capture_id(),
            })
        })?,
    )?;

    exports.raw_set(
        "string",
        lua.create_function(|_, ()| {
            Ok(Redirect::Capture {
                kind: CaptureKind::String,
                id: next_capture_id(),
            })
        })?,
    )?;

    Ok(())
}
