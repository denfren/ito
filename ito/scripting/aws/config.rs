//! Luau bindings for building an explicit AWS client, registered as
//! `aws.config`.
//!
//! The `aws.<service>` tables read the ambient configuration, which is right for
//! the common case and wrong the moment one script has to span two regions or two
//! accounts. `aws.config()` is the escape hatch: a builder for the configuration
//! itself, finalized by `:build()` into a client carrying the same per-service
//! keys (`ec2`, `s3`, `autoscaling`, `cloudtrail`, `cloudwatch`).
//!
//! ```lua
//! local eu = aws.config():region("eu-central-1"):profile("prod"):build()
//! local r, listing = eu.s3.list_objects("logs"):send()
//! local r, instances = eu.ec2.describe_instances():name("web-*"):send()
//! ```
//!
//! Every setter returns a **new** builder and leaves the receiver untouched, so a
//! base configuration can be specialised twice without the two interfering.
//!
//! `:profile(name)` is authoritative: the credentials *and* the region come from
//! that profile alone, so `AWS_ACCESS_KEY_ID`/`AWS_REGION` in the environment
//! cannot send the script to another account (`:region()` still overrides the
//! region). `:build()` resolves those credentials, so an unusable profile raises
//! at the line that named it rather than at some later `:send()`. Without a
//! profile nothing is resolved eagerly — the ambient chain is used lazily, like
//! the `aws` globals, which keeps LocalStack-style setups with no credentials
//! working.
//!
//! Every unset knob falls back to the ambient chain, so `aws.config()` with only
//! a region set keeps the machine's profile and credentials.

use std::time::Duration;

use aws_sdk_s3::config::ProvideCredentials;
use mlua::{
    AnyUserData, Function, Lua, Result as LuaResult, UserData, UserDataFields, UserDataMethods,
    Value,
};

use super::{ClientSource, autoscaling, cloudtrail, cloudwatch, ec2, err_in, s3};

/// The knobs, accumulated by the builder methods.
///
/// All optional: an unset field is left to the default provider chain rather
/// than given a value here, so "not configured in the script" and "configured to
/// the same thing the environment says" stay distinguishable.
#[derive(Default, Clone)]
struct ConfigState {
    region: Option<String>,
    profile: Option<String>,
    endpoint_url: Option<String>,
    timeout: Option<Duration>,
    retries: Option<u32>,
    path_style: bool,
}

/// Builder for an AWS configuration, finalized with `:build()`. Immutable: each
/// setter yields a fresh builder.
struct Config {
    state: ConfigState,
}

/// A non-empty string argument, raising `aws.config:<name>: …` otherwise.
fn text(name: &str, v: Value) -> LuaResult<String> {
    let ctx = format!("aws.config:{name}");
    match v {
        Value::String(s) => {
            let s = s.to_str()?.to_string();
            if s.is_empty() {
                return Err(err_in(&ctx, "expected a non-empty string"));
            }
            Ok(s)
        }
        other => Err(err_in(
            &ctx,
            format!("expected a string, got {}", other.type_name()),
        )),
    }
}

/// Apply `f` to a copy of the receiver's state and return it as a new builder.
fn with(lua: &Lua, ud: &AnyUserData, f: impl FnOnce(&mut ConfigState)) -> LuaResult<AnyUserData> {
    let mut state = ud.borrow::<Config>()?.state.clone();
    f(&mut state);
    lua.create_userdata(Config { state })
}

impl UserData for Config {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_function("region", |lua, (ud, v): (AnyUserData, Value)| {
            let v = text("region", v)?;
            with(lua, &ud, |s| s.region = Some(v))
        });

        // The named profile from `~/.aws/config`. Its own region applies unless
        // `:region()` overrides it, matching how the CLI resolves the two.
        methods.add_function("profile", |lua, (ud, v): (AnyUserData, Value)| {
            let v = text("profile", v)?;
            with(lua, &ud, |s| s.profile = Some(v))
        });

        // Point every service at another endpoint — localstack, minio, a VPC
        // endpoint. Applies to whichever services this client reaches.
        methods.add_function("endpoint_url", |lua, (ud, v): (AnyUserData, Value)| {
            let v = text("endpoint_url", v)?;
            with(lua, &ud, |s| s.endpoint_url = Some(v))
        });

        // Address S3 buckets in the path rather than the host name, which
        // MinIO/LocalStack need next to `:endpoint_url()`.
        methods.add_function("path_style", |lua, (ud, v): (AnyUserData, Option<bool>)| {
            with(lua, &ud, |s| s.path_style = v.unwrap_or(true))
        });

        // Seconds, fractional allowed, as the per-operation timeout. An
        // operation is the whole call including its retries.
        methods.add_function("timeout", |lua, (ud, secs): (AnyUserData, f64)| {
            if !(secs.is_finite() && secs > 0.0 && secs < 1e9) {
                return Err(err_in(
                    "aws.config:timeout",
                    format!("expected a positive number of seconds, got {secs}"),
                ));
            }
            with(lua, &ud, |s| {
                s.timeout = Some(Duration::from_secs_f64(secs))
            })
        });

        // Total attempts, not retries after the first: 1 disables retrying,
        // which is the SDK's own counting.
        methods.add_function("retries", |lua, (ud, n): (AnyUserData, Value)| {
            let n = match n {
                Value::Integer(i) => i as f64,
                Value::Number(f) => f,
                other => {
                    return Err(err_in(
                        "aws.config:retries",
                        format!("expected an integer, got {}", other.type_name()),
                    ));
                }
            };
            if !(n.fract() == 0.0 && (1.0..=u32::MAX as f64).contains(&n)) {
                return Err(err_in(
                    "aws.config:retries",
                    format!(
                        "expected an integer of at least 1 (the count includes the \
                         first attempt), got {n}"
                    ),
                ));
            }
            with(lua, &ud, |s| s.retries = Some(n as u32))
        });

        // Async because resolving credentials is a round trip. The borrow is
        // scoped and the state cloned before the await: holding either across it
        // would fail the moment another task touched this builder.
        methods.add_async_function("build", |lua, ud: AnyUserData| async move {
            let state = ud.borrow::<Self>()?.state.clone();
            let path_style = state.path_style;
            let sdk_config = load(state)
                .await
                .map_err(|e| err_in("aws.config:build", e))?;
            Client::new(
                &lua,
                ClientSource::Explicit {
                    config: std::rc::Rc::new(sdk_config),
                    path_style,
                },
            )
        });
    }
}

/// Resolve the configuration, letting every unset knob fall through to the
/// default provider chain — except under `:profile()`, which pins credentials
/// and region to that profile so the environment cannot override it, and
/// resolves the credentials now so a bad profile fails here.
async fn load(state: ConfigState) -> Result<aws_config::SdkConfig, String> {
    let mut loader = aws_config::defaults(aws_config::BehaviorVersion::latest());

    if let Some(profile) = &state.profile {
        // The default chain tries the environment before the profile; naming the
        // providers directly drops that precedence. A hand-built credentials
        // provider is not `configure`d by the loader, so an assume-role profile's
        // STS client would find no region: hand it the resolved one.
        let region_provider = aws_config::profile::ProfileFileRegionProvider::builder()
            .profile_name(profile.clone())
            .build();
        let region = match &state.region {
            Some(r) => Some(aws_config::Region::new(r.clone())),
            None => aws_config::meta::region::ProvideRegion::region(&region_provider).await,
        };
        let provider_config =
            aws_config::provider_config::ProviderConfig::without_region().with_region(region);
        loader = loader
            .profile_name(profile.clone())
            .credentials_provider(
                aws_config::profile::ProfileFileCredentialsProvider::builder()
                    .configure(&provider_config)
                    .profile_name(profile.clone())
                    .build(),
            )
            .region(region_provider);
    }
    // After the profile's region provider, so an explicit region wins.
    if let Some(region) = state.region {
        loader = loader.region(aws_config::Region::new(region));
    }
    if let Some(url) = state.endpoint_url {
        loader = loader.endpoint_url(url);
    }
    if let Some(timeout) = state.timeout {
        loader = loader.timeout_config(
            aws_config::timeout::TimeoutConfig::builder()
                .operation_timeout(timeout)
                .build(),
        );
    }
    if let Some(attempts) = state.retries {
        loader = loader
            .retry_config(aws_config::retry::RetryConfig::standard().with_max_attempts(attempts));
    }

    let config = loader.load().await;
    if let (Some(profile), Some(creds)) = (&state.profile, config.credentials_provider()) {
        creds.provide_credentials().await.map_err(|e| {
            let e: super::BoxError = e.into();
            format!("profile '{profile}': {}", super::format_error_chain(&e))
        })?;
    }
    Ok(config)
}

/// A configured client: the same per-service surface as the `aws` global, over
/// one explicitly resolved configuration.
///
/// The service tables are built once here rather than lazily per access, because
/// building one is just a table of closures — the expensive part, the
/// configuration, has already been resolved by the time this exists.
struct Client {
    ec2: mlua::Table,
    s3: mlua::Table,
    autoscaling: mlua::Table,
    cloudtrail: mlua::Table,
    cloudwatch: mlua::Table,
}

impl Client {
    fn new(lua: &Lua, source: ClientSource) -> LuaResult<Self> {
        Ok(Self {
            ec2: ec2::module(lua, source.clone())?,
            s3: s3::module(lua, source.clone())?,
            autoscaling: autoscaling::module(lua, source.clone())?,
            cloudtrail: cloudtrail::module(lua, source.clone())?,
            cloudwatch: cloudwatch::module(lua, source)?,
        })
    }
}

impl UserData for Client {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        // Fields rather than methods, so `client.s3.list_objects(..)` reads the
        // same as `aws.s3.list_objects(..)` — one surface, two configurations.
        fields.add_field_method_get("ec2", |_, this| Ok(this.ec2.clone()));
        fields.add_field_method_get("s3", |_, this| Ok(this.s3.clone()));
        fields.add_field_method_get("autoscaling", |_, this| Ok(this.autoscaling.clone()));
        fields.add_field_method_get("cloudtrail", |_, this| Ok(this.cloudtrail.clone()));
        fields.add_field_method_get("cloudwatch", |_, this| Ok(this.cloudwatch.clone()));
    }
}

/// Build the `aws.config` entry function.
pub fn function(lua: &Lua) -> LuaResult<Function> {
    lua.create_function(|_, ()| {
        Ok(Config {
            state: ConfigState::default(),
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The runtime shape module tests need: current-thread plus a `LocalSet`,
    /// since `build` is async and credential resolution needs a reactor.
    fn block_on<F: std::future::Future>(f: F) -> F::Output {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let local = tokio::task::LocalSet::new();
        local.block_on(&rt, f)
    }

    fn lua_with_config() -> Lua {
        let lua = Lua::new();
        lua.globals()
            .raw_set("config", function(&lua).unwrap())
            .unwrap();
        lua
    }

    fn state_of(lua: &Lua, src: &str) -> ConfigState {
        let ud: AnyUserData = lua.load(src).eval().unwrap();
        ud.borrow::<Config>().unwrap().state.clone()
    }

    fn err_of(lua: &Lua, src: &str) -> String {
        lua.load(src).eval::<AnyUserData>().unwrap_err().to_string()
    }

    /// The builder returns a builder, so the knobs chain.
    #[test]
    fn builder_methods_chain() {
        let lua = lua_with_config();
        let s = state_of(
            &lua,
            r#"return config():region("eu-central-1"):profile("p")
                :endpoint_url("http://localhost:4566"):timeout(5):retries(2):path_style()"#,
        );
        assert_eq!(s.region.as_deref(), Some("eu-central-1"));
        assert_eq!(s.profile.as_deref(), Some("p"));
        assert_eq!(s.retries, Some(2));
        assert!(s.path_style);
    }

    /// Setters copy: specialising a shared base twice must not cross-talk.
    #[test]
    fn setters_do_not_mutate_the_receiver() {
        let lua = lua_with_config();
        lua.load(
            r#"
            base = config():region("us-east-1")
            a = base:region("eu-west-1")
            b = base:profile("p")
        "#,
        )
        .exec()
        .unwrap();
        let get = |n: &str| {
            let ud: AnyUserData = lua.globals().get(n).unwrap();
            let s = ud.borrow::<Config>().unwrap().state.clone();
            s
        };
        assert_eq!(get("base").region.as_deref(), Some("us-east-1"));
        assert!(get("base").profile.is_none());
        assert_eq!(get("a").region.as_deref(), Some("eu-west-1"));
        assert_eq!(get("b").region.as_deref(), Some("us-east-1"));
        assert_eq!(get("b").profile.as_deref(), Some("p"));
    }

    /// A region alone is enough; every other knob falls through to the chain.
    #[test]
    fn state_records_only_what_was_set() {
        let lua = lua_with_config();
        let state = state_of(&lua, "return config():region('us-east-1')");
        assert_eq!(state.region.as_deref(), Some("us-east-1"));
        assert!(state.profile.is_none());
        assert!(state.endpoint_url.is_none());
        assert!(state.timeout.is_none());
        assert!(state.retries.is_none());
        assert!(!state.path_style);
    }

    /// Seconds, fractional allowed.
    #[test]
    fn timeout_takes_fractional_seconds() {
        let lua = lua_with_config();
        let state = state_of(&lua, "return config():timeout(1.5)");
        assert_eq!(state.timeout, Some(Duration::from_millis(1500)));
    }

    /// A nonsense timeout is the script author's mistake, so it raises at the
    /// call rather than being silently clamped.
    #[test]
    fn rejects_non_positive_timeout() {
        let lua = lua_with_config();
        for bad in ["0", "-1", "1e30"] {
            let err = err_of(&lua, &format!("return config():timeout({bad})"));
            assert!(err.contains("aws.config:timeout"), "{err}");
            assert!(err.contains("positive number of seconds"), "{err}");
        }
    }

    /// Zero, fractional or non-numeric attempts all raise the same way.
    #[test]
    fn rejects_bad_retries() {
        let lua = lua_with_config();
        for bad in ["0", "1.5", "-2", "'x'", "nil"] {
            let err = err_of(&lua, &format!("return config():retries({bad})"));
            assert!(err.contains("aws.config:retries"), "{bad}: {err}");
        }
    }

    /// An empty string would fail far from the call that wrote it.
    #[test]
    fn rejects_empty_strings() {
        let lua = lua_with_config();
        for m in ["region", "profile", "endpoint_url"] {
            let err = err_of(&lua, &format!("return config():{m}('')"));
            assert!(err.contains("non-empty"), "{m}: {err}");
        }
    }

    /// The knobs reach the loader. No network: resolving a region and an endpoint
    /// is local, and the credential chain is not consulted until a call is made.
    #[test]
    fn load_applies_region_and_endpoint() {
        let state = ConfigState {
            region: Some("ap-south-1".into()),
            endpoint_url: Some("http://localhost:4566".into()),
            ..Default::default()
        };
        let config = block_on(load(state)).unwrap();
        assert_eq!(
            config.region().map(|r| r.to_string()).as_deref(),
            Some("ap-south-1")
        );
        assert_eq!(config.endpoint_url(), Some("http://localhost:4566"));
    }

    /// An unset region still resolves through the chain rather than being forced
    /// to some default here.
    #[test]
    fn load_leaves_unset_knobs_to_the_chain() {
        let config = block_on(load(ConfigState::default())).unwrap();
        assert_eq!(config.endpoint_url(), None);
    }

    /// A profile that does not exist fails at `load` (i.e. `:build()`), not at
    /// the first call.
    #[test]
    fn load_raises_on_unknown_profile() {
        let state = ConfigState {
            profile: Some("ito-no-such-profile".into()),
            region: Some("eu-west-1".into()),
            ..Default::default()
        };
        let err = block_on(load(state)).unwrap_err();
        assert!(err.contains("ito-no-such-profile"), "{err}");
    }

    /// The built client carries every service surface, spelled the same as the
    /// globals.
    #[test]
    fn client_exposes_service_tables() {
        let lua = Lua::new();
        let config = block_on(load(ConfigState {
            region: Some("eu-west-1".into()),
            ..Default::default()
        }))
        .unwrap();
        let client = Client::new(
            &lua,
            ClientSource::Explicit {
                config: std::rc::Rc::new(config),
                path_style: false,
            },
        )
        .unwrap();
        lua.globals().raw_set("client", client).unwrap();

        lua.load(
            r#"
            assert(type(client.s3) == "table", "s3 must be a table")
            assert(type(client.ec2) == "table", "ec2 must be a table")
            assert(type(client.autoscaling) == "table", "autoscaling must be a table")
            assert(type(client.cloudtrail) == "table", "cloudtrail must be a table")
            assert(type(client.cloudwatch) == "table", "cloudwatch must be a table")
            assert(type(client.s3.list_objects) == "function")
            assert(type(client.ec2.describe_instances) == "function")
            assert(type(client.cloudtrail.lookup_events) == "function")
            assert(type(client.cloudwatch.get_metric_statistics) == "function")
            assert(type(client.cloudwatch.list_metrics) == "function")
        "#,
        )
        .exec()
        .unwrap();
    }
}
