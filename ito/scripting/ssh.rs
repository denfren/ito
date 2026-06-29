//! Luau bindings for running commands over SSH, mirroring the `proc` module.
//!
//! A hostname is all that is needed; everything else -- `User`, `HostName`,
//! `ProxyJump`, `IdentityFile` -- comes from the machine's ssh config, because
//! this drives the real `ssh` binary rather than reimplementing the protocol.
//!
//! Host keys are trusted on first use: an unknown host is recorded, a changed
//! key is an error, and ssh is never allowed to prompt.
//!
//! ```lua
//! local s = ssh.connect("prod")
//! local r = s:command({ "systemctl", "is-active", "nginx" })
//!     :stdout(proc.string())
//!     :run()
//! ```
//!
//! Each `connect()` is an independent session with its own control master, so
//! several connections -- to the same host or different ones -- coexist, and
//! every command on one session reuses its single handshake.
//!
//! `:check()` asks whether a session is still usable, which matters for a
//! long-lived connection that may have died between commands. It is `ssh -O
//! check` against the control master -- a *local* question about the
//! multiplexed connection, not a round trip to the remote host, so it is cheap
//! but does not prove the far side is healthy.
//!
//! Two differences from `proc` follow from what SSH can actually do:
//!
//!   * there is no `env()` or `current_dir()`, because sshd controls the remote
//!     environment; use an explicit remote shell instead
//!   * there is no `kill()`, because SSH offers no signal channel -- dropping
//!     the connection leaves the remote process running
//!
//! `proc.file()` still refers to a *local* file: it is a file descriptor on
//! this machine, so a remote command's output travels back over the connection
//! to be written here. To write on the far side, redirect inside the command.

use crate::scripting::proc;
use crate::scripting::redirect::{self, CaptureKind, Redirect, bind_alias};
use mlua::{AnyUserData, Lua, Result as LuaResult, Table, UserData, UserDataMethods, Value};
use openssh::{KnownHosts, Session, Stdio};
use std::sync::{Arc, Mutex};

/// A live SSH session. Cloning shares the underlying control master.
struct Connection {
    host: String,
    session: Arc<Session>,
}

#[derive(Clone)]
struct RemoteCommandState {
    program: String,
    args: Vec<String>,
    stdout: Redirect,
    stderr: Redirect,
}

/// Builder for a remote command, finalized with `:run()` or `:spawn()`.
struct RemoteCommand {
    host: String,
    session: Arc<Session>,
    state: Arc<Mutex<RemoteCommandState>>,
}

/// Translate a redirect into an `openssh::Stdio`.
///
/// Captures become pipes. Files are opened locally and passed as a descriptor,
/// which is why they land on this machine rather than the remote one.
fn to_stdio(redirect: &Redirect) -> LuaResult<Stdio> {
    match redirect {
        Redirect::Null => Ok(Stdio::null()),
        // `openssh` has its own `Stdio`, so the duplicated descriptor is
        // converted here rather than through `InheritTarget::to_stdio`.
        Redirect::Inherit(target) => Ok(match target.dup()? {
            Some(fd) => Stdio::from(std::fs::File::from(fd)),
            None => Stdio::inherit(),
        }),
        Redirect::Capture { .. } => Ok(Stdio::piped()),
        Redirect::File { path, file } => {
            let cloned = redirect::open_shared_file(path, file)?;
            Ok(Stdio::from(cloned))
        }
        Redirect::SameAsOther => Err(mlua::Error::runtime("internal: unresolved stream alias")),
    }
}

impl Connection {
    async fn connect(host: String) -> LuaResult<Self> {
        // Trust on first use: a new host is recorded, a *changed* key is an
        // error, and ssh never prompts (the crate always passes BatchMode=yes).
        //
        // This is an `-o` override, so it wins over StrictHostKeyChecking in the
        // ssh config; the crate offers no way to omit the option and let the
        // config decide. Hosts whose key legitimately rotates therefore need
        // `UserKnownHostsFile /dev/null` in their config, so that every connect
        // looks like a first connect.
        let session = Session::connect(&host, KnownHosts::Add)
            .await
            .map_err(|e| mlua::Error::runtime(format!("failed to connect to '{host}': {e}")))?;

        Ok(Self {
            host,
            session: Arc::new(session),
        })
    }
}

impl UserData for Connection {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("host", |_, this, ()| Ok(this.host.clone()));

        // Is the session still usable? Returns `true`, or `false` plus the
        // reason -- a dead connection is an expected answer to this question,
        // not an error, so it is reported rather than raised.
        methods.add_async_method("check", |_, this, ()| async move {
            let session = Arc::clone(&this.session);
            match session.check().await {
                Ok(()) => Ok((true, None)),
                Err(e) => Ok((false, Some(e.to_string()))),
            }
        });

        // Accepts a program name or a list of {program, args...}, like
        // `proc.command`.
        methods.add_method("command", |_, this, spec: Value| {
            let (program, args) = proc::parse_command_spec(spec)?;
            Ok(RemoteCommand {
                host: this.host.clone(),
                session: Arc::clone(&this.session),
                state: Arc::new(Mutex::new(RemoteCommandState {
                    program,
                    args,
                    // Neither stream leaks to the terminal by default.
                    stdout: Redirect::Null,
                    stderr: Redirect::Null,
                })),
            })
        });
    }
}

impl RemoteCommand {
    /// Build the command and start it, returning the spawned child.
    ///
    /// Async because the handshake and the remote exec are both round trips:
    /// blocking on them would stop every other task from progressing.
    async fn spawn(&self) -> LuaResult<RemoteChild> {
        let state = self.state.lock().unwrap().clone();

        if let Some(path) = redirect::conflicting_files(&state.stdout, &state.stderr) {
            return Err(mlua::Error::runtime(format!(
                "stdout and stderr both target '{}' via separate handles, which \
                 would give each its own write offset and corrupt the file; reuse \
                 one proc.file() handle to merge them",
                path.display()
            )));
        }

        // A shared capture handle cannot be merged into one pipe the way the
        // local module does: `openssh` hands back separate stdout/stderr
        // streams, so the two are stitched together after the fact instead.
        let merged = redirect::merged_capture(&state.stdout, &state.stderr);

        // `raw_command` rather than `command`: argv is passed explicitly, so
        // letting the crate shell-escape the program would double-escape it.
        let mut cmd = Arc::clone(&self.session).arc_raw_command(&state.program);
        cmd.args(&state.args);
        cmd.stdin(Stdio::null());
        cmd.stdout(to_stdio(&state.stdout)?);
        cmd.stderr(to_stdio(&state.stderr)?);

        let child = cmd.spawn().await.map_err(|e| {
            mlua::Error::runtime(format!(
                "failed to run '{}' on '{}': {e}",
                state.program, self.host
            ))
        })?;

        Ok(RemoteChild {
            host: self.host.clone(),
            program: state.program,
            child: Some(child),
            stdout_capture: match &state.stdout {
                Redirect::Capture { kind, .. } => Some(*kind),
                _ => None,
            },
            stderr_capture: match &state.stderr {
                Redirect::Capture { kind, .. } => Some(*kind),
                _ => None,
            },
            merged,
        })
    }
}

impl UserData for RemoteCommand {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_function("arg", |_, (ud, arg): (AnyUserData, String)| {
            ud.borrow::<Self>()?.state.lock().unwrap().args.push(arg);
            Ok(ud)
        });

        methods.add_function("args", |_, (ud, args): (AnyUserData, Vec<String>)| {
            ud.borrow::<Self>()?.state.lock().unwrap().args.extend(args);
            Ok(ud)
        });

        methods.add_function("stdout", |_, (ud, target): (AnyUserData, Value)| {
            let redirect = redirect::redirect_from_value(&target)?;
            let this = ud.borrow::<Self>()?;
            let mut state = this.state.lock().unwrap();
            // Bound on assignment, as in the local module, so reassigning the
            // other stream later does not drag this one along.
            state.stdout = bind_alias(redirect, &state.stderr);
            drop(state);
            drop(this);
            Ok(ud)
        });

        methods.add_function("stderr", |_, (ud, target): (AnyUserData, Value)| {
            let redirect = redirect::redirect_from_value(&target)?;
            let this = ud.borrow::<Self>()?;
            let mut state = this.state.lock().unwrap();
            state.stderr = bind_alias(redirect, &state.stdout);
            drop(state);
            drop(this);
            Ok(ud)
        });

        // --- finalizers ---------------------------------------------------

        methods.add_async_method("spawn", |_, this, ()| async move { this.spawn().await });

        methods.add_async_method("run", |lua, this, ()| async move {
            let mut child = this.spawn().await?;
            child.wait(&lua).await
        });

        // Present only to fail clearly: sshd controls the remote environment.
        methods.add_method("env", |_, _, (_key, _value): (String, String)| {
            Err::<(), _>(mlua::Error::runtime(
                "ssh commands have no env(): sshd controls the remote environment. \
                 Use an explicit remote shell, e.g. \
                 :command({\"sh\", \"-c\", \"VAR=1 exec cmd\"})",
            ))
        });

        methods.add_method("current_dir", |_, _, _dir: String| {
            Err::<(), _>(mlua::Error::runtime(
                "ssh commands have no current_dir(): use an explicit remote shell, \
                 e.g. :command({\"sh\", \"-c\", \"cd /path && exec cmd\"})",
            ))
        });
    }
}

/// A running remote command, awaited with `:wait()`.
struct RemoteChild {
    host: String,
    program: String,
    child: Option<openssh::Child<Arc<Session>>>,
    stdout_capture: Option<CaptureKind>,
    stderr_capture: Option<CaptureKind>,
    merged: Option<CaptureKind>,
}

impl RemoteChild {
    async fn wait(&mut self, lua: &Lua) -> LuaResult<Table> {
        let child = self
            .child
            .take()
            .ok_or_else(|| mlua::Error::runtime("this remote child has already been waited for"))?;

        // `wait_with_output` drains both pipes concurrently while waiting, so a
        // command that outproduces a pipe buffer cannot deadlock.
        let output = child.wait_with_output().await.map_err(|e| {
            mlua::Error::runtime(format!(
                "failed waiting for '{}' on '{}': {e}",
                self.program, self.host
            ))
        })?;

        let result = lua.create_table()?;
        result.raw_set("success", output.status.success())?;
        match output.status.code() {
            Some(code) => result.raw_set("status", code)?,
            // Killed by a signal; no exit code exists.
            None => result.raw_set("status", Value::Nil)?,
        }
        // The same fields a local child reports, through the same helper so the
        // two result tables cannot drift. This is the *local* ssh client's
        // status, though: a remote command killed by a signal on the far side
        // reaches us as sshd's exit code, not as a signal here.
        crate::scripting::proc::set_signal_fields(
            &result,
            std::os::unix::process::ExitStatusExt::signal(&output.status),
        )?;

        if let Some(kind) = self.merged {
            // One handle served both streams. openssh keeps them separate, so
            // concatenate: stdout first, then whatever arrived on stderr.
            let mut bytes = output.stdout.clone();
            bytes.extend_from_slice(&output.stderr);
            let value = redirect::capture_to_value(lua, kind, &bytes)?;
            result.raw_set("stdout", value.clone())?;
            result.raw_set("stderr", value)?;
        } else {
            if let Some(kind) = self.stdout_capture {
                result.raw_set(
                    "stdout",
                    redirect::capture_to_value(lua, kind, &output.stdout)?,
                )?;
            }
            if let Some(kind) = self.stderr_capture {
                result.raw_set(
                    "stderr",
                    redirect::capture_to_value(lua, kind, &output.stderr)?,
                )?;
            }
        }

        Ok(result)
    }
}

impl UserData for RemoteChild {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_async_method_mut(
            "wait",
            |lua, mut this, ()| async move { this.wait(&lua).await },
        );

        // SSH has no signal channel, so a truthful error beats a method that
        // pretends to work.
        methods.add_method("kill", |_, _, ()| {
            Err::<(), _>(mlua::Error::runtime(
                "SSH cannot signal a remote process; run it under a pty or use a \
                 remote pkill. Note that abandoning a remote child leaves it running",
            ))
        });
    }
}

/// Build the `ssh` module table.
pub fn module(lua: &Lua) -> LuaResult<Table> {
    let exports = lua.create_table()?;

    exports.raw_set(
        "connect",
        lua.create_async_function(
            |_, host: String| async move { Connection::connect(host).await },
        )?,
    )?;

    Ok(exports)
}

/// Install the `ssh` module as a global.
pub fn register(lua: &Lua) -> LuaResult<()> {
    lua.globals().raw_set("ssh", module(lua)?)?;
    Ok(())
}
