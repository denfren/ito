//! Luau bindings for AWS CloudTrail, registered as `aws.cloudtrail`.
//!
//! Follows the conventions documented on the parent module: a request is
//! configured through a builder finalized by `:send()`, and every `:send()`
//! either raises or returns `(envelope, data)`.
//!
//! # `lookup_events`
//!
//! The one call this module exposes: "what happened, and who/what did it" —
//! the API call history behind a change, as opposed to `aws.autoscaling`'s
//! own activity log, which explains *what* a group did but not *who* asked
//! for it. `LookupEvents` reads from CloudTrail's default 90-day event
//! history; it needs no trail or S3 bucket configured, unlike a full trail
//! export.
//!
//! ```lua
//! local r, events = aws.cloudtrail.lookup_events()
//!     :resource_name("web-prod")
//!     :start_time(time.now() - time.hours(1))
//!     :send()
//!
//! for _, e in ipairs(events) do
//!     print(e.event_time, e.username, e.event_name)
//! end
//! ```
//!
//! # `:max_events(n)`
//!
//! A lookup has no natural bound: account-wide and time-unbounded, it is the
//! whole 90-day history. `:max_events(n)` caps the total across pages,
//! stopping the walk once `n` events are in hand.
//!
//! # The one attribute per lookup
//!
//! CloudTrail's own `LookupAttributes` accepts at most one attribute per
//! call — asking for two is a service-side error, not a client-side
//! limitation this module works around. `:filter(<key>, <value>)` is the one
//! escape hatch. The typed helpers (`:event_name`, `:username`, …) are sugar
//! over the same single slot, and setting it a second time **raises** rather
//! than silently replacing the first.
//!
//! # `raw`
//!
//! Each event's `cloud_trail_event` arrives from the API as a JSON string;
//! `raw` is that string **parsed** into Luau data (keys stay CloudTrail's
//! camelCase; JSON `null` fields are absent), best-effort — a string
//! that fails to parse (never expected, but not asserted either) comes back
//! as the raw string rather than failing the whole lookup, the same
//! trade-off `aws.s3`'s body parsing makes.

use aws_sdk_cloudtrail::Client;
use aws_sdk_cloudtrail::types::{Event, LookupAttribute, LookupAttributeKey};
use mlua::{
    AnyUserData, Lua, LuaSerdeExt, Result as LuaResult, Table, UserData, UserDataMethods, Value,
};
use std::sync::{Arc, Mutex};

use super::{BoxError, ClientSource, date_to_string, lua_err, respond};

/// Shared request state, mutated in place by the builder methods.
#[derive(Default, Clone)]
struct LookupEventsState {
    /// At most one, per the API's own contract — the last call wins.
    attribute: Option<LookupAttribute>,
    start_time: Option<aws_smithy_types::DateTime>,
    end_time: Option<aws_smithy_types::DateTime>,
    /// Replaces the category rather than adding to it: omitting
    /// `EventCategory` returns management events, setting it returns Insights
    /// events, and no request returns both.
    insights: bool,
    max_events: Option<usize>,
}

/// Builder for `LookupEvents`, finalized with `:send()`.
struct LookupEvents {
    client: Client,
    state: Arc<Mutex<LookupEventsState>>,
}

/// Pause between pages: CloudTrail allows 2 lookups per second.
const PAGE_PAUSE: std::time::Duration = std::time::Duration::from_millis(500);

fn time_arg(name: &str, value: &Value) -> LuaResult<aws_smithy_types::DateTime> {
    super::time_arg("lookup_events", name, value)
}

/// Set the request's one lookup attribute; a second one raises.
fn set_attribute(ud: &AnyUserData, key: LookupAttributeKey, value: String) -> LuaResult<()> {
    let attr = LookupAttribute::builder()
        .attribute_key(key)
        .attribute_value(value)
        .build()
        .map_err(|e| mlua::Error::runtime(e.to_string()))?;
    let this = ud.borrow::<LookupEvents>()?;
    let mut state = this.state.lock().unwrap();
    if let Some(prev) = &state.attribute {
        return Err(mlua::Error::runtime(format!(
            "lookup_events: CloudTrail accepts one lookup attribute per request; '{}' is already set",
            prev.attribute_key.as_str()
        )));
    }
    state.attribute = Some(attr);
    Ok(())
}

/// A strict boolean: `true`/`false`, with nil meaning `true`. Truthiness is
/// not accepted, so `"false"` cannot silently read as `true`.
fn bool_arg(what: &str, value: Option<Value>) -> LuaResult<bool> {
    match value {
        None | Some(Value::Nil) => Ok(true),
        Some(Value::Boolean(b)) => Ok(b),
        Some(other) => Err(mlua::Error::runtime(format!(
            "lookup_events: '{what}' expects a boolean, got {}",
            other.type_name()
        ))),
    }
}

impl UserData for LookupEvents {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        // --- typed helpers over the one lookup attribute -------------------
        // CloudTrail accepts at most one `LookupAttribute` per request; each
        // of these sets that one slot, so the last call before `:send()`
        // wins rather than accumulating.

        methods.add_function("event_name", |_, (ud, v): (AnyUserData, String)| {
            set_attribute(&ud, LookupAttributeKey::EventName, v)?;
            Ok(ud)
        });

        methods.add_function("event_id", |_, (ud, v): (AnyUserData, String)| {
            set_attribute(&ud, LookupAttributeKey::EventId, v)?;
            Ok(ud)
        });

        methods.add_function("event_source", |_, (ud, v): (AnyUserData, String)| {
            set_attribute(&ud, LookupAttributeKey::EventSource, v)?;
            Ok(ud)
        });

        methods.add_function("username", |_, (ud, v): (AnyUserData, String)| {
            set_attribute(&ud, LookupAttributeKey::Username, v)?;
            Ok(ud)
        });

        methods.add_function("access_key_id", |_, (ud, v): (AnyUserData, String)| {
            set_attribute(&ud, LookupAttributeKey::AccessKeyId, v)?;
            Ok(ud)
        });

        methods.add_function("resource_name", |_, (ud, v): (AnyUserData, String)| {
            set_attribute(&ud, LookupAttributeKey::ResourceName, v)?;
            Ok(ud)
        });

        methods.add_function("resource_type", |_, (ud, v): (AnyUserData, String)| {
            set_attribute(&ud, LookupAttributeKey::ResourceType, v)?;
            Ok(ud)
        });

        methods.add_function("read_only", |_, (ud, v): (AnyUserData, Option<Value>)| {
            let v = bool_arg(":read_only()", v)?;
            set_attribute(&ud, LookupAttributeKey::ReadOnly, v.to_string())?;
            Ok(ud)
        });

        // --- generic escape hatch -------------------------------------------
        // Any attribute key by name, including ones without a typed helper
        // above (kept in step should CloudTrail add one).
        methods.add_function(
            "filter",
            |_, (ud, key, value): (AnyUserData, String, String)| {
                let k = LookupAttributeKey::from(key.as_str());
                if !LookupAttributeKey::values().contains(&key.as_str()) {
                    return Err(mlua::Error::runtime(format!(
                        "lookup_events: unknown attribute key '{key}', expected one of {}",
                        LookupAttributeKey::values().join(", ")
                    )));
                }
                set_attribute(&ud, k, value)?;
                Ok(ud)
            },
        );

        // --- time bounds -----------------------------------------------------

        methods.add_function("start_time", |_, (ud, t): (AnyUserData, Value)| {
            let dt = time_arg("start_time", &t)?;
            ud.borrow::<Self>()?.state.lock().unwrap().start_time = Some(dt);
            Ok(ud)
        });

        methods.add_function("end_time", |_, (ud, t): (AnyUserData, Value)| {
            let dt = time_arg("end_time", &t)?;
            ud.borrow::<Self>()?.state.lock().unwrap().end_time = Some(dt);
            Ok(ud)
        });

        // Swaps the category rather than widening it: a lookup returns
        // management events or Insights events, never both.
        // `:insights(false)` undoes it.
        methods.add_function("insights", |_, (ud, on): (AnyUserData, Option<Value>)| {
            let on = bool_arg(":insights()", on)?;
            ud.borrow::<Self>()?.state.lock().unwrap().insights = on;
            Ok(ud)
        });

        // A total across pages, not a page size: the walk stops once `n`
        // events are in hand. There is no natural bound otherwise -- an
        // account-wide, time-unbounded lookup is the whole 90-day history.
        methods.add_function("max_events", |_, (ud, n): (AnyUserData, i64)| {
            if n <= 0 {
                return Err(mlua::Error::runtime(format!(
                    "lookup_events: ':max_events()' must be positive, got {n}"
                )));
            }
            ud.borrow::<Self>()?.state.lock().unwrap().max_events = Some(n as usize);
            Ok(ud)
        });

        methods.add_async_function("send", |lua, ud: AnyUserData| async move {
            // The borrow is scoped: it must not be held across the await.
            let (client, state) = {
                let this = ud.borrow::<Self>()?;
                (this.client.clone(), this.state.lock().unwrap().clone())
            };

            if let (Some(a), Some(b)) = (state.start_time, state.end_time)
                && a.as_secs_f64() > b.as_secs_f64()
            {
                return Err(mlua::Error::runtime(
                    "lookup_events: ':start_time()' is after ':end_time()'",
                ));
            }

            let mut req = client.lookup_events();
            if let Some(attr) = state.attribute {
                req = req.lookup_attributes(attr);
            }
            if let Some(t) = state.start_time {
                req = req.start_time(t);
            }
            if let Some(t) = state.end_time {
                req = req.end_time(t);
            }
            if state.insights {
                req = req.event_category(aws_sdk_cloudtrail::types::EventCategory::Insight);
            }

            let out = lua.create_table()?;
            let mut idx = 0;
            let mut truncated = false;
            let mut stream = req.into_paginator().send();
            while let Some(page) = stream.next().await {
                let page = page.map_err(|e| lua_err(BoxError::from(e)))?;
                let more = page.next_token.is_some();
                for event in page.events.unwrap_or_default() {
                    if state.max_events.is_some_and(|max| idx >= max) {
                        truncated = true;
                        break;
                    }
                    idx += 1;
                    out.raw_set(idx, event_to_table(&lua, event)?)?;
                }
                if state.max_events.is_some_and(|max| idx >= max) {
                    truncated |= more;
                    break;
                }
                if !more {
                    break;
                }
                // CloudTrail allows 2 lookups per second: pace the walk.
                tokio::time::sleep(PAGE_PAUSE).await;
            }

            // Newest first, as the API returns them. `truncated` says
            // `:max_events()` cut a longer result short.
            let resp = respond(&lua, true, Value::Table(out))?;
            if let Some(Value::Table(env)) = resp.iter().next() {
                env.raw_set("truncated", truncated)?;
            }
            Ok(resp)
        });
    }
}

/// Parse an event's raw `CloudTrailEvent` JSON, best-effort: a string that
/// fails to parse comes back as itself rather than failing the whole lookup,
/// the same trade-off `aws.s3`'s body parsing makes.
/// JSON `null` becomes nil (the field is absent), not the truthy sentinel.
fn parse_raw(lua: &Lua, raw: &str) -> LuaResult<Value> {
    match serde_json::from_str::<serde_json::Value>(raw) {
        Ok(json) => lua.to_value_with(
            &json,
            mlua::serde::ser::Options::new()
                .serialize_none_to_null(false)
                .serialize_unit_to_null(false),
        ),
        Err(_) => Ok(Value::String(lua.create_string(raw)?)),
    }
}

fn event_to_table(lua: &Lua, event: Event) -> LuaResult<Table> {
    let t = lua.create_table()?;

    if let Some(v) = event.event_id {
        t.raw_set("event_id", v)?;
    }
    if let Some(v) = event.event_name {
        t.raw_set("event_name", v)?;
    }
    // The API sends this one as the string "true"/"false"; it comes back as a
    // boolean so it round-trips with `:read_only(<bool>)` and so
    // `if e.read_only then` means what it reads as -- a non-empty string is
    // truthy in Luau, which made "false" true. Nothing else is expected, and
    // anything else is read as false rather than invented into a third state.
    if let Some(v) = event.read_only {
        t.raw_set("read_only", v == "true")?;
    }
    if let Some(v) = event.access_key_id {
        t.raw_set("access_key_id", v)?;
    }
    if let Some(v) = event.event_source {
        t.raw_set("event_source", v)?;
    }
    if let Some(v) = event.username {
        t.raw_set("username", v)?;
    }
    if let Some(v) = event.event_time.as_ref().and_then(date_to_string) {
        t.raw_set("event_time", v)?;
    }
    if let Some(v) = event.cloud_trail_event {
        // Promote the commonly wanted fields out of the camelCase `raw`.
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&v) {
            for (field, key) in [
                ("error_code", "errorCode"),
                ("aws_region", "awsRegion"),
                ("source_ip", "sourceIPAddress"),
            ] {
                if let Some(s) = json.get(key).and_then(|x| x.as_str()) {
                    t.raw_set(field, s)?;
                }
            }
        }
        t.raw_set("raw", parse_raw(lua, &v)?)?;
    }

    // The resources an event names -- an ASG, an instance, a trail -- kept as
    // a plain array; `resource_type` is absent (nil, not empty string) when
    // CloudTrail could not determine it.
    let resources = lua.create_table()?;
    for (idx, r) in event.resources.unwrap_or_default().into_iter().enumerate() {
        let rt = lua.create_table()?;
        if let Some(v) = r.resource_type {
            rt.raw_set("resource_type", v)?;
        }
        if let Some(v) = r.resource_name {
            rt.raw_set("resource_name", v)?;
        }
        resources.raw_set(idx + 1, rt)?;
    }
    t.raw_set("resources", resources)?;

    Ok(t)
}

/// Build the `aws.cloudtrail` module table over `source`.
///
/// Called once for the `aws.cloudtrail` global (ambient configuration) and
/// again per `aws.config():build()`, so the surface cannot differ between
/// them.
pub(crate) fn module(lua: &Lua, source: ClientSource) -> LuaResult<Table> {
    let exports = lua.create_table()?;

    let c = source.clone();
    exports.raw_set(
        "lookup_events",
        lua.create_async_function(move |_, ()| {
            let c = c.clone();
            async move {
                Ok(LookupEvents {
                    client: c.cloudtrail().await,
                    state: Arc::new(Mutex::new(LookupEventsState::default())),
                })
            }
        })?,
    )?;

    Ok(exports)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal config carrying only the behavior version the SDK requires
    /// at construction — enough to build a client for a test that never
    /// calls `:send()`.
    fn test_sdk_config() -> aws_config::SdkConfig {
        aws_config::SdkConfig::builder()
            .behavior_version(aws_config::BehaviorVersion::latest())
            .build()
    }

    #[test]
    fn converts_event_fields_and_resources() {
        let lua = Lua::new();
        let event = Event::builder()
            .event_id("abc-123")
            .event_name("RunInstances")
            .username("alice")
            .event_source("ec2.amazonaws.com")
            .read_only("false")
            .event_time(aws_smithy_types::DateTime::from_secs(1_700_000_000))
            .resources(
                aws_sdk_cloudtrail::types::Resource::builder()
                    .resource_type("AWS::EC2::Instance")
                    .resource_name("i-abc")
                    .build(),
            )
            .cloud_trail_event(r#"{"eventVersion":"1.08"}"#)
            .build();

        let t = event_to_table(&lua, event).unwrap();

        assert_eq!(t.get::<String>("event_id").unwrap(), "abc-123");
        assert_eq!(t.get::<String>("event_name").unwrap(), "RunInstances");
        assert_eq!(t.get::<String>("username").unwrap(), "alice");
        assert_eq!(
            t.get::<String>("event_time").unwrap(),
            "2023-11-14T22:13:20Z"
        );

        let resources = t.get::<Table>("resources").unwrap();
        assert_eq!(resources.raw_len(), 1);
        let r = resources.get::<Table>(1).unwrap();
        assert_eq!(
            r.get::<String>("resource_type").unwrap(),
            "AWS::EC2::Instance"
        );
        assert_eq!(r.get::<String>("resource_name").unwrap(), "i-abc");

        let raw = t.get::<Table>("raw").unwrap();
        assert_eq!(raw.get::<String>("eventVersion").unwrap(), "1.08");
    }

    /// The API's "true"/"false" string arrives as a Luau boolean, so it matches
    /// what `:read_only(<bool>)` takes and `if e.read_only then` is not true
    /// for both values.
    #[test]
    fn read_only_becomes_a_boolean() {
        let lua = Lua::new();

        for (sent, expected) in [("true", true), ("false", false)] {
            let event = Event::builder().read_only(sent).build();
            let t = event_to_table(&lua, event).unwrap();
            assert_eq!(t.get::<bool>("read_only").unwrap(), expected, "{sent}");
        }
    }

    #[test]
    fn promotes_fields_and_drops_json_null() {
        let lua = Lua::new();
        let event = Event::builder()
            .cloud_trail_event(
                r#"{"errorCode":"AccessDenied","awsRegion":"eu-central-1","sourceIPAddress":"1.2.3.4","errorMessage":null}"#,
            )
            .build();
        let t = event_to_table(&lua, event).unwrap();
        assert_eq!(t.get::<String>("error_code").unwrap(), "AccessDenied");
        assert_eq!(t.get::<String>("aws_region").unwrap(), "eu-central-1");
        assert_eq!(t.get::<String>("source_ip").unwrap(), "1.2.3.4");
        let raw = t.get::<Table>("raw").unwrap();
        assert_eq!(raw.get::<Value>("errorMessage").unwrap(), Value::Nil);
    }

    fn builder_lua() -> Lua {
        let lua = Lua::new();
        lua.globals()
            .raw_set(
                "lookup",
                LookupEvents {
                    client: Client::new(&test_sdk_config()),
                    state: Arc::new(Mutex::new(LookupEventsState::default())),
                },
            )
            .unwrap();
        lua
    }

    #[test]
    fn second_attribute_raises() {
        let lua = builder_lua();
        let err = lua
            .load(r#"return lookup:username("a"):event_name("b")"#)
            .eval::<AnyUserData>()
            .unwrap_err()
            .to_string();
        assert!(err.contains("already set"), "{err}");
    }

    #[test]
    fn read_only_requires_a_boolean() {
        let lua = builder_lua();
        let err = lua
            .load(r#"return lookup:read_only("false")"#)
            .eval::<AnyUserData>()
            .unwrap_err()
            .to_string();
        assert!(err.contains("expects a boolean"), "{err}");
    }

    #[test]
    fn filter_rejects_unknown_keys() {
        let lua = builder_lua();
        let err = lua
            .load(r#"return lookup:filter("Nope", "x")"#)
            .eval::<AnyUserData>()
            .unwrap_err()
            .to_string();
        assert!(err.contains("unknown attribute key"), "{err}");
    }

    #[test]
    fn times_accept_numbers_and_rfc3339() {
        assert_eq!(
            time_arg("t", &Value::Integer(10)).unwrap().secs(),
            10
        );
        let lua = Lua::new();
        let s = Value::String(lua.create_string("2023-11-14T22:13:20Z").unwrap());
        assert_eq!(time_arg("t", &s).unwrap().secs(), 1_700_000_000);
        assert!(time_arg("t", &Value::Boolean(true)).is_err());
    }

    /// An event with no resources must still expose an empty table, never nil.
    #[test]
    fn absent_resources_become_empty_table() {
        let lua = Lua::new();
        let event = Event::builder().event_id("no-resources").build();

        let t = event_to_table(&lua, event).unwrap();

        let resources = t.get::<Table>("resources").expect("must be a table");
        assert_eq!(resources.raw_len(), 0);
    }

    /// A raw event that fails to parse is not a lookup failure -- it comes
    /// back as the string itself.
    #[test]
    fn malformed_raw_event_falls_back_to_string() {
        let lua = Lua::new();
        let v = parse_raw(&lua, "not json").unwrap();
        match v {
            Value::String(s) => assert_eq!(s.to_str().unwrap(), "not json"),
            other => panic!("expected a string fallback, got {other:?}"),
        }
    }

    #[test]
    fn rejects_non_positive_max_events() {
        let lua = Lua::new();
        lua.globals()
            .raw_set(
                "lookup",
                LookupEvents {
                    client: Client::new(&test_sdk_config()),
                    state: Arc::new(Mutex::new(LookupEventsState::default())),
                },
            )
            .unwrap();

        for bad in ["0", "-1"] {
            let err = lua
                .load(format!("return lookup:max_events({bad})"))
                .eval::<AnyUserData>()
                .unwrap_err()
                .to_string();
            assert!(err.contains("must be positive"), "{err}");
        }
    }

    /// One attribute is kept, matching CloudTrail's own "at most one
    /// `LookupAttribute`" rule.
    #[test]
    fn a_single_attribute_is_kept() {
        let lua = Lua::new();
        let ud = lua
            .create_userdata(LookupEvents {
                client: Client::new(&test_sdk_config()),
                state: Arc::new(Mutex::new(LookupEventsState::default())),
            })
            .unwrap();
        lua.globals().raw_set("lookup", ud.clone()).unwrap();

        lua.load(r#"lookup:event_name("RunInstances")"#)
            .exec()
            .unwrap();

        let state = ud
            .borrow::<LookupEvents>()
            .unwrap()
            .state
            .lock()
            .unwrap()
            .clone();
        let attr = state.attribute.unwrap();
        assert_eq!(attr.attribute_key, LookupAttributeKey::EventName);
        assert_eq!(attr.attribute_value, "RunInstances");
    }
}
