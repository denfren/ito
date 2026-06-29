//! Luau bindings for AWS, registered as the `aws` global.
//!
//! One submodule per service, each installed as a key on the `aws` table
//! (`aws.ec2`, `aws.s3`, `aws.autoscaling`) the way the SDK itself is
//! organized — so a new service is a new key rather than a new global.
//!
//! Auto Scaling gets its own key despite the "EC2 Auto Scaling" branding: it is
//! a separate API with its own endpoint, SDK crate and IAM prefix, so nesting it
//! under `ec2` would be the one place the table stops mirroring the SDK.
//!
//! Every service follows the same two rules:
//!   * required arguments are passed to the entry function, positionally when
//!     there are at most two and their order is unambiguous, otherwise as a
//!     table
//!   * the call returns a mutable builder for the optional parts, finalized by
//!     `:send()`
//!
//! Failures that are bugs rather than answers — bad credentials, denied access,
//! connectivity, a malformed request — always raise a Luau error. A collection
//! that matched nothing is an empty table, never nil.
//!
//! Where a service can answer "that does not exist", it says so in its return
//! value instead of raising; `s3` does this and documents the shape.
//!
//! # Two ways to reach a service
//!
//! `aws.ec2` and `aws.s3` are backed by the *ambient* configuration — whatever
//! `AWS_PROFILE`/`AWS_REGION`/`~/.aws/config`/SSO/IMDS resolve to — built lazily
//! on first use and then cached, so a script that never calls AWS pays nothing.
//!
//! `aws.config()` builds an explicit client instead, carrying the same `s3` and
//! `ec2` keys:
//!
//! ```lua
//! local eu = aws.config():region("eu-central-1"):profile("prod"):build()
//! local r, listing = eu.s3.list_objects("logs"):send()
//! ```
//!
//! The two surfaces are the same functions over a different configuration, which
//! is what lets one script span regions or accounts.

pub mod autoscaling;
pub mod config;
pub mod ec2;
pub mod s3;

use std::error::Error as StdError;
use std::rc::Rc;

use mlua::{Lua, Result as LuaResult, Table, Value};

/// Any error on the way to a Luau error.
///
/// The SDK's own error types are only ever flattened into a message by
/// [`format_error_chain`], so nothing here needs a richer error type.
pub(crate) type BoxError = Box<dyn StdError + Send + Sync>;

/// Flatten an error and its source chain into one message. The AWS SDK reports
/// top-level errors as opaque strings like "dispatch failure" and puts the
/// actionable cause (missing credentials, auth failure) in the sources.
pub(crate) fn format_error_chain(err: &BoxError) -> String {
    let mut parts = vec![err.to_string()];
    let mut source: Option<&(dyn StdError + 'static)> = err.source();
    while let Some(e) = source {
        let msg = e.to_string();
        // Skip repeats; nested SDK errors often restate their parent verbatim.
        if parts.last().map(String::as_str) != Some(msg.as_str()) {
            parts.push(msg);
        }
        source = e.source();
    }
    parts.join(": ")
}

/// Turn any SDK error into a Luau error with the whole chain in its message.
pub(crate) fn lua_err(err: impl Into<BoxError>) -> mlua::Error {
    mlua::Error::runtime(format_error_chain(&err.into()))
}

/// Accept either a single string or an array of strings, so `:tag("Name", "web")`
/// and `:tag("Name", {"web", "api"})` both work.
pub(crate) fn value_to_string_list(value: Value) -> LuaResult<Vec<String>> {
    match value {
        Value::String(s) => Ok(vec![s.to_str()?.to_string()]),
        Value::Table(t) => {
            let mut out = Vec::new();
            for pair in t.sequence_values::<String>() {
                out.push(pair?);
            }
            Ok(out)
        }
        other => Err(mlua::Error::runtime(format!(
            "expected string or array of strings, got {}",
            other.type_name()
        ))),
    }
}

/// A lazily built, then cached, ambient service client.
///
/// The client is built on first use rather than up front: resolving credentials
/// costs a round trip (IMDS, SSO, STS), and most scripts never touch a given
/// service. It is then cached, so a script making many calls pays once.
///
/// A tokio mutex rather than a `OnceCell`, because the initializer is async:
/// concurrent first calls from several tasks wait for one initialization instead
/// of racing to build several clients. `Rc` rather than `Arc` because the value
/// is only ever touched from the one thread the Lua state lives on.
pub(crate) struct LazyClient<C> {
    cell: tokio::sync::Mutex<Option<C>>,
}

impl<C: Clone> LazyClient<C> {
    fn new() -> Self {
        Self {
            cell: tokio::sync::Mutex::new(None),
        }
    }

    /// The cached client, building it from the ambient AWS config on first call.
    async fn get(&self, build: impl FnOnce(&aws_config::SdkConfig) -> C) -> C {
        let mut guard = self.cell.lock().await;
        match &*guard {
            Some(client) => client.clone(),
            None => {
                let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
                let built = build(&config);
                *guard = Some(built.clone());
                built
            }
        }
    }
}

/// Where a service module gets its client from.
///
/// The two arms differ only in *when* the credential resolution happens, which
/// is the whole point of the distinction: the ambient surface must stay free for
/// a script that never calls AWS, while an explicit `aws.config():build()` is an
/// explicit act whose failure belongs at the line that named the profile.
///
/// `Rc` rather than `Arc` throughout: both live only on the thread the Lua state
/// does.
#[derive(Clone)]
pub(crate) enum ClientSource {
    /// The process-wide default configuration, resolved on first use and cached.
    Ambient(Rc<LazyClient<Client>>),
    /// A configuration already resolved by `aws.config():build()`.
    Explicit(Rc<aws_config::SdkConfig>),
}

/// The one service client an [`ClientSource::Ambient`] cell holds.
///
/// A single enum rather than a generic `LazyClient<C>` per service, because a
/// `ClientSource` is a concrete type handed to every service module and so
/// cannot be monomorphized over one of them. The other arm is unreachable
/// because [`register`] gives each service its own cell, so the same call both
/// builds and reads one variant.
#[derive(Clone)]
pub(crate) enum Client {
    Ec2(aws_sdk_ec2::Client),
    S3(aws_sdk_s3::Client),
    AutoScaling(aws_sdk_autoscaling::Client),
}

impl ClientSource {
    /// An Auto Scaling client for this source.
    pub(crate) async fn autoscaling(&self) -> aws_sdk_autoscaling::Client {
        match self {
            Self::Ambient(lazy) => {
                match lazy
                    .get(|c| Client::AutoScaling(aws_sdk_autoscaling::Client::new(c)))
                    .await
                {
                    Client::AutoScaling(c) => c,
                    _ => unreachable!("an autoscaling cell only ever holds an autoscaling client"),
                }
            }
            Self::Explicit(config) => aws_sdk_autoscaling::Client::new(config),
        }
    }

    /// An EC2 client for this source.
    pub(crate) async fn ec2(&self) -> aws_sdk_ec2::Client {
        match self {
            Self::Ambient(lazy) => {
                match lazy.get(|c| Client::Ec2(aws_sdk_ec2::Client::new(c))).await {
                    Client::Ec2(c) => c,
                    _ => unreachable!("an ec2 cell only ever holds an ec2 client"),
                }
            }
            Self::Explicit(config) => aws_sdk_ec2::Client::new(config),
        }
    }

    /// An S3 client for this source.
    pub(crate) async fn s3(&self) -> aws_sdk_s3::Client {
        match self {
            Self::Ambient(lazy) => match lazy.get(|c| Client::S3(aws_sdk_s3::Client::new(c))).await
            {
                Client::S3(c) => c,
                _ => unreachable!("an s3 cell only ever holds an s3 client"),
            },
            Self::Explicit(config) => aws_sdk_s3::Client::new(config),
        }
    }

    /// A fresh ambient source — one cache cell, so one service's clients are
    /// built once between them.
    pub(crate) fn ambient() -> Self {
        Self::Ambient(Rc::new(LazyClient::new()))
    }
}

/// Install the `aws` global: one key per service over the ambient configuration,
/// plus `config` for building an explicit client.
pub fn register(lua: &Lua) -> LuaResult<()> {
    let globals = lua.globals();
    let aws: Table = match globals.raw_get("aws")? {
        Value::Table(existing) => existing,
        _ => {
            let created = lua.create_table()?;
            globals.raw_set("aws", created.clone())?;
            created
        }
    };
    // Each service gets its own cell: they cache different client types, and a
    // script touching only one must not pay for the other.
    aws.raw_set("ec2", ec2::module(lua, ClientSource::ambient())?)?;
    aws.raw_set("s3", s3::module(lua, ClientSource::ambient())?)?;
    aws.raw_set(
        "autoscaling",
        autoscaling::module(lua, ClientSource::ambient())?,
    )?;
    aws.raw_set("config", config::function(lua)?)?;
    Ok(())
}
