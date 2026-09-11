//! Luau bindings for AWS EC2, registered as `aws.ec2`.
//!
//! Follows the conventions documented on the parent module: data comes back as
//! plain Luau values, a request is configured through a builder finalized by
//! `:send()`, and errors raise.
//!
//! # The two return values
//!
//! As in [`s3`](super::s3), every `:send()` either raises or returns **two**
//! values: a table describing the call, always carrying `ok`, and the response
//! as native Luau data.
//!
//! ```lua
//! local r, instances = aws.ec2.describe_instances():send()
//! local r, states    = aws.ec2.stop_instances({ "i-abc" }):send()
//! ```
//!
//! For a describe, `ok` is always true — a query that matched nothing is an
//! empty table, not a failure — so the envelope earns its place on the actions
//! instead, where `:dry_run()` makes "allowed, and nothing happened" a real
//! answer rather than an error. The shape is uniform across both so that one
//! `:send()` convention holds for the whole `aws` surface.
//!
//! # Instance status
//!
//! `describe_instance_status` is AWS's own verdict on an instance — the
//! two-minute system/instance status checks plus any scheduled maintenance
//! event — as opposed to `describe_instances`' `state`, which is only EC2's
//! *intended* lifecycle state. A `running` instance can still be failing its
//! system status check; that distinction is the reason this is a separate
//! call rather than a field folded into `describe_instances`. Stopped
//! instances have no checks to report and are excluded by default, matching
//! the API; `:include_all_instances()` asks for them anyway.
//!
//! # Actions
//!
//! The lifecycle actions (`start`/`stop`/`reboot`/`terminate`) are the only
//! part of this module that changes anything, and each takes `:dry_run()`,
//! which maps onto EC2's own `DryRun` parameter: the request is validated for
//! permissions and then refused by the service. EC2 reports that refusal as the
//! `DryRunOperation` error, which is a success here — `ok` is true and nothing
//! ran. `UnauthorizedOperation`, the other half of that pair, still raises.
//!
//! # Tagging
//!
//! `create_tags`/`delete_tags` are one generic pair rather than a set per
//! resource kind, because EC2's tag API is keyed by resource *id*, not type: an
//! instance, an AMI, a snapshot and a volume all go through the same call, and
//! splitting them would invent a distinction the service does not make.
//!
//! ```lua
//! aws.ec2.create_tags("i-abc"):tags({ Env = "prod" }):send()
//! aws.ec2.delete_tags({ "i-abc", "vol-def" }):tags({ "Env" }):send()
//! ```
//!
//! `create_tags` takes a `{[key] = value}` map — the same shape reads hand back
//! as `.tags` — so a read/modify/write round-trip needs no reshaping.
//!
//! `delete_tags` takes **either** shape, mirroring upstream, where omitting a
//! tag's value deletes whatever value it has: a `{[key] = value}` map deletes
//! only on a value match, and an array of keys (`{ "Env", "Name" }`) deletes
//! regardless of value. Upstream also treats an *absent* tag set as "delete
//! every tag", which is too quiet a way to lose a fleet's tags, so `:tags()` is
//! required here; a caller who means all of them passes the keys.
//!
//! Neither call returns data, so `:send()` resolves to `(envelope, nil)`.

use std::result::Result as StdResult;

use aws_sdk_ec2::Client;
use aws_sdk_ec2::types::{Filter, Image, Instance, InstanceStateChange, Snapshot};
use aws_smithy_types::error::metadata::ProvideErrorMetadata;
use mlua::{
    AnyUserData, Lua, MultiValue, Result as LuaResult, Table, UserData, UserDataMethods, Value,
};
use std::sync::{Arc, Mutex};

use super::{
    BoxError, ClientSource, array, date_to_string, format_error_chain, is_not_found, lua_err,
    non_empty, respond, string_list,
};

/// Collect a `Tag` list into a `{[key] = value}` table.
///
/// Always a table, possibly empty, so `.tags.Name` needs no guard.
fn tags_to_table(lua: &Lua, tags: Option<Vec<aws_sdk_ec2::types::Tag>>) -> LuaResult<Table> {
    let t = lua.create_table()?;
    for tag in tags.unwrap_or_default() {
        if let (Some(key), Some(value)) = (tag.key, tag.value) {
            t.raw_set(key, value)?;
        }
    }
    Ok(t)
}

/// Whether an SDK error is EC2 refusing a `DryRun` request because the caller
/// *does* hold the permission.
///
/// The SDK models this as an unhandled service error rather than a variant, so
/// the error code is the only thing to match on. `UnauthorizedOperation` — the
/// same exchange for a caller who lacks the permission — deliberately does not
/// match: that one is a real answer to raise on.
fn is_dry_run_success(err: &impl ProvideErrorMetadata) -> bool {
    err.code() == Some("DryRunOperation")
}

/// The answer to an accepted `:dry_run()`: nothing ran, so the data is an empty
/// table and the envelope says `dry_run = true`.
fn dry_run_ok(lua: &Lua) -> LuaResult<MultiValue> {
    let env = lua.create_table()?;
    env.raw_set("ok", true)?;
    env.raw_set("dry_run", true)?;
    Ok(MultiValue::from_vec(vec![
        Value::Table(env),
        Value::Table(array(&lua, lua.create_table()?)),
    ]))
}

/// Shared request state, mutated in place by the builder methods.
#[derive(Default, Clone)]
struct DescribeInstancesState {
    instance_ids: Vec<String>,
    filters: Vec<(String, Vec<String>)>,
}

/// Builder for `DescribeInstances`, finalized with `:send()`.
struct DescribeInstances {
    client: Client,
    state: Arc<Mutex<DescribeInstancesState>>,
}

impl DescribeInstances {
    fn new(client: Client) -> Self {
        Self {
            client,
            state: Arc::new(Mutex::new(DescribeInstancesState::default())),
        }
    }

    /// Collect every matching instance, following pagination to exhaustion.
    async fn collect(
        client: Client,
        state: DescribeInstancesState,
    ) -> StdResult<Vec<Instance>, BoxError> {
        let mut req = client.describe_instances();

        if !state.instance_ids.is_empty() {
            req = req.set_instance_ids(Some(state.instance_ids));
        }

        if !state.filters.is_empty() {
            req = req.set_filters(Some(build_filters(state.filters)));
        }

        // `items()` flattens pages into reservations, not instances, so the
        // instances still have to be pulled out of each reservation.
        let mut stream = req.into_paginator().items().send();
        let mut instances = Vec::new();
        while let Some(reservation) = stream.next().await {
            let reservation = match reservation {
                Ok(r) => r,
                // An unknown id is "no match", not a failure.
                Err(e) if is_not_found(&e) => return Ok(Vec::new()),
                Err(e) => return Err(e.into()),
            };
            instances.extend(reservation.instances.unwrap_or_default());
        }
        Ok(instances)
    }
}

impl UserData for DescribeInstances {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        // --- typed helpers -------------------------------------------------
        // Each corresponds to a real EC2 filter, so the available methods
        // describe the actual API surface.

        methods.add_function("ids", |_, (ud, ids): (AnyUserData, Value)| {
            let ids = string_list("ec2:ids", ids)?;
            ud.borrow::<Self>()?
                .state
                .lock()
                .unwrap()
                .instance_ids
                .extend(ids);
            Ok(ud)
        });

        methods.add_function("id", |_, (ud, id): (AnyUserData, String)| {
            let id = non_empty("ec2:id", id)?;
            ud.borrow::<Self>()?
                .state
                .lock()
                .unwrap()
                .instance_ids
                .push(id);
            Ok(ud)
        });

        // `tag:<key>` — matches a specific tag's value. Values support the `*`
        // and `?` wildcards, which is why there is no ends-with variant: EC2
        // has no such filter.
        methods.add_function(
            "tag",
            |_, (ud, key, values): (AnyUserData, String, Value)| {
                let values = string_list("ec2:tag", values)?;
                push_filter(&ud, format!("tag:{key}"), values)?;
                Ok(ud)
            },
        );

        // `tag-key` — instances carrying the key, regardless of value.
        methods.add_function("tag_key", |_, (ud, keys): (AnyUserData, Value)| {
            let keys = string_list("ec2:tag_key", keys)?;
            push_filter(&ud, "tag-key".to_string(), keys)?;
            Ok(ud)
        });

        methods.add_function("instance_state", |_, (ud, states): (AnyUserData, Value)| {
            let states = string_list("ec2:instance_state", states)?;
            push_filter(&ud, "instance-state-name".to_string(), states)?;
            Ok(ud)
        });

        methods.add_function("instance_type", |_, (ud, types): (AnyUserData, Value)| {
            let types = string_list("ec2:instance_type", types)?;
            push_filter(&ud, "instance-type".to_string(), types)?;
            Ok(ud)
        });

        methods.add_function("vpc_id", |_, (ud, ids): (AnyUserData, Value)| {
            let ids = string_list("ec2:vpc_id", ids)?;
            push_filter(&ud, "vpc-id".to_string(), ids)?;
            Ok(ud)
        });

        methods.add_function("subnet_id", |_, (ud, ids): (AnyUserData, Value)| {
            let ids = string_list("ec2:subnet_id", ids)?;
            push_filter(&ud, "subnet-id".to_string(), ids)?;
            Ok(ud)
        });

        // `Name` is a tag, not an instance attribute; this is sugar for
        // `:tag("Name", ...)` and inherits the same wildcard support.
        methods.add_function("name", |_, (ud, names): (AnyUserData, Value)| {
            let names = string_list("ec2:name", names)?;
            push_filter(&ud, "tag:Name".to_string(), names)?;
            Ok(ud)
        });

        // --- generic escape hatch -----------------------------------------
        // Any EC2 filter, including ones without a typed helper above.
        methods.add_function(
            "filter",
            |_, (ud, name, values): (AnyUserData, String, Value)| {
                let values = string_list("ec2:filter", values)?;
                push_filter(&ud, name, values)?;
                Ok(ud)
            },
        );

        // --- finalizer ----------------------------------------------------
        // Async, so several `:send()`s under `task.spawn` overlap instead of
        // each pinning the runtime for a full round of pagination.
        methods.add_async_function("send", |lua, ud: AnyUserData| async move {
            // The borrow is scoped: it must not be held across the await, or a
            // task touching this builder meanwhile would hit a borrow error.
            let (client, state) = {
                let this = ud.borrow::<Self>()?;
                (this.client.clone(), this.state.lock().unwrap().clone())
            };

            let instances = DescribeInstances::collect(client, state)
                .await
                .map_err(|e| mlua::Error::runtime(format_error_chain(&e)))?;

            let out = lua.create_table()?;
            for (idx, instance) in instances.into_iter().enumerate() {
                out.raw_set(idx + 1, instance_to_table(&lua, instance)?)?;
            }
            // Always ok: no matches is an empty table, and anything that could
            // have gone wrong has already raised.
            respond(&lua, true, Value::Table(array(&lua, out)))
        });
    }
}

/// Append a filter to the builder's shared state.
fn push_filter(ud: &AnyUserData, name: String, values: Vec<String>) -> LuaResult<()> {
    ud.borrow::<DescribeInstances>()?
        .state
        .lock()
        .unwrap()
        .filters
        .push((name, values));
    Ok(())
}

/// Convert an instance to a plain Luau table with snake_case keys mirroring the
/// SDK. Tags become a `{[key] = value}` map. Absent scalars are omitted rather
/// than set to nil; `tags` is always present, possibly empty.
fn instance_to_table(lua: &Lua, instance: Instance) -> LuaResult<Table> {
    let t = lua.create_table()?;

    if let Some(v) = instance.instance_id {
        t.raw_set("instance_id", v)?;
    }
    if let Some(v) = instance.image_id {
        t.raw_set("image_id", v)?;
    }
    if let Some(v) = instance.instance_type {
        t.raw_set("instance_type", v.as_str().to_string())?;
    }
    if let Some(v) = instance.architecture {
        t.raw_set("architecture", v.as_str().to_string())?;
    }
    if let Some(v) = instance.private_ip_address {
        t.raw_set("private_ip_address", v)?;
    }
    if let Some(v) = instance.public_ip_address {
        t.raw_set("public_ip_address", v)?;
    }
    if let Some(v) = instance.vpc_id {
        t.raw_set("vpc_id", v)?;
    }
    if let Some(v) = instance.subnet_id {
        t.raw_set("subnet_id", v)?;
    }
    if let Some(v) = instance.state_transition_reason {
        t.raw_set("state_transition_reason", v)?;
    }

    // `state` is flattened to its name, the field scripts actually want; the
    // numeric code is kept alongside it.
    if let Some(state) = instance.state {
        if let Some(name) = state.name {
            t.raw_set("state", name.as_str().to_string())?;
        }
        if let Some(code) = state.code {
            t.raw_set("state_code", code)?;
        }
    }

    if let Some(placement) = instance.placement
        && let Some(az) = placement.availability_zone
    {
        t.raw_set("availability_zone", az)?;
    }

    // RFC-3339 keeps the timestamp comparable and printable in Luau, which has
    // no native date type.
    if let Some(v) = instance.launch_time.as_ref().and_then(date_to_string) {
        t.raw_set("launch_time", v)?;
    }

    t.raw_set("tags", tags_to_table(lua, instance.tags)?)?;

    Ok(t)
}

// --- describe_images -------------------------------------------------------

/// Shared request state, mutated in place by the builder methods.
#[derive(Default, Clone)]
struct DescribeImagesState {
    image_ids: Vec<String>,
    owners: Vec<String>,
    filters: Vec<(String, Vec<String>)>,
    include_deprecated: bool,
}

/// Builder for `DescribeImages`, finalized with `:send()`.
struct DescribeImages {
    client: Client,
    state: Arc<Mutex<DescribeImagesState>>,
}

impl UserData for DescribeImages {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_function("id", |_, (ud, id): (AnyUserData, String)| {
            let id = non_empty("ec2:id", id)?;
            ud.borrow::<Self>()?
                .state
                .lock()
                .unwrap()
                .image_ids
                .push(id);
            Ok(ud)
        });

        methods.add_function("ids", |_, (ud, ids): (AnyUserData, Value)| {
            let ids = string_list("ec2:ids", ids)?;
            ud.borrow::<Self>()?
                .state
                .lock()
                .unwrap()
                .image_ids
                .extend(ids);
            Ok(ud)
        });

        // An owner is an account id or one of the aliases EC2 defines —
        // `self`, `amazon`, `aws-marketplace`. Without one, the result is every
        // public AMI in the region, which is never what a script wants.
        methods.add_function("owner", |_, (ud, owners): (AnyUserData, Value)| {
            let owners = string_list("ec2:owner", owners)?;
            ud.borrow::<Self>()?
                .state
                .lock()
                .unwrap()
                .owners
                .extend(owners);
            Ok(ud)
        });

        methods.add_function("name", |_, (ud, names): (AnyUserData, Value)| {
            let names = string_list("ec2:name", names)?;
            push_image_filter(&ud, "name".to_string(), names)?;
            Ok(ud)
        });

        methods.add_function(
            "tag",
            |_, (ud, key, values): (AnyUserData, String, Value)| {
                let values = string_list("ec2:tag", values)?;
                push_image_filter(&ud, format!("tag:{key}"), values)?;
                Ok(ud)
            },
        );

        // `tag-key` — images carrying the key, regardless of value.
        methods.add_function("tag_key", |_, (ud, keys): (AnyUserData, Value)| {
            let keys = string_list("ec2:tag_key", keys)?;
            push_image_filter(&ud, "tag-key".to_string(), keys)?;
            Ok(ud)
        });

        methods.add_function("state", |_, (ud, states): (AnyUserData, Value)| {
            let states = string_list("ec2:state", states)?;
            push_image_filter(&ud, "state".to_string(), states)?;
            Ok(ud)
        });

        methods.add_function("architecture", |_, (ud, arches): (AnyUserData, Value)| {
            let arches = string_list("ec2:architecture", arches)?;
            push_image_filter(&ud, "architecture".to_string(), arches)?;
            Ok(ud)
        });

        // Deprecated AMIs are hidden by default, matching the API.
        methods.add_function("include_deprecated", |_, ud: AnyUserData| {
            ud.borrow::<Self>()?
                .state
                .lock()
                .unwrap()
                .include_deprecated = true;
            Ok(ud)
        });

        methods.add_function(
            "filter",
            |_, (ud, name, values): (AnyUserData, String, Value)| {
                let values = string_list("ec2:filter", values)?;
                push_image_filter(&ud, name, values)?;
                Ok(ud)
            },
        );

        methods.add_async_function("send", |lua, ud: AnyUserData| async move {
            // The borrow is scoped: it must not be held across the await.
            let (client, state) = {
                let this = ud.borrow::<Self>()?;
                (this.client.clone(), this.state.lock().unwrap().clone())
            };

            let mut req = client.describe_images();
            if !state.image_ids.is_empty() {
                req = req.set_image_ids(Some(state.image_ids));
            }
            if !state.owners.is_empty() {
                req = req.set_owners(Some(state.owners));
            }
            if state.include_deprecated {
                req = req.include_deprecated(true);
            }
            if !state.filters.is_empty() {
                req = req.set_filters(Some(build_filters(state.filters)));
            }

            let out = lua.create_table()?;
            let mut idx = 0;
            let mut stream = req.into_paginator().items().send();
            while let Some(image) = stream.next().await {
                let image = match image {
                    Ok(v) => v,
                    // An unknown id is "no match", not a failure.
                    Err(e) if is_not_found(&e) => {
                        return respond(&lua, true, Value::Table(array(&lua, lua.create_table()?)));
                    }
                    Err(e) => return Err(lua_err(BoxError::from(e))),
                };
                idx += 1;
                out.raw_set(idx, image_to_table(&lua, image)?)?;
            }

            respond(&lua, true, Value::Table(array(&lua, out)))
        });
    }
}

/// Append a filter to a [`DescribeImages`] builder's shared state.
fn push_image_filter(ud: &AnyUserData, name: String, values: Vec<String>) -> LuaResult<()> {
    ud.borrow::<DescribeImages>()?
        .state
        .lock()
        .unwrap()
        .filters
        .push((name, values));
    Ok(())
}

fn image_to_table(lua: &Lua, image: Image) -> LuaResult<Table> {
    let t = lua.create_table()?;

    if let Some(v) = image.image_id {
        t.raw_set("image_id", v)?;
    }
    if let Some(v) = image.name {
        t.raw_set("name", v)?;
    }
    if let Some(v) = image.description {
        t.raw_set("description", v)?;
    }
    if let Some(v) = image.owner_id {
        t.raw_set("owner_id", v)?;
    }
    if let Some(v) = image.image_owner_alias {
        t.raw_set("owner_alias", v)?;
    }
    if let Some(v) = image.state {
        t.raw_set("state", v.as_str().to_string())?;
    }
    if let Some(v) = image.architecture {
        t.raw_set("architecture", v.as_str().to_string())?;
    }
    if let Some(v) = image.root_device_type {
        t.raw_set("root_device_type", v.as_str().to_string())?;
    }
    if let Some(v) = image.virtualization_type {
        t.raw_set("virtualization_type", v.as_str().to_string())?;
    }
    if let Some(v) = image.public {
        t.raw_set("public", v)?;
    }
    // Already an RFC-3339 string on the wire for images, unlike every other
    // timestamp in this API, which is a `DateTime`.
    if let Some(v) = image.creation_date {
        t.raw_set("creation_date", v)?;
    }
    if let Some(v) = image.deprecation_time {
        t.raw_set("deprecation_time", v)?;
    }

    t.raw_set("tags", tags_to_table(lua, image.tags)?)?;

    Ok(t)
}

// --- describe_snapshots ----------------------------------------------------

/// Shared request state, mutated in place by the builder methods.
#[derive(Default, Clone)]
struct DescribeSnapshotsState {
    snapshot_ids: Vec<String>,
    owners: Vec<String>,
    filters: Vec<(String, Vec<String>)>,
}

/// Builder for `DescribeSnapshots`, finalized with `:send()`.
struct DescribeSnapshots {
    client: Client,
    state: Arc<Mutex<DescribeSnapshotsState>>,
}

impl UserData for DescribeSnapshots {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_function("id", |_, (ud, id): (AnyUserData, String)| {
            let id = non_empty("ec2:id", id)?;
            ud.borrow::<Self>()?
                .state
                .lock()
                .unwrap()
                .snapshot_ids
                .push(id);
            Ok(ud)
        });

        methods.add_function("ids", |_, (ud, ids): (AnyUserData, Value)| {
            let ids = string_list("ec2:ids", ids)?;
            ud.borrow::<Self>()?
                .state
                .lock()
                .unwrap()
                .snapshot_ids
                .extend(ids);
            Ok(ud)
        });

        // As for images: without an owner the result is every public snapshot
        // in the region.
        methods.add_function("owner", |_, (ud, owners): (AnyUserData, Value)| {
            let owners = string_list("ec2:owner", owners)?;
            ud.borrow::<Self>()?
                .state
                .lock()
                .unwrap()
                .owners
                .extend(owners);
            Ok(ud)
        });

        methods.add_function("volume_id", |_, (ud, ids): (AnyUserData, Value)| {
            let ids = string_list("ec2:volume_id", ids)?;
            push_snapshot_filter(&ud, "volume-id".to_string(), ids)?;
            Ok(ud)
        });

        methods.add_function("status", |_, (ud, states): (AnyUserData, Value)| {
            let states = string_list("ec2:status", states)?;
            push_snapshot_filter(&ud, "status".to_string(), states)?;
            Ok(ud)
        });

        methods.add_function(
            "tag",
            |_, (ud, key, values): (AnyUserData, String, Value)| {
                let values = string_list("ec2:tag", values)?;
                push_snapshot_filter(&ud, format!("tag:{key}"), values)?;
                Ok(ud)
            },
        );

        // `tag-key` — snapshots carrying the key, regardless of value.
        methods.add_function("tag_key", |_, (ud, keys): (AnyUserData, Value)| {
            let keys = string_list("ec2:tag_key", keys)?;
            push_snapshot_filter(&ud, "tag-key".to_string(), keys)?;
            Ok(ud)
        });

        methods.add_function(
            "filter",
            |_, (ud, name, values): (AnyUserData, String, Value)| {
                let values = string_list("ec2:filter", values)?;
                push_snapshot_filter(&ud, name, values)?;
                Ok(ud)
            },
        );

        methods.add_async_function("send", |lua, ud: AnyUserData| async move {
            let (client, state) = {
                let this = ud.borrow::<Self>()?;
                (this.client.clone(), this.state.lock().unwrap().clone())
            };

            let mut req = client.describe_snapshots();
            if !state.snapshot_ids.is_empty() {
                req = req.set_snapshot_ids(Some(state.snapshot_ids));
            }
            if !state.owners.is_empty() {
                req = req.set_owner_ids(Some(state.owners));
            }
            if !state.filters.is_empty() {
                req = req.set_filters(Some(build_filters(state.filters)));
            }

            let out = lua.create_table()?;
            let mut idx = 0;
            let mut stream = req.into_paginator().items().send();
            while let Some(snapshot) = stream.next().await {
                let snapshot = match snapshot {
                    Ok(v) => v,
                    // An unknown id is "no match", not a failure.
                    Err(e) if is_not_found(&e) => {
                        return respond(&lua, true, Value::Table(array(&lua, lua.create_table()?)));
                    }
                    Err(e) => return Err(lua_err(BoxError::from(e))),
                };
                idx += 1;
                out.raw_set(idx, snapshot_to_table(&lua, snapshot)?)?;
            }

            respond(&lua, true, Value::Table(array(&lua, out)))
        });
    }
}

/// Append a filter to a [`DescribeSnapshots`] builder's shared state.
fn push_snapshot_filter(ud: &AnyUserData, name: String, values: Vec<String>) -> LuaResult<()> {
    ud.borrow::<DescribeSnapshots>()?
        .state
        .lock()
        .unwrap()
        .filters
        .push((name, values));
    Ok(())
}

fn snapshot_to_table(lua: &Lua, snapshot: Snapshot) -> LuaResult<Table> {
    let t = lua.create_table()?;

    if let Some(v) = snapshot.snapshot_id {
        t.raw_set("snapshot_id", v)?;
    }
    if let Some(v) = snapshot.volume_id {
        t.raw_set("volume_id", v)?;
    }
    if let Some(v) = snapshot.description {
        t.raw_set("description", v)?;
    }
    if let Some(v) = snapshot.owner_id {
        t.raw_set("owner_id", v)?;
    }
    if let Some(v) = snapshot.state {
        t.raw_set("state", v.as_str().to_string())?;
    }
    if let Some(v) = snapshot.state_message {
        t.raw_set("state_message", v)?;
    }
    if let Some(v) = snapshot.progress {
        t.raw_set("progress", v)?;
    }
    if let Some(v) = snapshot.volume_size {
        t.raw_set("volume_size", v)?;
    }
    if let Some(v) = snapshot.encrypted {
        t.raw_set("encrypted", v)?;
    }
    if let Some(v) = snapshot.storage_tier {
        t.raw_set("storage_tier", v.as_str().to_string())?;
    }
    if let Some(v) = snapshot.start_time.as_ref().and_then(date_to_string) {
        t.raw_set("start_time", v)?;
    }

    t.raw_set("tags", tags_to_table(lua, snapshot.tags)?)?;

    Ok(t)
}

// --- describe_instance_status ------------------------------------------------

/// Shared request state, mutated in place by the builder methods.
#[derive(Default, Clone)]
struct DescribeInstanceStatusState {
    instance_ids: Vec<String>,
    filters: Vec<(String, Vec<String>)>,
    include_all_instances: bool,
}

/// Builder for `DescribeInstanceStatus`, finalized with `:send()`.
///
/// AWS's own verdict on an instance — the two-minute status checks plus any
/// scheduled maintenance event — as opposed to `describe_instances`' `state`,
/// which is EC2's *intended* lifecycle state. A `running` instance can still
/// fail its system status check.
struct DescribeInstanceStatus {
    client: Client,
    state: Arc<Mutex<DescribeInstanceStatusState>>,
}

impl UserData for DescribeInstanceStatus {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_function("id", |_, (ud, id): (AnyUserData, String)| {
            let id = non_empty("ec2:id", id)?;
            ud.borrow::<Self>()?
                .state
                .lock()
                .unwrap()
                .instance_ids
                .push(id);
            Ok(ud)
        });

        methods.add_function("ids", |_, (ud, ids): (AnyUserData, Value)| {
            let ids = string_list("ec2:ids", ids)?;
            ud.borrow::<Self>()?
                .state
                .lock()
                .unwrap()
                .instance_ids
                .extend(ids);
            Ok(ud)
        });

        // Off by default, matching the API: a stopped instance has no status
        // checks to report, so including it would only add empty rows.
        methods.add_function("include_all_instances", |_, ud: AnyUserData| {
            ud.borrow::<Self>()?
                .state
                .lock()
                .unwrap()
                .include_all_instances = true;
            Ok(ud)
        });

        // The same tag helpers `describe_instances` has: the health of the
        // instances carrying a tag is one question, and without these it
        // takes two round trips to ask.
        methods.add_function(
            "tag",
            |_, (ud, key, values): (AnyUserData, String, Value)| {
                let values = string_list("ec2:tag", values)?;
                push_status_filter(&ud, format!("tag:{key}"), values)?;
                Ok(ud)
            },
        );

        methods.add_function("tag_key", |_, (ud, keys): (AnyUserData, Value)| {
            let keys = string_list("ec2:tag_key", keys)?;
            push_status_filter(&ud, "tag-key".to_string(), keys)?;
            Ok(ud)
        });

        // `Name` is a tag, not an instance attribute; sugar for `:tag("Name", …)`.
        methods.add_function("name", |_, (ud, names): (AnyUserData, Value)| {
            let names = string_list("ec2:name", names)?;
            push_status_filter(&ud, "tag:Name".to_string(), names)?;
            Ok(ud)
        });

        methods.add_function(
            "filter",
            |_, (ud, name, values): (AnyUserData, String, Value)| {
                let values = string_list("ec2:filter", values)?;
                push_status_filter(&ud, name, values)?;
                Ok(ud)
            },
        );

        methods.add_async_function("send", |lua, ud: AnyUserData| async move {
            let (client, state) = {
                let this = ud.borrow::<Self>()?;
                (this.client.clone(), this.state.lock().unwrap().clone())
            };

            let mut req = client.describe_instance_status();
            if !state.instance_ids.is_empty() {
                req = req.set_instance_ids(Some(state.instance_ids));
            }
            if state.include_all_instances {
                req = req.include_all_instances(true);
            }
            if !state.filters.is_empty() {
                req = req.set_filters(Some(build_filters(state.filters)));
            }

            let out = lua.create_table()?;
            let mut idx = 0;
            let mut stream = req.into_paginator().items().send();
            while let Some(status) = stream.next().await {
                let status = match status {
                    Ok(v) => v,
                    // An unknown id is "no match", not a failure.
                    Err(e) if is_not_found(&e) => {
                        return respond(&lua, true, Value::Table(array(&lua, lua.create_table()?)));
                    }
                    Err(e) => return Err(lua_err(BoxError::from(e))),
                };
                idx += 1;
                out.raw_set(idx, instance_status_to_table(&lua, status)?)?;
            }

            respond(&lua, true, Value::Table(array(&lua, out)))
        });
    }
}

/// Append a filter to a [`DescribeInstanceStatus`] builder's shared state.
fn push_status_filter(ud: &AnyUserData, name: String, values: Vec<String>) -> LuaResult<()> {
    ud.borrow::<DescribeInstanceStatus>()?
        .state
        .lock()
        .unwrap()
        .filters
        .push((name, values));
    Ok(())
}

fn status_summary_to_table(
    lua: &Lua,
    summary: aws_sdk_ec2::types::InstanceStatusSummary,
) -> LuaResult<Table> {
    let t = lua.create_table()?;
    if let Some(v) = summary.status {
        t.raw_set("status", v.as_str().to_string())?;
    }
    let details = lua.create_table()?;
    for (idx, d) in summary.details.unwrap_or_default().into_iter().enumerate() {
        let dt = lua.create_table()?;
        if let Some(v) = d.name {
            dt.raw_set("name", v.as_str().to_string())?;
        }
        if let Some(v) = d.status {
            dt.raw_set("status", v.as_str().to_string())?;
        }
        if let Some(v) = d.impaired_since.as_ref().and_then(date_to_string) {
            dt.raw_set("impaired_since", v)?;
        }
        details.raw_set(idx + 1, dt)?;
    }
    t.raw_set("details", details)?;
    Ok(t)
}

fn instance_status_to_table(
    lua: &Lua,
    status: aws_sdk_ec2::types::InstanceStatus,
) -> LuaResult<Table> {
    let t = lua.create_table()?;

    if let Some(v) = status.instance_id {
        t.raw_set("instance_id", v)?;
    }
    if let Some(v) = status.availability_zone {
        t.raw_set("availability_zone", v)?;
    }
    if let Some(state) = status.instance_state {
        if let Some(name) = state.name {
            t.raw_set("instance_state", name.as_str().to_string())?;
        }
        if let Some(code) = state.code {
            t.raw_set("instance_state_code", code)?;
        }
    }
    if let Some(v) = status.instance_status {
        t.raw_set("instance_status", status_summary_to_table(lua, v)?)?;
    }
    if let Some(v) = status.system_status {
        t.raw_set("system_status", status_summary_to_table(lua, v)?)?;
    }

    // Scheduled maintenance events (reboot, retirement, ...) that explain a
    // status change without a health-check failure behind it.
    let events = lua.create_table()?;
    for (idx, event) in status.events.unwrap_or_default().into_iter().enumerate() {
        let e = lua.create_table()?;
        if let Some(v) = event.instance_event_id {
            e.raw_set("id", v)?;
        }
        if let Some(v) = event.code {
            e.raw_set("code", v.as_str().to_string())?;
        }
        if let Some(v) = event.description {
            e.raw_set("description", v)?;
        }
        if let Some(v) = event.not_before.as_ref().and_then(date_to_string) {
            e.raw_set("not_before", v)?;
        }
        if let Some(v) = event.not_after.as_ref().and_then(date_to_string) {
            e.raw_set("not_after", v)?;
        }
        if let Some(v) = event.not_before_deadline.as_ref().and_then(date_to_string) {
            e.raw_set("not_before_deadline", v)?;
        }
        events.raw_set(idx + 1, e)?;
    }
    t.raw_set("events", events)?;

    Ok(t)
}

// --- lifecycle actions -----------------------------------------------------

/// Which lifecycle action a [`LifecycleAction`] builder performs.
///
/// One builder over an enum rather than four near-identical types: they differ
/// only in the call `:send()` makes and in whether `:force()` means anything,
/// so four copies would be four places to keep in step.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Lifecycle {
    Start,
    Stop,
    Reboot,
    Terminate,
}

impl Lifecycle {
    /// The name used in error messages, matching the Luau function.
    fn name(self) -> &'static str {
        match self {
            Self::Start => "start_instances",
            Self::Stop => "stop_instances",
            Self::Reboot => "reboot_instances",
            Self::Terminate => "terminate_instances",
        }
    }
}

/// Shared request state, mutated in place by the builder methods.
#[derive(Clone)]
struct LifecycleState {
    instance_ids: Vec<String>,
    dry_run: bool,
    force: bool,
    hibernate: bool,
}

/// Builder for the instance lifecycle actions, finalized with `:send()`.
struct LifecycleAction {
    client: Client,
    action: Lifecycle,
    state: Arc<Mutex<LifecycleState>>,
}

impl UserData for LifecycleAction {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        // Validate the permissions and stop there. EC2 does the refusing, so
        // this cannot be defeated by a bug on our side of the call.
        methods.add_function("dry_run", |_, ud: AnyUserData| {
            ud.borrow::<Self>()?.state.lock().unwrap().dry_run = true;
            Ok(ud)
        });

        // `:force()` and `:hibernate()` are `stop_instances` only; setting one
        // on another action is a script bug, so it raises here rather than
        // being quietly dropped on the way to EC2.
        methods.add_function("force", |_, ud: AnyUserData| {
            {
                let this = ud.borrow::<Self>()?;
                stop_only(this.action, "force")?;
                this.state.lock().unwrap().force = true;
            }
            Ok(ud)
        });

        methods.add_function("hibernate", |_, ud: AnyUserData| {
            {
                let this = ud.borrow::<Self>()?;
                stop_only(this.action, "hibernate")?;
                this.state.lock().unwrap().hibernate = true;
            }
            Ok(ud)
        });

        methods.add_async_function("send", |lua, ud: AnyUserData| async move {
            let (client, action, state) = {
                let this = ud.borrow::<Self>()?;
                (
                    this.client.clone(),
                    this.action,
                    this.state.lock().unwrap().clone(),
                )
            };

            // An empty id list would be a request EC2 rejects with a less
            // helpful message than this one.
            if state.instance_ids.is_empty() {
                return Err(mlua::Error::runtime(format!(
                    "{}: expected at least one instance id",
                    action.name()
                )));
            }

            let ids = state.instance_ids;
            let changes = match action {
                Lifecycle::Start => {
                    let out = client
                        .start_instances()
                        .set_instance_ids(Some(ids))
                        .dry_run(state.dry_run)
                        .send()
                        .await;
                    match out {
                        Ok(out) => out.starting_instances.unwrap_or_default(),
                        Err(e) if is_dry_run_success(&e) => return dry_run_ok(&lua),
                        Err(e) => return Err(lua_err(BoxError::from(e))),
                    }
                }
                Lifecycle::Stop => {
                    let out = client
                        .stop_instances()
                        .set_instance_ids(Some(ids))
                        .dry_run(state.dry_run)
                        .force(state.force)
                        .hibernate(state.hibernate)
                        .send()
                        .await;
                    match out {
                        Ok(out) => out.stopping_instances.unwrap_or_default(),
                        Err(e) if is_dry_run_success(&e) => return dry_run_ok(&lua),
                        Err(e) => return Err(lua_err(BoxError::from(e))),
                    }
                }
                Lifecycle::Terminate => {
                    let out = client
                        .terminate_instances()
                        .set_instance_ids(Some(ids))
                        .dry_run(state.dry_run)
                        .send()
                        .await;
                    match out {
                        Ok(out) => out.terminating_instances.unwrap_or_default(),
                        Err(e) if is_dry_run_success(&e) => return dry_run_ok(&lua),
                        Err(e) => return Err(lua_err(BoxError::from(e))),
                    }
                }
                // `RebootInstances` returns nothing at all: the reboot is a
                // request, not a state transition EC2 reports back.
                Lifecycle::Reboot => {
                    let out = client
                        .reboot_instances()
                        .set_instance_ids(Some(ids))
                        .dry_run(state.dry_run)
                        .send()
                        .await;
                    match out {
                        Ok(_) => Vec::new(),
                        Err(e) if is_dry_run_success(&e) => return dry_run_ok(&lua),
                        Err(e) => return Err(lua_err(BoxError::from(e))),
                    }
                }
            };

            let out = lua.create_table()?;
            for (idx, change) in changes.into_iter().enumerate() {
                out.raw_set(idx + 1, state_change_to_table(&lua, change)?)?;
            }
            respond(&lua, true, Value::Table(array(&lua, out)))
        });
    }
}

/// Reject a `stop_instances`-only knob on any other action.
fn stop_only(action: Lifecycle, method: &str) -> LuaResult<()> {
    if action == Lifecycle::Stop {
        return Ok(());
    }
    Err(mlua::Error::runtime(format!(
        "{}: ':{method}()' applies to stop_instances only",
        action.name()
    )))
}

/// Convert a state change, flattening both states to their names — the field
/// scripts actually want — with the numeric codes kept alongside.
fn state_change_to_table(lua: &Lua, change: InstanceStateChange) -> LuaResult<Table> {
    let t = lua.create_table()?;

    if let Some(v) = change.instance_id {
        t.raw_set("instance_id", v)?;
    }
    if let Some(state) = change.current_state {
        if let Some(name) = state.name {
            t.raw_set("current_state", name.as_str().to_string())?;
        }
        if let Some(code) = state.code {
            t.raw_set("current_state_code", code)?;
        }
    }
    if let Some(state) = change.previous_state {
        if let Some(name) = state.name {
            t.raw_set("previous_state", name.as_str().to_string())?;
        }
        if let Some(code) = state.code {
            t.raw_set("previous_state_code", code)?;
        }
    }

    Ok(t)
}

// --- tagging ---------------------------------------------------------------

/// Which tag action a [`TagAction`] builder performs.
///
/// One builder over an enum, as for [`Lifecycle`]: the two differ only in the
/// call `:send()` makes and in what an empty tag set means, so two copies would
/// be two places to keep in step.
#[derive(Clone, Copy, PartialEq, Eq)]
enum TagAction {
    Create,
    Delete,
}

impl TagAction {
    /// The name used in error messages, matching the Luau function.
    fn name(self) -> &'static str {
        match self {
            Self::Create => "create_tags",
            Self::Delete => "delete_tags",
        }
    }
}

/// A tag as the request carries it: a value of `None` is the key-only form,
/// which `DeleteTags` reads as "any value".
#[derive(Clone, PartialEq, Eq, Debug)]
struct TagSpec {
    key: String,
    value: Option<String>,
}

/// Shared request state, mutated in place by the builder methods.
#[derive(Clone)]
struct TagState {
    resources: Vec<String>,
    /// `None` until `:tags()` is called — the distinction `delete_tags` needs,
    /// where an absent tag set would otherwise mean "every tag".
    tags: Option<Vec<TagSpec>>,
    dry_run: bool,
}

/// Builder for `CreateTags`/`DeleteTags`, finalized with `:send()`.
struct TagRequest {
    client: Client,
    action: TagAction,
    state: Arc<Mutex<TagState>>,
}

/// Read a Luau tag set into [`TagSpec`]s.
///
/// Two shapes are accepted, told apart by the table's *sequence* part: a
/// `{[key] = value}` map yields key+value pairs, while an array of key strings
/// (`{ "Env", "Name" }`) yields key-only specs. The array form exists because
/// `DeleteTags` deletes any value when the value is omitted; for `create_tags`
/// a key-only tag has no meaning, so `allow_key_only` rejects it at the call
/// site rather than letting EC2 answer with a worse message.
///
/// Mixing the two in one table is allowed and means what it reads as: the
/// sequence entries are keys, the remaining string keys are key/value pairs.
fn value_to_tag_specs(value: Value, allow_key_only: bool, action: &str) -> LuaResult<Vec<TagSpec>> {
    let Value::Table(t) = value else {
        return Err(mlua::Error::runtime(format!(
            "{action}: expected a table of tags, got {}",
            value.type_name()
        )));
    };

    let mut out = Vec::new();

    // The sequence part first: these are bare keys.
    let seq_len = t.raw_len();
    for idx in 1..=seq_len {
        // Strict: a number would coerce to a string key (`{123}`) unnoticed.
        let key = match t.raw_get::<Value>(idx)? {
            Value::String(s) if !s.as_bytes().is_empty() => s.to_str()?.to_string(),
            other => {
                return Err(mlua::Error::runtime(format!(
                    "{action}: tag keys must be non-empty strings, got {}",
                    other.type_name()
                )));
            }
        };
        if !allow_key_only {
            return Err(mlua::Error::runtime(format!(
                "{action}: tags must be a {{[key] = value}} map, got a key without a value ({key:?})"
            )));
        }
        out.push(TagSpec { key, value: None });
    }

    // Then everything else: the `{[key] = value}` map, skipping the positive
    // integer keys already consumed above.
    for pair in t.pairs::<Value, Value>() {
        let (k, v) = pair?;
        // Luau may hand a table's integer key back as either representation, so
        // both are checked rather than trusting one.
        let numeric = match &k {
            Value::Integer(i) => Some(*i as f64),
            Value::Number(n) => Some(*n),
            _ => None,
        };
        if let Some(n) = numeric
            && n >= 1.0
            && n <= seq_len as f64
        {
            continue;
        }
        let key = match k {
            Value::String(s) => s.to_str()?.to_string(),
            other => {
                return Err(mlua::Error::runtime(format!(
                    "{action}: tag keys must be strings, got {}",
                    other.type_name()
                )));
            }
        };
        let value = match v {
            Value::String(s) => s.to_str()?.to_string(),
            Value::Integer(i) => i.to_string(),
            Value::Number(n) => n.to_string(),
            Value::Boolean(b) => b.to_string(),
            other => {
                return Err(mlua::Error::runtime(format!(
                    "{action}: tag value for {key:?} must be a string, got {}",
                    other.type_name()
                )));
            }
        };
        out.push(TagSpec {
            key,
            value: Some(value),
        });
    }

    Ok(out)
}

/// Build the SDK tag list a tag request sends.
fn build_tags(tags: Vec<TagSpec>) -> Vec<aws_sdk_ec2::types::Tag> {
    tags.into_iter()
        .map(|spec| {
            let b = aws_sdk_ec2::types::Tag::builder().key(spec.key);
            // Key-only: `DeleteTags` reads an omitted value as "any value".
            match spec.value {
                Some(v) => b.value(v).build(),
                None => b.build(),
            }
        })
        .collect()
}

impl UserData for TagRequest {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        // The tag set. For `create_tags` a `{[key] = value}` map only; for
        // `delete_tags` either that or an array of key strings.
        methods.add_function("tags", |_, (ud, tags): (AnyUserData, Value)| {
            {
                let this = ud.borrow::<Self>()?;
                let specs =
                    value_to_tag_specs(tags, this.action == TagAction::Delete, this.action.name())?;
                this.state.lock().unwrap().tags = Some(specs);
            }
            Ok(ud)
        });

        // Validate the permissions and stop there, exactly as the lifecycle
        // actions do: EC2 does the refusing.
        methods.add_function("dry_run", |_, ud: AnyUserData| {
            ud.borrow::<Self>()?.state.lock().unwrap().dry_run = true;
            Ok(ud)
        });

        methods.add_async_function("send", |lua, ud: AnyUserData| async move {
            let (client, action, state) = {
                let this = ud.borrow::<Self>()?;
                (
                    this.client.clone(),
                    this.action,
                    this.state.lock().unwrap().clone(),
                )
            };

            let tags = check_tag_request(action, &state)?;
            let resources = state.resources;
            let tags = build_tags(tags);

            match action {
                TagAction::Create => {
                    let out = client
                        .create_tags()
                        .set_resources(Some(resources))
                        .set_tags(Some(tags))
                        .dry_run(state.dry_run)
                        .send()
                        .await;
                    match out {
                        Ok(_) => {}
                        Err(e) if is_dry_run_success(&e) => return dry_run_ok(&lua),
                        Err(e) => return Err(lua_err(BoxError::from(e))),
                    }
                }
                TagAction::Delete => {
                    let out = client
                        .delete_tags()
                        .set_resources(Some(resources))
                        .set_tags(Some(tags))
                        .dry_run(state.dry_run)
                        .send()
                        .await;
                    match out {
                        Ok(_) => {}
                        Err(e) if is_dry_run_success(&e) => return dry_run_ok(&lua),
                        Err(e) => return Err(lua_err(BoxError::from(e))),
                    }
                }
            }

            // Neither call returns data; the envelope is the whole answer.
            respond(&lua, true, Value::Nil)
        });
    }
}

/// Reject a tag request EC2 would answer with a worse message, or — for a
/// delete with no `:tags()` — would answer far too willingly.
fn check_tag_request(action: TagAction, state: &TagState) -> LuaResult<Vec<TagSpec>> {
    if state.resources.is_empty() {
        return Err(mlua::Error::runtime(format!(
            "{}: expected at least one resource id",
            action.name()
        )));
    }

    // Upstream reads an absent tag set as "delete every tag on these
    // resources". That is a silent fleet-wide loss one forgotten call away, so
    // it has to be asked for by naming the keys.
    let Some(tags) = state.tags.as_ref() else {
        return Err(mlua::Error::runtime(match action {
            TagAction::Delete => format!(
                "{}: expected ':tags()' — pass the keys to delete; there is no \
                 shorthand for deleting every tag",
                action.name()
            ),
            TagAction::Create => format!("{}: expected at least one tag", action.name()),
        }));
    };

    // An empty set is a mistake on both: `create_tags` has nothing to write,
    // and EC2 reads an empty `delete_tags` list as "delete every tag" (the SDK
    // even omits the empty list from the request).
    if tags.is_empty() {
        return Err(mlua::Error::runtime(format!(
            "{}: expected at least one tag",
            action.name()
        )));
    }

    Ok(tags.clone())
}

/// Build the SDK filter list shared by every describe in this module.
fn build_filters(filters: Vec<(String, Vec<String>)>) -> Vec<Filter> {
    filters
        .into_iter()
        .map(|(name, values)| {
            Filter::builder()
                .name(name)
                .set_values(Some(values))
                .build()
        })
        .collect()
}

/// Build the `aws.ec2` module table over `source`.
///
/// Called once for the `aws.ec2` global (ambient configuration) and again per
/// `aws.config():build()`, so the surface cannot differ between them.
pub(crate) fn module(lua: &Lua, source: ClientSource) -> LuaResult<Table> {
    let exports = lua.create_table()?;

    let c = source.clone();
    exports.raw_set(
        "describe_instances",
        lua.create_async_function(move |_, ()| {
            let c = c.clone();
            async move { Ok(DescribeInstances::new(c.ec2().await)) }
        })?,
    )?;

    let c = source.clone();
    exports.raw_set(
        "describe_images",
        lua.create_async_function(move |_, ()| {
            let c = c.clone();
            async move {
                Ok(DescribeImages {
                    client: c.ec2().await,
                    state: Arc::new(Mutex::new(DescribeImagesState::default())),
                })
            }
        })?,
    )?;

    let c = source.clone();
    exports.raw_set(
        "describe_snapshots",
        lua.create_async_function(move |_, ()| {
            let c = c.clone();
            async move {
                Ok(DescribeSnapshots {
                    client: c.ec2().await,
                    state: Arc::new(Mutex::new(DescribeSnapshotsState::default())),
                })
            }
        })?,
    )?;

    let c = source.clone();
    exports.raw_set(
        "describe_instance_status",
        lua.create_async_function(move |_, ()| {
            let c = c.clone();
            async move {
                Ok(DescribeInstanceStatus {
                    client: c.ec2().await,
                    state: Arc::new(Mutex::new(DescribeInstanceStatusState::default())),
                })
            }
        })?,
    )?;

    // The four lifecycle actions differ only in the enum they carry, so they
    // are registered from one list rather than four near-identical closures.
    for (name, action) in [
        ("start_instances", Lifecycle::Start),
        ("stop_instances", Lifecycle::Stop),
        ("reboot_instances", Lifecycle::Reboot),
        ("terminate_instances", Lifecycle::Terminate),
    ] {
        let c = source.clone();
        exports.raw_set(
            name,
            // The ids are required, so they are an argument rather than a
            // builder method — there is no useful call without them.
            lua.create_async_function(move |_, ids: Value| {
                let c = c.clone();
                async move {
                    let instance_ids = string_list(name, ids)?;
                    Ok(LifecycleAction {
                        client: c.ec2().await,
                        action,
                        state: Arc::new(Mutex::new(LifecycleState {
                            instance_ids,
                            dry_run: false,
                            force: false,
                            hibernate: false,
                        })),
                    })
                }
            })?,
        )?;
    }

    // Both tag actions differ only in the enum they carry. One generic pair
    // covers instances, AMIs, snapshots, volumes and the rest, because EC2's
    // tag API is keyed by resource id, not by resource type.
    for (name, action) in [
        ("create_tags", TagAction::Create),
        ("delete_tags", TagAction::Delete),
    ] {
        let c = source.clone();
        exports.raw_set(
            name,
            // The ids are required, so they are an argument rather than a
            // builder method — there is no useful call without them.
            lua.create_async_function(move |_, resources: Value| {
                let c = c.clone();
                async move {
                    let resources = string_list(name, resources)?;
                    Ok(TagRequest {
                        client: c.ec2().await,
                        action,
                        state: Arc::new(Mutex::new(TagState {
                            resources,
                            tags: None,
                            dry_run: false,
                        })),
                    })
                }
            })?,
        )?;
    }

    Ok(exports)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_sdk_ec2::types::{
        ArchitectureValues, ImageState, InstanceState, InstanceStateName, InstanceType,
        SnapshotState, Tag,
    };

    #[test]
    fn converts_instance_fields_and_flattens_state() {
        let lua = Lua::new();
        let instance = Instance::builder()
            .instance_id("i-abc123")
            .instance_type(InstanceType::T3Micro)
            .state(
                InstanceState::builder()
                    .name(InstanceStateName::Running)
                    .code(16)
                    .build(),
            )
            .tags(Tag::builder().key("Name").value("web-1").build())
            .build();

        let t = instance_to_table(&lua, instance).unwrap();

        assert_eq!(t.get::<String>("instance_id").unwrap(), "i-abc123");
        assert_eq!(t.get::<String>("instance_type").unwrap(), "t3.micro");
        assert_eq!(t.get::<String>("state").unwrap(), "running");
        assert_eq!(t.get::<i32>("state_code").unwrap(), 16);
        assert_eq!(
            t.get::<Table>("tags")
                .unwrap()
                .get::<String>("Name")
                .unwrap(),
            "web-1"
        );
    }

    /// An instance with no tags must still expose an empty `tags` table, never nil.
    #[test]
    fn absent_tags_become_empty_table_not_nil() {
        let lua = Lua::new();
        let instance = Instance::builder().instance_id("i-notags").build();

        let t = instance_to_table(&lua, instance).unwrap();

        let tags = t.get::<Table>("tags").expect("tags must be a table");
        assert_eq!(tags.raw_len(), 0);
        assert!(t.get::<Value>("vpc_id").unwrap().is_nil());
    }

    #[test]
    fn accepts_single_string_or_list() {
        let lua = Lua::new();
        let single = Value::String(lua.create_string("web").unwrap());
        assert_eq!(string_list("t", single).unwrap(), vec!["web"]);

        let list = lua.create_table().unwrap();
        list.raw_set(1, "web").unwrap();
        list.raw_set(2, "api").unwrap();
        assert_eq!(
            string_list("t", Value::Table(list)).unwrap(),
            vec!["web", "api"]
        );
    }

    #[test]
    fn rejects_non_string_values() {
        let err = string_list("t", Value::Integer(42)).unwrap_err();
        assert!(
            err.to_string()
                .contains("expected string or array of strings")
        );
    }

    #[test]
    fn converts_image_fields() {
        let lua = Lua::new();
        let image = Image::builder()
            .image_id("ami-abc")
            .name("base-2024")
            .state(ImageState::Available)
            .architecture(ArchitectureValues::X8664)
            .creation_date("2024-01-02T03:04:05.000Z")
            .public(false)
            .tags(Tag::builder().key("Role").value("base").build())
            .build();

        let t = image_to_table(&lua, image).unwrap();

        assert_eq!(t.get::<String>("image_id").unwrap(), "ami-abc");
        assert_eq!(t.get::<String>("name").unwrap(), "base-2024");
        assert_eq!(t.get::<String>("state").unwrap(), "available");
        assert_eq!(t.get::<String>("architecture").unwrap(), "x86_64");
        assert!(!t.get::<bool>("public").unwrap());
        assert_eq!(
            t.get::<Table>("tags")
                .unwrap()
                .get::<String>("Role")
                .unwrap(),
            "base"
        );
    }

    #[test]
    fn converts_snapshot_fields() {
        let lua = Lua::new();
        let snapshot = Snapshot::builder()
            .snapshot_id("snap-abc")
            .volume_id("vol-abc")
            .state(SnapshotState::Completed)
            .volume_size(8)
            .encrypted(true)
            .start_time(aws_smithy_types::DateTime::from_secs(1_700_000_000))
            .build();

        let t = snapshot_to_table(&lua, snapshot).unwrap();

        assert_eq!(t.get::<String>("snapshot_id").unwrap(), "snap-abc");
        assert_eq!(t.get::<String>("volume_id").unwrap(), "vol-abc");
        assert_eq!(t.get::<String>("state").unwrap(), "completed");
        assert_eq!(t.get::<i32>("volume_size").unwrap(), 8);
        assert!(t.get::<bool>("encrypted").unwrap());
        assert_eq!(
            t.get::<String>("start_time").unwrap(),
            "2023-11-14T22:13:20Z"
        );
        // Always a table, so `.tags.x` needs no guard even with none set.
        assert_eq!(t.get::<Table>("tags").unwrap().raw_len(), 0);
    }

    /// Both states are flattened to their names, with the codes kept alongside.
    #[test]
    fn converts_state_change_both_directions() {
        let lua = Lua::new();
        let change = InstanceStateChange::builder()
            .instance_id("i-abc")
            .previous_state(
                InstanceState::builder()
                    .name(InstanceStateName::Running)
                    .code(16)
                    .build(),
            )
            .current_state(
                InstanceState::builder()
                    .name(InstanceStateName::ShuttingDown)
                    .code(32)
                    .build(),
            )
            .build();

        let t = state_change_to_table(&lua, change).unwrap();

        assert_eq!(t.get::<String>("instance_id").unwrap(), "i-abc");
        assert_eq!(t.get::<String>("previous_state").unwrap(), "running");
        assert_eq!(t.get::<i32>("previous_state_code").unwrap(), 16);
        assert_eq!(t.get::<String>("current_state").unwrap(), "shutting-down");
        assert_eq!(t.get::<i32>("current_state_code").unwrap(), 32);
    }

    /// `:force()`/`:hibernate()` mean nothing outside a stop, so they raise
    /// rather than being dropped on the way to EC2.
    #[test]
    fn force_is_rejected_off_stop() {
        assert!(stop_only(Lifecycle::Stop, "force").is_ok());

        let err = stop_only(Lifecycle::Terminate, "force").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("terminate_instances"), "{msg}");
        assert!(msg.contains("stop_instances only"), "{msg}");
    }

    /// A `DryRunOperation` refusal is the success case; `UnauthorizedOperation`
    /// — the same exchange for a caller lacking the permission — is not.
    #[test]
    fn dry_run_refusal_is_a_success_but_unauthorized_is_not() {
        use aws_smithy_types::error::ErrorMetadata;

        let dry_run = ErrorMetadata::builder().code("DryRunOperation").build();
        assert!(is_dry_run_success(&dry_run));

        let unauthorized = ErrorMetadata::builder()
            .code("UnauthorizedOperation")
            .build();
        assert!(!is_dry_run_success(&unauthorized));
    }

    #[test]
    fn builds_filters_with_several_values() {
        let filters = build_filters(vec![(
            "instance-state-name".to_string(),
            vec!["running".to_string(), "stopped".to_string()],
        )]);

        assert_eq!(filters.len(), 1);
        assert_eq!(filters[0].name(), Some("instance-state-name"));
        assert_eq!(filters[0].values(), ["running", "stopped"]);
    }

    /// The status builder's tag helpers map onto the same EC2 filter names
    /// `describe_instances` uses, so a tagged set's health is one call.
    #[test]
    fn instance_status_tag_helpers_build_ec2_filters() {
        let lua = Lua::new();
        let ud = lua
            .create_userdata(DescribeInstanceStatus {
                client: Client::new(
                    &aws_config::SdkConfig::builder()
                        .behavior_version(aws_config::BehaviorVersion::latest())
                        .build(),
                ),
                state: Arc::new(Mutex::new(DescribeInstanceStatusState::default())),
            })
            .unwrap();
        lua.globals().raw_set("status", ud.clone()).unwrap();

        lua.load(r#"status:name("web"):tag("Env", "prod"):tag_key("Owner")"#)
            .exec()
            .unwrap();

        let filters = ud
            .borrow::<DescribeInstanceStatus>()
            .unwrap()
            .state
            .lock()
            .unwrap()
            .filters
            .clone();

        assert_eq!(
            filters,
            vec![
                ("tag:Name".to_string(), vec!["web".to_string()]),
                ("tag:Env".to_string(), vec!["prod".to_string()]),
                ("tag-key".to_string(), vec!["Owner".to_string()]),
            ]
        );
    }

    // --- tagging -----------------------------------------------------------

    /// A `{[key] = value}` map is the create shape and yields key+value tags.
    #[test]
    fn map_tags_carry_key_and_value() {
        let lua = Lua::new();
        let t = lua.create_table().unwrap();
        t.raw_set("Env", "prod").unwrap();

        let specs = value_to_tag_specs(Value::Table(t), false, "create_tags").unwrap();

        assert_eq!(
            specs,
            vec![TagSpec {
                key: "Env".to_string(),
                value: Some("prod".to_string()),
            }]
        );

        let tags = build_tags(specs);
        assert_eq!(tags[0].key(), Some("Env"));
        assert_eq!(tags[0].value(), Some("prod"));
    }

    /// An array of key strings is the delete shape and yields key-only tags,
    /// which is how EC2 spells "delete regardless of value".
    #[test]
    fn array_tags_are_key_only() {
        let lua = Lua::new();
        let t = lua.create_table().unwrap();
        t.raw_set(1, "Env").unwrap();
        t.raw_set(2, "Name").unwrap();

        let specs = value_to_tag_specs(Value::Table(t), true, "delete_tags").unwrap();

        assert_eq!(
            specs,
            vec![
                TagSpec {
                    key: "Env".to_string(),
                    value: None
                },
                TagSpec {
                    key: "Name".to_string(),
                    value: None
                },
            ]
        );

        let tags = build_tags(specs);
        assert_eq!(tags[0].key(), Some("Env"));
        assert_eq!(tags[0].value(), None);
        assert_eq!(tags[1].key(), Some("Name"));
        assert_eq!(tags[1].value(), None);
    }

    /// Mixing the shapes in one delete table means what it reads as.
    #[test]
    fn mixed_tags_keep_both_forms() {
        let lua = Lua::new();
        let t = lua.create_table().unwrap();
        t.raw_set(1, "Name").unwrap();
        t.raw_set("Env", "prod").unwrap();

        let mut specs = value_to_tag_specs(Value::Table(t), true, "delete_tags").unwrap();
        specs.sort_by(|a, b| a.key.cmp(&b.key));

        assert_eq!(
            specs,
            vec![
                TagSpec {
                    key: "Env".to_string(),
                    value: Some("prod".to_string()),
                },
                TagSpec {
                    key: "Name".to_string(),
                    value: None,
                },
            ]
        );
    }

    /// A key without a value has no meaning on create, so it is refused at the
    /// call site instead of by EC2.
    #[test]
    fn create_rejects_key_only_tags() {
        let lua = Lua::new();
        let t = lua.create_table().unwrap();
        t.raw_set(1, "Env").unwrap();

        let err = value_to_tag_specs(Value::Table(t), false, "create_tags").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("create_tags"), "{msg}");
        assert!(msg.contains("[key] = value"), "{msg}");
    }

    #[test]
    fn rejects_non_table_tag_set() {
        let err = value_to_tag_specs(Value::Integer(1), true, "delete_tags").unwrap_err();
        assert!(err.to_string().contains("expected a table of tags"));
    }

    /// No resource ids is a request EC2 rejects with a worse message.
    #[test]
    fn tag_request_needs_a_resource_id() {
        let state = TagState {
            resources: Vec::new(),
            tags: Some(vec![TagSpec {
                key: "Env".to_string(),
                value: Some("prod".to_string()),
            }]),
            dry_run: false,
        };

        let err = check_tag_request(TagAction::Create, &state).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("create_tags"), "{msg}");
        assert!(msg.contains("at least one resource id"), "{msg}");
    }

    /// Create needs tags; delete needs `:tags()` to have been called at all,
    /// because upstream reads its absence as "delete every tag".
    #[test]
    fn absent_tag_set_is_rejected_for_both_reasons() {
        let state = TagState {
            resources: vec!["i-abc".to_string()],
            tags: None,
            dry_run: false,
        };

        let create = check_tag_request(TagAction::Create, &state)
            .unwrap_err()
            .to_string();
        assert!(create.contains("create_tags"), "{create}");
        assert!(create.contains("at least one tag"), "{create}");

        let delete = check_tag_request(TagAction::Delete, &state)
            .unwrap_err()
            .to_string();
        assert!(delete.contains("delete_tags"), "{delete}");
        assert!(delete.contains("':tags()'"), "{delete}");
    }

    /// An empty set is an error on both: on delete EC2 would read it as "every
    /// tag" (P0-1).
    #[test]
    fn empty_tag_set_is_rejected_on_both() {
        let state = TagState {
            resources: vec!["i-abc".to_string()],
            tags: Some(Vec::new()),
            dry_run: false,
        };

        assert!(check_tag_request(TagAction::Create, &state).is_err());
        assert!(check_tag_request(TagAction::Delete, &state).is_err());
    }

    #[test]
    fn empty_ids_and_numeric_tag_keys_are_rejected() {
        let lua = Lua::new();
        let t = lua.create_table().unwrap();
        assert!(string_list("ec2:ids", Value::Table(t)).is_err());
        let t = lua.create_table().unwrap();
        t.raw_set(1, 123).unwrap();
        assert!(value_to_tag_specs(Value::Table(t), true, "delete_tags").is_err());
    }

    #[test]
    fn dry_run_answer_is_flagged_with_empty_data() {
        let lua = Lua::new();
        let mv = dry_run_ok(&lua).unwrap().into_vec();
        let Value::Table(env) = &mv[0] else { panic!() };
        assert!(env.raw_get::<bool>("dry_run").unwrap());
        assert!(matches!(mv[1], Value::Table(_)));
    }
}
