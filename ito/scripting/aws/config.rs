//! Luau bindings for building an explicit AWS client, registered as
//! `aws.config`.
//!
//! `aws.ec2` and `aws.s3` read the ambient configuration, which is right for the
//! common case and wrong the moment one script has to span two regions or two
//! accounts. `aws.config()` is the escape hatch: a builder for the configuration
//! itself, finalized by `:build()` into a client carrying the same per-service
//! keys.
//!
//! ```lua
//! local eu = aws.config():region("eu-central-1"):profile("prod"):build()
//! local r, listing = eu.s3.list_objects("logs"):send()
//! local instances  = eu.ec2.describe_instances():name("web-*"):send()
//! ```
//!
//! `:build()` is where the credentials resolve, so an unusable profile raises at
//! the line that named it rather than at some later `:send()`. That is the one
//! deliberate difference from the ambient surface, which defers the same work to
//! first use to stay free for scripts that never call AWS.
//!
//! Every unset knob falls back to the ambient chain, so `aws.config()` with only
//! a region set keeps the machine's profile and credentials.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use mlua::{
    AnyUserData, Function, Lua, Result as LuaResult, UserData, UserDataFields, UserDataMethods,
};

use super::{ClientSource, autoscaling, ec2, s3};

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
}

/// Builder for an AWS configuration, finalized with `:build()`.
struct Config {
    state: Arc<Mutex<ConfigState>>,
}

impl UserData for Config {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_function("region", |_, (ud, v): (AnyUserData, String)| {
            ud.borrow::<Self>()?.state.lock().unwrap().region = Some(v);
            Ok(ud)
        });

        // The named profile from `~/.aws/config`. Its own region applies unless
        // `:region()` overrides it, matching how the CLI resolves the two.
        methods.add_function("profile", |_, (ud, v): (AnyUserData, String)| {
            ud.borrow::<Self>()?.state.lock().unwrap().profile = Some(v);
            Ok(ud)
        });

        // Point every service at another endpoint — localstack, minio, a VPC
        // endpoint. Applies to whichever services this client reaches.
        methods.add_function("endpoint_url", |_, (ud, v): (AnyUserData, String)| {
            ud.borrow::<Self>()?.state.lock().unwrap().endpoint_url = Some(v);
            Ok(ud)
        });

        // Seconds, fractional allowed, as the per-operation timeout. An
        // operation is the whole call including its retries.
        methods.add_function("timeout", |_, (ud, secs): (AnyUserData, f64)| {
            if !(secs.is_finite() && secs > 0.0) {
                return Err(mlua::Error::runtime(format!(
                    "timeout must be a positive number of seconds, got {secs}"
                )));
            }
            ud.borrow::<Self>()?.state.lock().unwrap().timeout =
                Some(Duration::from_secs_f64(secs));
            Ok(ud)
        });

        // Total attempts, not retries after the first: 1 disables retrying,
        // which is the SDK's own counting.
        methods.add_function("retries", |_, (ud, n): (AnyUserData, u32)| {
            if n == 0 {
                return Err(mlua::Error::runtime(
                    "retries must be at least 1 (the count includes the first attempt)",
                ));
            }
            ud.borrow::<Self>()?.state.lock().unwrap().retries = Some(n);
            Ok(ud)
        });

        // Async because resolving credentials is a round trip. The borrow is
        // scoped and the state cloned before the await: holding either across it
        // would fail the moment another task touched this builder.
        methods.add_async_function("build", |lua, ud: AnyUserData| async move {
            let state = ud.borrow::<Self>()?.state.lock().unwrap().clone();
            let sdk_config = load(state).await;
            Client::new(&lua, ClientSource::Explicit(std::rc::Rc::new(sdk_config)))
        });
    }
}

/// Resolve the configuration, letting every unset knob fall through to the
/// default provider chain.
async fn load(state: ConfigState) -> aws_config::SdkConfig {
    let mut loader = aws_config::defaults(aws_config::BehaviorVersion::latest());

    if let Some(region) = state.region {
        loader = loader.region(aws_config::Region::new(region));
    }
    if let Some(profile) = state.profile {
        loader = loader.profile_name(profile);
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

    loader.load().await
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
}

impl Client {
    fn new(lua: &Lua, source: ClientSource) -> LuaResult<Self> {
        Ok(Self {
            ec2: ec2::module(lua, source.clone())?,
            s3: s3::module(lua, source.clone())?,
            autoscaling: autoscaling::module(lua, source)?,
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
    }
}

/// Build the `aws.config` entry function.
pub fn function(lua: &Lua) -> LuaResult<Function> {
    lua.create_function(|_, ()| {
        Ok(Config {
            state: Arc::new(Mutex::new(ConfigState::default())),
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

    /// The builder returns itself, so the knobs chain.
    #[test]
    fn builder_methods_chain() {
        let lua = lua_with_config();
        lua.load(
            r#"
            local c = config():region("eu-central-1"):profile("p")
                :endpoint_url("http://localhost:4566"):timeout(5):retries(2)
            assert(c ~= nil)
        "#,
        )
        .exec()
        .unwrap();
    }

    /// A region alone is enough; every other knob falls through to the chain.
    #[test]
    fn state_records_only_what_was_set() {
        let lua = lua_with_config();
        let ud: AnyUserData = lua
            .load("return config():region('us-east-1')")
            .eval()
            .unwrap();
        let state = ud.borrow::<Config>().unwrap().state.lock().unwrap().clone();
        assert_eq!(state.region.as_deref(), Some("us-east-1"));
        assert!(state.profile.is_none());
        assert!(state.endpoint_url.is_none());
        assert!(state.timeout.is_none());
        assert!(state.retries.is_none());
    }

    /// Seconds, fractional allowed.
    #[test]
    fn timeout_takes_fractional_seconds() {
        let lua = lua_with_config();
        let ud: AnyUserData = lua.load("return config():timeout(1.5)").eval().unwrap();
        let state = ud.borrow::<Config>().unwrap().state.lock().unwrap().clone();
        assert_eq!(state.timeout, Some(Duration::from_millis(1500)));
    }

    /// A nonsense timeout is the script author's mistake, so it raises at the
    /// call rather than being silently clamped.
    #[test]
    fn rejects_non_positive_timeout() {
        let lua = lua_with_config();
        for bad in ["0", "-1"] {
            let err = lua
                .load(format!("return config():timeout({bad})"))
                .eval::<AnyUserData>()
                .unwrap_err()
                .to_string();
            assert!(err.contains("positive number of seconds"), "{err}");
        }
    }

    /// Zero attempts would mean "never call AWS", which is never what was meant.
    #[test]
    fn rejects_zero_retries() {
        let lua = lua_with_config();
        let err = lua
            .load("return config():retries(0)")
            .eval::<AnyUserData>()
            .unwrap_err()
            .to_string();
        assert!(err.contains("at least 1"), "{err}");
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
        let config = block_on(load(state));
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
        let config = block_on(load(ConfigState::default()));
        assert_eq!(config.endpoint_url(), None);
    }

    /// The built client carries both service surfaces, spelled the same as the
    /// globals.
    #[test]
    fn client_exposes_service_tables() {
        let lua = Lua::new();
        let config = block_on(load(ConfigState {
            region: Some("eu-west-1".into()),
            ..Default::default()
        }));
        let client = Client::new(&lua, ClientSource::Explicit(std::rc::Rc::new(config))).unwrap();
        lua.globals().raw_set("client", client).unwrap();

        lua.load(
            r#"
            assert(type(client.s3) == "table", "s3 must be a table")
            assert(type(client.ec2) == "table", "ec2 must be a table")
            assert(type(client.s3.list_objects) == "function")
            assert(type(client.ec2.describe_instances) == "function")
        "#,
        )
        .exec()
        .unwrap();
    }
}
