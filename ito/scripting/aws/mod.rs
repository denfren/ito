//! Luau bindings for AWS, registered as the `aws` global.
//!
//! One submodule per service, each installed as a key on the `aws` table
//! (`aws.ec2`, `aws.s3`, `aws.autoscaling`, `aws.cloudtrail`, `aws.cloudwatch`)
//! the way the SDK itself is organized — so a new service is a new key rather
//! than a new global.
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
//! Shared argument rules (helpers in this file): an id/name/filter list that is
//! empty, or holds an empty string, raises — AWS reads an empty list as "no
//! restriction", which would widen the query to the whole account; errors are
//! prefixed with the call (`<fn>: …`); a timestamp argument is Unix seconds or an
//! RFC 3339 string (`time_arg`); a bool knob is a real boolean.
//!
//! Result order is per service and documented there: `ec2`/`s3` return what the
//! API returns (S3 in key order), `autoscaling` likewise, `cloudtrail` is
//! newest-first and `cloudwatch` datapoints are oldest-first.
//!
//! # Two ways to reach a service
//!
//! `aws.ec2`, `aws.s3`, `aws.autoscaling`, `aws.cloudtrail` and `aws.cloudwatch`
//! are backed by the *ambient* configuration — whatever
//! `AWS_PROFILE`/`AWS_REGION`/`~/.aws/config`/SSO/IMDS resolve to — built lazily
//! on first use and then cached, so a script that never calls AWS pays nothing.
//!
//! `aws.config()` builds an explicit client instead, carrying the same five
//! keys:
//!
//! ```lua
//! local eu = aws.config():region("eu-central-1"):profile("prod"):build()
//! local r, listing = eu.s3.list_objects("logs"):send()
//! ```
//!
//! The two surfaces are the same functions over a different configuration, which
//! is what lets one script span regions or accounts.

pub mod autoscaling;
pub mod cloudtrail;
pub mod cloudwatch;
pub mod config;
pub mod ec2;
pub mod s3;

use std::error::Error as StdError;
use std::rc::Rc;

use mlua::{Lua, MultiValue, Result as LuaResult, Table, Value};

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
    string_list("argument", value)
}

/// [`value_to_string_list`] naming the call in its errors (`<ctx>: …`).
///
/// An empty list (or an empty string) raises: AWS reads an empty id/name/filter
/// list as "no restriction", so `ids({})` would silently widen the query to the
/// whole account.
pub(crate) fn string_list(ctx: &str, value: Value) -> LuaResult<Vec<String>> {
    let out = match value {
        Value::String(s) => vec![s.to_str()?.to_string()],
        Value::Table(t) => {
            let mut out = Vec::new();
            for v in t.sequence_values::<Value>() {
                match v? {
                    Value::String(s) => out.push(s.to_str()?.to_string()),
                    other => {
                        return Err(err_in(
                            ctx,
                            format!("expected an array of strings, found {}", other.type_name()),
                        ));
                    }
                }
            }
            out
        }
        other => {
            return Err(err_in(
                ctx,
                format!(
                    "expected string or array of strings, got {}",
                    other.type_name()
                ),
            ));
        }
    };
    if out.is_empty() {
        return Err(err_in(
            ctx,
            "expected at least one value, got an empty list",
        ));
    }
    if out.iter().any(String::is_empty) {
        return Err(err_in(ctx, "values must not be empty strings"));
    }
    Ok(out)
}

/// A single-string argument that must not be empty.
pub(crate) fn non_empty(ctx: &str, s: String) -> LuaResult<String> {
    if s.is_empty() {
        return Err(err_in(ctx, "expected a non-empty string"));
    }
    Ok(s)
}

/// A runtime error prefixed with the call it came from: `<ctx>: <msg>`.
pub(crate) fn err_in(ctx: &str, msg: impl std::fmt::Display) -> mlua::Error {
    mlua::Error::runtime(format!("{ctx}: {msg}"))
}

/// A timestamp argument as an SDK `DateTime`: Unix seconds (number) or an
/// RFC 3339 string carrying its own offset, the same spellings `time.*` takes.
pub(crate) fn time_arg(
    ctx: &str,
    name: &str,
    value: &Value,
) -> LuaResult<aws_smithy_types::DateTime> {
    let secs = match value {
        Value::Integer(n) => *n as f64,
        Value::Number(n) => *n,
        Value::String(s) => {
            let text = s.to_str()?;
            let ts = text.trim().parse::<jiff::Timestamp>().map_err(|e| {
                err_in(
                    ctx,
                    format!("'{name}' could not read '{text}' as an RFC 3339 timestamp ({e})"),
                )
            })?;
            return Ok(aws_smithy_types::DateTime::from_secs_and_nanos(
                ts.as_second(),
                ts.subsec_nanosecond() as u32,
            ));
        }
        other => {
            return Err(err_in(
                ctx,
                format!(
                    "'{name}' must be Unix seconds or an RFC 3339 string, got {}",
                    other.type_name()
                ),
            ));
        }
    };
    if !secs.is_finite() {
        return Err(err_in(
            ctx,
            format!("'{name}' must be a finite number of seconds since the epoch, got {secs}"),
        ));
    }
    Ok(aws_smithy_types::DateTime::from_secs_f64(secs))
}

/// Whether a service error is a "that does not exist" answer (`*.NotFound`),
/// which a describe reports as an empty result instead of raising.
pub(crate) fn is_not_found(
    err: &impl aws_smithy_types::error::metadata::ProvideErrorMetadata,
) -> bool {
    err.code().is_some_and(|c| c.ends_with(".NotFound"))
}

/// Mark a result list as an array so an empty listing stays `[]` when encoded.
///
/// Placeholder until the codecs' shared array marker lands (X-3): today it
/// returns the table unchanged, so call sites are already in place.
pub(crate) fn array(lua: &Lua, t: Table) -> Table {
    // Marks an (even empty) table as an array for the codecs; see `codec`.
    let _ = t.set_metatable(Some(mlua::LuaSerdeExt::array_metatable(lua)));
    t
}

/// Format a timestamp as RFC 3339, which keeps it comparable and printable in
/// Luau, which has no native date type.
pub(crate) fn date_to_string(dt: &aws_smithy_types::DateTime) -> Option<String> {
    dt.fmt(aws_smithy_types::date_time::Format::DateTime).ok()
}

/// Return `(envelope, data)`, the pair every `:send()` resolves to.
pub(crate) fn respond(lua: &Lua, ok: bool, data: Value) -> LuaResult<MultiValue> {
    let env = lua.create_table()?;
    env.raw_set("ok", ok)?;
    Ok(MultiValue::from_vec(vec![Value::Table(env), data]))
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
    Explicit {
        config: Rc<aws_config::SdkConfig>,
        /// `aws.config():path_style()`: S3 addresses buckets in the path (MinIO,
        /// LocalStack), which an `endpoint_url` alone does not imply.
        path_style: bool,
    },
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
    CloudTrail(aws_sdk_cloudtrail::Client),
    CloudWatch(aws_sdk_cloudwatch::Client),
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
            Self::Explicit { config, .. } => aws_sdk_autoscaling::Client::new(config),
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
            Self::Explicit { config, .. } => aws_sdk_ec2::Client::new(config),
        }
    }

    /// A CloudTrail client for this source.
    pub(crate) async fn cloudtrail(&self) -> aws_sdk_cloudtrail::Client {
        match self {
            Self::Ambient(lazy) => {
                match lazy
                    .get(|c| Client::CloudTrail(aws_sdk_cloudtrail::Client::new(c)))
                    .await
                {
                    Client::CloudTrail(c) => c,
                    _ => unreachable!("a cloudtrail cell only ever holds a cloudtrail client"),
                }
            }
            Self::Explicit { config, .. } => aws_sdk_cloudtrail::Client::new(config),
        }
    }

    /// A CloudWatch client for this source.
    pub(crate) async fn cloudwatch(&self) -> aws_sdk_cloudwatch::Client {
        match self {
            Self::Ambient(lazy) => {
                match lazy
                    .get(|c| Client::CloudWatch(aws_sdk_cloudwatch::Client::new(c)))
                    .await
                {
                    Client::CloudWatch(c) => c,
                    _ => unreachable!("a cloudwatch cell only ever holds a cloudwatch client"),
                }
            }
            Self::Explicit { config, .. } => aws_sdk_cloudwatch::Client::new(config),
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
            Self::Explicit { config, path_style } => aws_sdk_s3::Client::from_conf(
                aws_sdk_s3::config::Builder::from(&**config)
                    .force_path_style(*path_style)
                    .build(),
            ),
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
    aws.raw_set(
        "cloudtrail",
        cloudtrail::module(lua, ClientSource::ambient())?,
    )?;
    aws.raw_set(
        "cloudwatch",
        cloudwatch::module(lua, ClientSource::ambient())?,
    )?;
    aws.raw_set("config", config::function(lua)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn string_list_rejects_empty_and_blank() {
        let lua = Lua::new();
        let empty = Value::Table(lua.create_table().unwrap());
        assert!(
            string_list("f", empty)
                .unwrap_err()
                .to_string()
                .contains("f: expected at least one")
        );
        let blank = Value::String(lua.create_string("").unwrap());
        assert!(
            string_list("f", blank)
                .unwrap_err()
                .to_string()
                .contains("empty strings")
        );
        let one = Value::String(lua.create_string("a").unwrap());
        assert_eq!(string_list("f", one).unwrap(), vec!["a"]);
    }

    #[test]
    fn time_arg_takes_numbers_and_rfc3339() {
        let lua = Lua::new();
        let s = Value::String(lua.create_string("2026-01-01T00:00:00Z").unwrap());
        assert_eq!(time_arg("f", "t", &s).unwrap().secs(), 1767225600);
        assert_eq!(time_arg("f", "t", &Value::Integer(5)).unwrap().secs(), 5);
        let bad = Value::String(lua.create_string("nope").unwrap());
        assert!(
            time_arg("f", "t", &bad)
                .unwrap_err()
                .to_string()
                .starts_with("runtime error: f: 't'")
        );
        assert!(time_arg("f", "t", &Value::Nil).is_err());
    }
}
