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
//! (`{"sh", "-c", "cmd > /remote/path"}`). The path is resolved against this
//! process's working directory, not a command's `:current_dir()`.
//!
//! A `proc.file()` handle opens its file on first use and keeps the descriptor,
//! so reusing one handle across several commands continues at the same offset
//! (an append in effect) until the handle is collected; make a fresh handle to
//! truncate again. `{append = true}` opens with `O_APPEND` instead of truncating.
//!
//! `:stdin()` takes its own small set of values (`Stdin`): NULL, INHERIT, a
//! `proc.file()` (read, not created), or a Luau string/buffer fed to the child.

use mlua::{Lua, Result as LuaResult, Table, UserData, Value};
use std::ffi::OsString;
use std::fs::File;
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::task::JoinHandle;

/// A Luau string's bytes as an `OsString`: Luau strings are bytes, and so are
/// Unix paths, arguments and environment entries.
pub(crate) fn os_string(s: &mlua::LuaString) -> OsString {
    OsString::from_vec(s.as_bytes().to_vec())
}

/// Where a stream should go. Cloning a `Redirect` shares the underlying target,
/// which is what makes merging work.
///
/// `AliasStdout`/`AliasStderr` never survive into a built command:
/// `:stdout()`/`:stderr()` substitute the other stream's current target on
/// assignment (see `bind_alias`).
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
        append: bool,
    },
    /// Capture for later retrieval, decoded as the given kind.
    Capture { kind: CaptureKind, id: usize },
    /// `proc.STDOUT`: the child's stdout target, for use on stderr (`2>&1`).
    /// Bound on assignment, not when the command runs.
    AliasStdout,
    /// `proc.STDERR`: the child's stderr target, for use on stdout (`1>&2`).
    AliasStderr,
}

/// Which of the child's streams a target is being assigned to.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Slot {
    Stdout,
    Stderr,
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
        use std::io::Write;
        use std::os::fd::AsFd;
        // Anything we printed ourselves must land before the child's output.
        let _ = std::io::stdout().flush();
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
            None => {
                use std::io::Write;
                let _ = std::io::stdout().flush();
                std::process::Stdio::inherit()
            }
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
        Value::UserData(ud) => match ud.borrow::<Redirect>() {
            Ok(r) => Ok(r.clone()),
            Err(_) => Err(not_a_redirect("a userdata of another kind")),
        },
        other => Err(not_a_redirect(other.type_name())),
    }
}

fn not_a_redirect(got: &str) -> mlua::Error {
    mlua::Error::runtime(format!(
        "expected a redirect target (proc.INHERIT, proc.OUT, proc.ERR, proc.NULL, \
         proc.STDOUT, proc.STDERR, proc.file(path), proc.buffer() or proc.string()), got {got}"
    ))
}

/// What a child reads on stdin.
#[derive(Clone)]
pub(crate) enum Stdin {
    Null,
    Inherit,
    /// Read from a local file (opened read-only at spawn).
    File(PathBuf),
    /// Fed to the child, then stdin is closed.
    Bytes(Arc<[u8]>),
}

/// Pull a stdin source out of a Luau value: a string or buffer is the input
/// itself; `proc.NULL`, `proc.INHERIT` and `proc.file(path)` are the only
/// redirect handles that make sense on input.
pub(crate) fn stdin_from_value(value: &Value) -> LuaResult<Stdin> {
    match value {
        Value::String(s) => Ok(Stdin::Bytes(s.as_bytes().to_vec().into())),
        Value::Buffer(b) => Ok(Stdin::Bytes(b.to_vec().into())),
        Value::UserData(ud) => match ud.borrow::<Redirect>().as_deref() {
            Ok(Redirect::Null) => Ok(Stdin::Null),
            Ok(Redirect::Inherit(InheritTarget::Same)) => Ok(Stdin::Inherit),
            Ok(Redirect::File { path, .. }) => Ok(Stdin::File(path.clone())),
            _ => Err(bad_stdin()),
        },
        _ => Err(bad_stdin()),
    }
}

fn bad_stdin() -> mlua::Error {
    mlua::Error::runtime(
        "expected a stdin source (proc.NULL, proc.INHERIT, proc.file(path), a string or a buffer)",
    )
}

impl Stdin {
    /// The file to open for reading, if this source is one.
    pub(crate) fn open_file(&self) -> LuaResult<Option<File>> {
        let Self::File(path) = self else {
            return Ok(None);
        };
        File::open(path).map(Some).map_err(|e| {
            mlua::Error::runtime(format!(
                "failed to open '{}' for stdin: {e}",
                path.display()
            ))
        })
    }
}

/// Write `bytes` into a child's stdin on its own task, then close it. Errors
/// (the child closing its end early) are the child's business, not ours.
pub(crate) fn feed<W>(mut stdin: W, bytes: Arc<[u8]>)
where
    W: tokio::io::AsyncWrite + Unpin + 'static,
{
    tokio::task::spawn_local(async move {
        let _ = stdin.write_all(&bytes).await;
        let _ = stdin.shutdown().await;
    });
}

/// A task draining one captured stream into a byte buffer.
///
/// Local rather than `tokio::spawn`ed: the runtime is single-threaded and a
/// child's pipes are `!Send`.
pub(crate) type Reader = JoinHandle<Vec<u8>>;

/// Drain a stream into a buffer on its own task, starting immediately.
///
/// Reading has to begin at spawn time, not at `wait()`: a child that outruns a
/// pipe's buffer would otherwise block against a reader that has not started.
pub(crate) fn read_to_end<R>(mut stream: R) -> Reader
where
    R: AsyncReadExt + Unpin + 'static,
{
    tokio::task::spawn_local(async move {
        let mut buf = Vec::new();
        // A read error leaves whatever arrived before it; the child's exit
        // status is the authority on whether the run succeeded.
        let _ = stream.read_to_end(&mut buf).await;
        buf
    })
}

/// Await a reader task; `which` only names the stream in the error message.
pub(crate) async fn join_reader(reader: Option<Reader>, which: &str) -> LuaResult<Vec<u8>> {
    let Some(handle) = reader else {
        return Ok(Vec::new());
    };
    handle
        .await
        .map_err(|e| mlua::Error::runtime(format!("{which} reader task failed: {e}")))
}

/// One pipe for both streams, so merged output is truly interleaved: the two
/// writer ends (for the child's stdout and stderr) and the reader draining it.
pub(crate) fn merged_pipe() -> LuaResult<(File, File, Reader)> {
    use std::os::fd::OwnedFd;
    let (reader, writer) =
        std::io::pipe().map_err(|e| mlua::Error::runtime(format!("failed to create pipe: {e}")))?;
    let writer2 = writer
        .try_clone()
        .map_err(|e| mlua::Error::runtime(format!("failed to duplicate pipe: {e}")))?;
    // Not one of the child's own handles, so it has to be registered with the
    // reactor by hand to become readable asynchronously.
    let rx = tokio::net::unix::pipe::Receiver::from_owned_fd(OwnedFd::from(reader))
        .map_err(|e| mlua::Error::runtime(format!("failed to watch the merged pipe: {e}")))?;
    Ok((
        File::from(OwnedFd::from(writer)),
        File::from(OwnedFd::from(writer2)),
        read_to_end(rx),
    ))
}

/// Open a file target on first use and hand out `try_clone()`s thereafter, so
/// two streams sharing one handle write through a single file descriptor -- one
/// write offset, correctly interleaved output.
pub(crate) fn open_shared_file(
    path: &Path,
    file: &Arc<Mutex<Option<File>>>,
    append: bool,
) -> LuaResult<File> {
    let mut slot = file.lock().unwrap();
    if slot.is_none() {
        let opened = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .append(append)
            .truncate(!append)
            .open(path)
            .map_err(|e| {
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

/// Whether two paths name one file: equal strings, or (once both exist) the
/// same device and inode, which also catches `a/../f`, symlinks and hard links.
fn same_file(p1: &Path, p2: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    if p1 == p2 {
        return true;
    }
    match (std::fs::metadata(p1), std::fs::metadata(p2)) {
        (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
        _ => false,
    }
}

/// True when both streams write to the same file through *different* file
/// handles, which would give each its own write offset and clobber the file.
pub(crate) fn conflicting_files(stdout: &Redirect, stderr: &Redirect) -> Option<PathBuf> {
    if let (
        Redirect::File {
            path: p1, file: f1, ..
        },
        Redirect::File {
            path: p2, file: f2, ..
        },
    ) = (stdout, stderr)
        && same_file(p1, p2)
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
/// `proc.STDOUT`/`proc.STDERR` bind to the other stream's *current* target
/// (`other`, `None` when unset, which reads as NULL), matching the shell:
/// `cmd >f 2>&1 >/dev/null` still sends stderr to the file. An alias onto its
/// own stream (`:stdout(proc.STDOUT)`) is a mistake and raises. A plain
/// `INHERIT` on the other stream is resolved to the concrete stream it means,
/// since "the slot's own counterpart" would otherwise change meaning here.
pub(crate) fn bind_alias(
    assigned: Redirect,
    other: Option<&Redirect>,
    slot: Slot,
) -> LuaResult<Redirect> {
    match (&assigned, slot) {
        (Redirect::AliasStdout, Slot::Stdout) => Err(mlua::Error::runtime(
            "proc.STDOUT on :stdout() aliases a stream onto itself; it belongs on :stderr()",
        )),
        (Redirect::AliasStderr, Slot::Stderr) => Err(mlua::Error::runtime(
            "proc.STDERR on :stderr() aliases a stream onto itself; it belongs on :stdout()",
        )),
        (Redirect::AliasStdout | Redirect::AliasStderr, _) => {
            Ok(match other.cloned().unwrap_or(Redirect::Null) {
                Redirect::Inherit(InheritTarget::Same) => Redirect::Inherit(match slot {
                    Slot::Stdout => InheritTarget::Stderr,
                    Slot::Stderr => InheritTarget::Stdout,
                }),
                r => r,
            })
        }
        _ => Ok(assigned),
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
    // stream currently targets, at the moment of assignment. Distinct values so
    // a self-alias can be told apart and refused.
    exports.raw_set("STDOUT", Redirect::AliasStdout)?;
    exports.raw_set("STDERR", Redirect::AliasStderr)?;

    // Each call yields a distinct handle; reuse one handle to merge streams.
    // The path is always local, even for a remote command. `{append = true}`
    // opens for append instead of truncating.
    exports.raw_set(
        "file",
        lua.create_function(|_, (path, opts): (mlua::LuaString, Option<Table>)| {
            let append = match opts {
                Some(t) => t.get::<Option<bool>>("append")?.unwrap_or(false),
                None => false,
            };
            Ok(Redirect::File {
                path: PathBuf::from(os_string(&path)),
                file: Arc::new(Mutex::new(None)),
                append,
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

#[cfg(test)]
mod tests {
    use super::*;

    /// `:stdout(INHERIT):stderr(STDOUT)` must mean *our stdout*, not "whatever
    /// matches the stderr slot" (our stderr).
    #[test]
    fn alias_of_plain_inherit_names_the_concrete_stream() {
        let inherit = Redirect::Inherit(InheritTarget::Same);
        let r = bind_alias(Redirect::AliasStdout, Some(&inherit), Slot::Stderr).unwrap();
        assert!(matches!(r, Redirect::Inherit(InheritTarget::Stdout)));
        let r = bind_alias(Redirect::AliasStderr, Some(&inherit), Slot::Stdout).unwrap();
        assert!(matches!(r, Redirect::Inherit(InheritTarget::Stderr)));
    }

    #[test]
    fn alias_of_an_unconfigured_stream_is_null() {
        let r = bind_alias(Redirect::AliasStdout, None, Slot::Stderr).unwrap();
        assert!(matches!(r, Redirect::Null));
    }

    #[test]
    fn self_alias_is_rejected() {
        assert!(bind_alias(Redirect::AliasStdout, None, Slot::Stdout).is_err());
        assert!(bind_alias(Redirect::AliasStderr, None, Slot::Stderr).is_err());
    }
}
