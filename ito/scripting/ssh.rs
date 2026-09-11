//! Luau bindings for running commands over SSH, mirroring the `proc` module.
//!
//! A hostname is all that is needed; everything else -- `User`, `HostName`,
//! `ProxyJump`, `IdentityFile` -- comes from the machine's ssh config, because
//! this drives the real `ssh` binary rather than reimplementing the protocol.
//! `host` may be anything `ssh` accepts as a destination: an alias, `user@host`,
//! or an `ssh://user@host:port` URI.
//!
//! Host keys are checked strictly: an unknown or changed key is an error, and
//! ssh is never allowed to prompt. `ssh.connect(host, { tofu = true })` opts
//! into trust-on-first-use (an unknown host is recorded, a changed key is still
//! an error).
//!
//! ```lua
//! local s = ssh.connect("prod")
//! local r = s:command({ "systemctl", "is-active", "nginx" })
//!     :stdout(proc.string())
//!     :run()
//! s:close()
//! ```
//!
//! Each `connect()` is an independent session with its own control master, so
//! several connections -- to the same host or different ones -- coexist, and
//! every command on one session reuses its single handshake. `:close()` shuts
//! the master down (awaited); without it the session is closed when collected,
//! with a blocking `ssh -O exit` at an unpredictable moment.
//!
//! `:check()` asks whether a session is still usable, which matters for a
//! long-lived connection that may have died between commands. It is `ssh -O
//! check` against the control master -- a *local* question about the
//! multiplexed connection, not a round trip to the remote host, so it is cheap
//! but does not prove the far side is healthy.
//!
//! The program and each argument are shell-escaped for the remote side, so they
//! arrive as exactly those words; sshd still runs the result through the user's
//! login shell, which is why an explicit `{"sh", "-c", ...}` is the way to get
//! redirections, variables or a `cd`.
//!
//! Differences from `proc` follow from what SSH can actually do:
//!
//!   * there is no `env()`/`envs()`/`env_remove()`/`env_clear()`, `current_dir()`
//!     or `exec()`, because sshd controls the remote environment; use an
//!     explicit remote shell. They exist only to fail with that explanation
//!   * there is no `kill()`/`signal()`/`id()`, because SSH offers no signal
//!     channel and there is no local pid -- dropping the connection leaves the
//!     remote process running
//!   * a command that dies with status 255 (ssh's own failure code) or 127
//!     (command not found) is reported as `status = 255`/`127` with its output,
//!     like any other nonzero exit, rather than raised. A command exiting 255
//!     itself is indistinguishable from ssh failing
//!   * merged capture (`:stdout(h):stderr(h)`) shares one local pipe, so the two
//!     streams interleave as they arrive
//!
//! `:interactive()` runs the command on a remote pty attached to this terminal
//! (`ssh -tt` over the existing master) and waits for it; it does not accept
//! redirects. `:stdin(source)` takes the same values as `proc`'s.
//!
//! `proc.file()` still refers to a *local* file: it is a file descriptor on
//! this machine, so a remote command's output travels back over the connection
//! to be written here. To write on the far side, redirect inside the command.

use crate::scripting::proc::{self, Captured, IgnoreInterrupts};
use crate::scripting::redirect::{
    self, Reader, Redirect, Slot, Stdin, bind_alias, stdin_from_value,
};
use mlua::{
    AnyUserData, Lua, Result as LuaResult, Table, UserData, UserDataMethods, Value, Variadic,
};
use openssh::{KnownHosts, Session, Stdio};
use std::os::unix::process::ExitStatusExt;
use std::process::ExitStatus;
use std::sync::{Arc, Mutex};
use tokio::task::JoinHandle;

/// An error and its whole `source()` chain, since openssh's top-level messages
/// ("failed to connect") carry the actual reason only one level down.
fn chain(e: &dyn std::error::Error) -> String {
    let mut out = e.to_string();
    let mut source = e.source();
    while let Some(s) = source {
        let text = s.to_string();
        if !out.contains(&text) {
            out.push_str(": ");
            out.push_str(&text);
        }
        source = s.source();
    }
    out
}

/// Quote one word for the remote shell (only used to build the single command
/// string handed to the system `ssh` by `:interactive()`).
fn sh_quote(s: &str) -> String {
    let plain = !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./=:,@%+".contains(c));
    if plain {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

/// Remote arguments travel through `openssh`'s `&str` API, so they must be UTF-8.
fn utf8(s: std::ffi::OsString) -> LuaResult<String> {
    s.into_string()
        .map_err(|_| mlua::Error::runtime("ssh commands must be valid UTF-8"))
}

/// A live SSH session. `None` once `:close()`d. Commands hold their own `Arc`.
struct Connection {
    host: String,
    session: Mutex<Option<Arc<Session>>>,
}

#[derive(Clone)]
struct RemoteCommandState {
    program: String,
    args: Vec<String>,
    stdin: Option<Stdin>,
    stdout: Option<Redirect>,
    stderr: Option<Redirect>,
}

/// Builder for a remote command, finalized with `:run()`, `:spawn()` or
/// `:interactive()`.
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
        Redirect::File { path, file, append } => {
            let cloned = redirect::open_shared_file(path, file, *append)?;
            Ok(Stdio::from(cloned))
        }
        Redirect::AliasStdout | Redirect::AliasStderr => {
            Err(mlua::Error::runtime("internal: unresolved stream alias"))
        }
    }
}

impl Connection {
    async fn connect(host: String, tofu: bool) -> LuaResult<Self> {
        // Strict unless the caller opts into trust-on-first-use. Either way this
        // is an `-o` override that wins over StrictHostKeyChecking in the ssh
        // config (the crate offers no way to omit it), and ssh never prompts (the
        // crate always passes BatchMode=yes).
        let check = if tofu {
            KnownHosts::Add
        } else {
            KnownHosts::Strict
        };
        let session = Session::connect(&host, check)
            .await
            .map_err(|e| mlua::Error::runtime(format!("failed to connect to '{host}': {}", chain(&e))))?;

        Ok(Self {
            host,
            session: Mutex::new(Some(Arc::new(session))),
        })
    }

    fn session(&self) -> LuaResult<Arc<Session>> {
        self.session
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| mlua::Error::runtime("this ssh session has been closed"))
    }
}

impl UserData for Connection {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("host", |_, this, ()| Ok(this.host.clone()));

        // Is the session still usable? Returns `true`, or `false` plus the
        // reason -- a dead connection is an expected answer to this question,
        // not an error, so it is reported rather than raised.
        methods.add_async_method("check", |_, this, ()| async move {
            let session = this.session()?;
            match session.check().await {
                Ok(()) => Ok((true, None)),
                Err(e) => Ok((false, Some(chain(&e)))),
            }
        });

        // Shut the control master down, awaited. Refused while a command or
        // child still holds the session (closing needs sole ownership).
        methods.add_async_method("close", |_, this, ()| async move {
            let taken = this.session.lock().unwrap().take();
            let Some(session) = taken else {
                return Ok(());
            };
            match Arc::try_unwrap(session) {
                Ok(session) => session.close().await.map_err(|e| {
                    mlua::Error::runtime(format!(
                        "failed to close the session to '{}': {}",
                        this.host,
                        chain(&e)
                    ))
                }),
                Err(session) => {
                    *this.session.lock().unwrap() = Some(session);
                    Err(mlua::Error::runtime(
                        "cannot close: a command or child on this session is still alive",
                    ))
                }
            }
        });

        // Accepts a program name or a list of {program, args...}, like
        // `proc.command`.
        methods.add_method("command", |_, this, spec: Value| {
            let (program, args) = proc::parse_command_spec(spec)?;
            Ok(RemoteCommand {
                host: this.host.clone(),
                session: this.session()?,
                state: Arc::new(Mutex::new(RemoteCommandState {
                    program: utf8(program)?,
                    args: args.into_iter().map(utf8).collect::<LuaResult<_>>()?,
                    stdin: None,
                    stdout: None,
                    stderr: None,
                })),
            })
        });
    }
}

type Exit = Result<ExitStatus, openssh::Error>;

impl RemoteCommand {
    /// Build the command and start it, returning the spawned child.
    ///
    /// Async because the handshake and the remote exec are both round trips:
    /// blocking on them would stop every other task from progressing.
    async fn spawn(&self) -> LuaResult<RemoteChild> {
        let state = self.state.lock().unwrap().clone();
        let stdout = state.stdout.clone().unwrap_or(Redirect::Null);
        let stderr = state.stderr.clone().unwrap_or(Redirect::Null);

        if let Some(path) = redirect::conflicting_files(&stdout, &stderr) {
            return Err(mlua::Error::runtime(format!(
                "stdout and stderr both target '{}' via separate handles, which \
                 would give each its own write offset and corrupt the file; reuse \
                 one proc.file() handle to merge them",
                path.display()
            )));
        }

        let captured = Captured::of(&stdout, &stderr);
        let merged = redirect::merged_capture(&stdout, &stderr).is_some();

        // `command` escapes the program and each argument, so they reach the
        // remote shell as exactly the words given (no injection through them).
        let mut cmd = Arc::clone(&self.session).arc_command(state.program.clone());
        cmd.args(&state.args);

        let mut input = None;
        match state.stdin.clone().unwrap_or(Stdin::Null) {
            Stdin::Null => cmd.stdin(Stdio::null()),
            Stdin::Inherit => cmd.stdin(Stdio::inherit()),
            file @ Stdin::File(_) => {
                cmd.stdin(Stdio::from(file.open_file()?.expect("a file source")))
            }
            Stdin::Bytes(bytes) => {
                input = Some(bytes);
                cmd.stdin(Stdio::piped())
            }
        };

        // One pipe for both streams keeps merged output interleaved and drains
        // from spawn, like the local module.
        let mut merged_reader = None;
        if merged {
            let (out, err, reader) = redirect::merged_pipe()?;
            cmd.stdout(Stdio::from(out));
            cmd.stderr(Stdio::from(err));
            merged_reader = Some(reader);
        } else {
            cmd.stdout(to_stdio(&stdout)?);
            cmd.stderr(to_stdio(&stderr)?);
        }

        let mut child = cmd.spawn().await.map_err(|e| {
            mlua::Error::runtime(format!(
                "failed to run '{}' on '{}': {}",
                state.program,
                self.host,
                chain(&e)
            ))
        })?;
        drop(cmd);

        if let (Some(bytes), Some(pipe)) = (input, child.stdin().take()) {
            redirect::feed(pipe, bytes);
        }

        let readers = match merged_reader {
            Some(reader) => (Some(reader), None),
            None => (
                child.stdout().take().map(redirect::read_to_end),
                child.stderr().take().map(redirect::read_to_end),
            ),
        };

        // `openssh::Child::wait` consumes the child, so it runs on its own task
        // from the start: the handle then survives an abandoned `:wait()`.
        let status = tokio::task::spawn_local(async move { child.wait().await });

        Ok(RemoteChild {
            host: self.host.clone(),
            program: state.program,
            status: Some(status),
            readers,
            captured,
        })
    }

    /// Run the command on a remote pty attached to this terminal, via the
    /// system `ssh` over the existing control master.
    async fn interactive(&self, lua: &Lua) -> LuaResult<Table> {
        use std::io::Write;
        let state = self.state.lock().unwrap().clone();
        if state.stdin.is_some() || state.stdout.is_some() || state.stderr.is_some() {
            return Err(mlua::Error::runtime(
                ":interactive() attaches the remote pty to this terminal as-is; \
                 it does not support :stdin()/:stdout()/:stderr()",
            ));
        }
        let words: Vec<String> = std::iter::once(&state.program)
            .chain(&state.args)
            .map(|w| sh_quote(w))
            .collect();

        let _guard = IgnoreInterrupts::new();
        let _ = std::io::stdout().flush();
        let mut cmd = tokio::process::Command::new("ssh");
        cmd.arg("-S")
            .arg(self.session.control_socket())
            .arg("-tt")
            .arg(&self.host)
            .arg(words.join(" "))
            .stdin(std::process::Stdio::inherit())
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit())
            .kill_on_drop(true);
        // SAFETY: only async-signal-safe `signal(2)` calls between fork and exec.
        unsafe {
            cmd.pre_exec(|| {
                libc::signal(libc::SIGINT, libc::SIG_DFL);
                libc::signal(libc::SIGQUIT, libc::SIG_DFL);
                Ok(())
            });
        }
        let mut child = cmd.spawn().map_err(|e| {
            mlua::Error::runtime(format!("failed to run ssh for '{}': {e}", self.host))
        })?;
        let status = child.wait().await.map_err(|e| {
            mlua::Error::runtime(format!("failed waiting for ssh to '{}': {e}", self.host))
        })?;
        let none = Captured::of(&Redirect::Null, &Redirect::Null);
        proc::result_table(lua, &none, status, &[], &[])
    }
}

/// The methods sshd's model has no room for, present only to fail with a
/// reason rather than being a confusing `nil` (and accepting any arguments,
/// so the reason is what the caller sees, not a conversion error).
fn unsupported<M: UserDataMethods<RemoteCommand>>(methods: &mut M, name: &'static str, why: &'static str) {
    methods.add_method(name, move |_, _, _: Variadic<Value>| {
        Err::<(), _>(mlua::Error::runtime(format!("ssh commands have no {name}(): {why}")))
    });
}

impl UserData for RemoteCommand {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_function("arg", |_, (ud, arg): (AnyUserData, mlua::LuaString)| {
            let arg = utf8(redirect::os_string(&arg))?;
            ud.borrow::<Self>()?.state.lock().unwrap().args.push(arg);
            Ok(ud)
        });

        methods.add_function("args", |_, (ud, args): (AnyUserData, Vec<mlua::LuaString>)| {
            let args: Vec<String> = args
                .iter()
                .map(|a| utf8(redirect::os_string(a)))
                .collect::<LuaResult<_>>()?;
            ud.borrow::<Self>()?.state.lock().unwrap().args.extend(args);
            Ok(ud)
        });

        methods.add_function("stdin", |_, (ud, source): (AnyUserData, Value)| {
            let source = stdin_from_value(&source)?;
            ud.borrow::<Self>()?.state.lock().unwrap().stdin = Some(source);
            Ok(ud)
        });

        methods.add_function("stdout", |_, (ud, target): (AnyUserData, Value)| {
            let redirect = redirect::redirect_from_value(&target)?;
            let this = ud.borrow::<Self>()?;
            let mut state = this.state.lock().unwrap();
            // Bound on assignment, as in the local module, so reassigning the
            // other stream later does not drag this one along.
            state.stdout = Some(bind_alias(redirect, state.stderr.as_ref(), Slot::Stdout)?);
            drop(state);
            drop(this);
            Ok(ud)
        });

        methods.add_function("stderr", |_, (ud, target): (AnyUserData, Value)| {
            let redirect = redirect::redirect_from_value(&target)?;
            let this = ud.borrow::<Self>()?;
            let mut state = this.state.lock().unwrap();
            state.stderr = Some(bind_alias(redirect, state.stdout.as_ref(), Slot::Stderr)?);
            drop(state);
            drop(this);
            Ok(ud)
        });

        // --- finalizers ---------------------------------------------------

        // Connects and execs remotely before returning (errors surface here);
        // the child's output is drained from now on, not from `:wait()`.
        methods.add_async_method("spawn", |_, this, ()| async move { this.spawn().await });

        methods.add_async_method("run", |lua, this, ()| async move {
            let mut child = this.spawn().await?;
            let (mut status, mut readers) = child.take_pieces()?;
            child.finish(&lua, &mut status, &mut readers).await
        });

        methods.add_async_method("interactive", |lua, this, ()| async move {
            this.interactive(&lua).await
        });

        // Present only to fail clearly: sshd controls the remote environment.
        const ENV_WHY: &str = "sshd controls the remote environment. Use an explicit remote \
             shell, e.g. :command({\"sh\", \"-c\", \"VAR=1 exec cmd\"})";
        for name in ["env", "envs", "env_remove", "env_clear"] {
            unsupported(methods, name, ENV_WHY);
        }
        unsupported(
            methods,
            "current_dir",
            "use an explicit remote shell, e.g. :command({\"sh\", \"-c\", \"cd /path && exec cmd\"})",
        );
        unsupported(
            methods,
            "exec",
            "there is no remote process image to replace; use :run() or :interactive()",
        );
    }
}

/// A running remote command, awaited with `:wait()`.
struct RemoteChild {
    host: String,
    program: String,
    /// The task awaiting the ssh child; `None` once waited for.
    status: Option<JoinHandle<Exit>>,
    readers: (Option<Reader>, Option<Reader>),
    captured: Captured,
}

/// Await a reader in place, so an abandoned wait leaves it for the guard.
async fn join_in_place(reader: &mut Option<Reader>, which: &str) -> LuaResult<Vec<u8>> {
    let Some(handle) = reader.as_mut() else {
        return Ok(Vec::new());
    };
    let bytes = handle
        .await
        .map_err(|e| mlua::Error::runtime(format!("{which} reader task failed: {e}")))?;
    *reader = None;
    Ok(bytes)
}

impl RemoteChild {
    fn take_pieces(&mut self) -> LuaResult<(Option<JoinHandle<Exit>>, (Option<Reader>, Option<Reader>))> {
        let status = self.status.take().ok_or_else(|| {
            mlua::Error::runtime("this remote child has already been waited for")
        })?;
        Ok((Some(status), (self.readers.0.take(), self.readers.1.take())))
    }

    /// Wait for the readers and the exit, and build the result table. Works on
    /// the borrowed pieces so the caller (a guard) still owns them if dropped.
    async fn finish(
        &self,
        lua: &Lua,
        status: &mut Option<JoinHandle<Exit>>,
        readers: &mut (Option<Reader>, Option<Reader>),
    ) -> LuaResult<Table> {
        let out = join_in_place(&mut readers.0, "stdout").await?;
        let err = join_in_place(&mut readers.1, "stderr").await?;

        let exit = status
            .as_mut()
            .expect("armed guard holds a status")
            .await
            .map_err(|e| mlua::Error::runtime(format!("wait task failed: {e}")))?;
        *status = None;
        let exit = match exit {
            Ok(st) => st,
            // ssh's own failure code and "command not found" are results, with
            // whatever output the command produced, not errors.
            Err(openssh::Error::RemoteProcessTerminated) => ExitStatus::from_raw(255 << 8),
            Err(openssh::Error::Remote(_)) => ExitStatus::from_raw(127 << 8),
            Err(e) => {
                return Err(mlua::Error::runtime(format!(
                    "failed waiting for '{}' on '{}': {}",
                    self.program,
                    self.host,
                    chain(&e)
                )));
            }
        };
        proc::result_table(lua, &self.captured, exit, &out, &err)
    }
}

/// Restores a child's moved-out pieces if its `:wait()` is abandoned, as
/// `proc`'s `WaitGuard` does, so the handle is not consumed by a wait that
/// never finished.
struct WaitGuard<'a> {
    ud: &'a AnyUserData,
    status: Option<JoinHandle<Exit>>,
    readers: (Option<Reader>, Option<Reader>),
}

impl Drop for WaitGuard<'_> {
    fn drop(&mut self) {
        let Some(status) = self.status.take() else {
            return; // the wait finished
        };
        if let Ok(mut this) = self.ud.borrow_mut::<RemoteChild>() {
            this.status = Some(status);
            this.readers = (self.readers.0.take(), self.readers.1.take());
        }
    }
}

impl RemoteChild {
    /// The scoped-borrow wait shared by `:wait()` and `:try_wait()`.
    async fn wait_via(lua: &Lua, ud: &AnyUserData) -> LuaResult<Table> {
        let (mut guard, child) = {
            let mut this = ud.borrow_mut::<Self>()?;
            let (status, readers) = this.take_pieces()?;
            let child = RemoteChild {
                host: this.host.clone(),
                program: this.program.clone(),
                status: None,
                readers: (None, None),
                captured: this.captured.clone(),
            };
            (WaitGuard { ud, status, readers }, child)
        };
        child
            .finish(lua, &mut guard.status, &mut guard.readers)
            .await
    }
}

impl UserData for RemoteChild {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        // Scoped borrow (not `add_async_method_mut`): an abandoned wait must
        // not keep the borrow and brick the handle.
        methods.add_async_function("wait", |lua, ud: AnyUserData| async move {
            Self::wait_via(&lua, &ud).await
        });

        // `nil` while still running, else what `:wait()` returns.
        methods.add_async_function("try_wait", |lua, ud: AnyUserData| async move {
            let running = {
                let this = ud.borrow::<Self>()?;
                this.status.as_ref().is_some_and(|h| !h.is_finished())
            };
            if running {
                return Ok(Value::Nil);
            }
            Ok(Value::Table(Self::wait_via(&lua, &ud).await?))
        });

        // SSH has no signal channel, so a truthful error beats a method that
        // pretends to work.
        methods.add_method("kill", |_, _, ()| {
            Err::<(), _>(mlua::Error::runtime(
                "SSH cannot signal a remote process. Stop it from a second command \
                 (e.g. :command({\"pkill\", \"-f\", pattern})) or ask it to exit \
                 itself; abandoning a remote child leaves it running",
            ))
        });
        methods.add_method("signal", |_, _, _: Variadic<Value>| {
            Err::<(), _>(mlua::Error::runtime(
                "ssh children have no signal(): SSH cannot signal a remote process",
            ))
        });
        methods.add_method("id", |_, _, ()| {
            Err::<(), _>(mlua::Error::runtime(
                "ssh children have no id(): the process is remote, there is no local pid",
            ))
        });
    }
}

/// Build the `ssh` module table.
pub fn module(lua: &Lua) -> LuaResult<Table> {
    let exports = lua.create_table()?;

    // ssh.connect(host[, { tofu = true }])
    exports.raw_set(
        "connect",
        lua.create_async_function(|_, (host, opts): (String, Option<Table>)| async move {
            let tofu = match opts {
                Some(t) => t.get::<Option<bool>>("tofu")?.unwrap_or(false),
                None => false,
            };
            Connection::connect(host, tofu).await
        })?,
    )?;

    Ok(exports)
}

/// Install the `ssh` module as a global.
pub fn register(lua: &Lua) -> LuaResult<()> {
    lua.globals().raw_set("ssh", module(lua)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Run `script` on a current-thread runtime with a `LocalSet`, as the host
    /// does. No ssh server is involved: these cover what needs none.
    fn eval(script: &str) -> LuaResult<()> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build test runtime");
        let local = tokio::task::LocalSet::new();
        local.block_on(&runtime, async {
            let lua = Lua::new();
            lua.globals().set("proc", proc::module(&lua)?)?;
            register(&lua)?;
            lua.load(script).exec_async().await
        })
    }

    #[test]
    fn quotes_only_what_needs_it() {
        assert_eq!(sh_quote("ls"), "ls");
        assert_eq!(sh_quote("a b"), "'a b'");
        assert_eq!(sh_quote("it's"), "'it'\\''s'");
        assert_eq!(sh_quote(""), "''");
        assert_eq!(sh_quote("$(rm -rf /)"), "'$(rm -rf /)'");
    }

    #[test]
    fn error_chain_includes_sources() {
        #[derive(Debug)]
        struct Outer(std::io::Error);
        impl std::fmt::Display for Outer {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "failed to connect")
            }
        }
        impl std::error::Error for Outer {
            fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                Some(&self.0)
            }
        }
        let e = Outer(std::io::Error::other("host key verification failed"));
        assert_eq!(chain(&e), "failed to connect: host key verification failed");
    }

    #[test]
    fn non_utf8_commands_are_rejected() {
        use std::os::unix::ffi::OsStringExt;
        assert!(utf8(std::ffi::OsString::from_vec(vec![0xff])).is_err());
    }

    #[test]
    fn unreachable_host_raises_with_a_reason() {
        let err = eval(r#"ssh.connect("nonexistent.invalid")"#).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("failed to connect to 'nonexistent.invalid'"), "got: {msg}");
        // The reason (or ssh's absence) follows, not a bare "failed to connect".
        assert!(msg.len() > "failed to connect to 'nonexistent.invalid': ".len(), "got: {msg}");
    }

    #[test]
    fn connect_options_must_be_a_table_of_the_right_shape() {
        assert!(eval(r#"ssh.connect("h", { tofu = "yes" })"#).is_err());
        assert!(eval(r#"ssh.connect("h", 5)"#).is_err());
    }

}
