//! Luau bindings for AWS EC2 Auto Scaling, registered as `aws.autoscaling`.
//!
//! Its own key rather than a nesting under `ec2`: despite the "EC2 Auto
//! Scaling" branding this is a separate API, with its own endpoint, SDK crate
//! and IAM prefix (`autoscaling:*`).
//!
//! Follows the conventions documented on the parent module — a builder
//! finalized by `:send()`, returning `(envelope, data)` — and the vocabulary of
//! the API itself: the unit is a *group*, and instances are its members.
//!
//! # Scaling
//!
//! `set_desired_capacity` moves a group to a size within its existing bounds;
//! `update_group` is the one that changes the bounds themselves. They are kept
//! apart because the first is the routine act and the second reshapes the
//! group — asking for 20 instances in a group whose `max_size` is 10 should
//! say so rather than silently raise the ceiling.
//!
//! # Instance refresh
//!
//! A refresh replaces a group's instances in batches. It is asynchronous: the
//! start call returns an id, and progress arrives through
//! `describe_instance_refreshes`. `:wait()` on the start builder polls that for
//! you until the refresh reaches a terminal state — it yields rather than
//! blocking, so several groups can refresh concurrently under `task`.
//!
//! # Tagging
//!
//! A group's tags are **not** the EC2 tags on the instances it launches: they
//! live in this API, keyed by group name, and `aws.ec2.describe_instances` will
//! never show them. `propagate_at_launch` is the only bridge between the two
//! namespaces — it copies a group tag onto each instance the group launches
//! *from then on*, leaving instances already running untouched.
//!
//! ```lua
//! aws.autoscaling.create_or_update_tags("web-prod")
//!     :tags({ Env = "prod" })
//!     :propagate_at_launch()
//!     :send()
//! aws.autoscaling.delete_tags("web-prod"):tags({ "Env" }):send()
//! ```
//!
//! Upstream models each tag as a `{ResourceId, ResourceType, Key, Value,
//! PropagateAtLaunch}` record and takes a list of them, so one call could in
//! principle span groups. Here the group is the entry-function argument, as it
//! is for every other call in this module: it keeps `ResourceId` from being
//! repeated on every tag, and `ResourceType` is always `auto-scaling-group`, so
//! we set it rather than asking.
//!
//! `create_or_update_tags` takes a `{[key] = value}` map — the shape a read
//! hands back as `.tags` — so a read/modify/write round-trip needs no
//! reshaping. `delete_tags` takes **either** that map or an array of keys
//! (`{ "Env", "Name" }`), keys alone being what a delete needs; `:tags()` is
//! required rather than defaulting to anything, since both "nothing" and
//! "everything" are bad guesses at what an absent tag set meant.
//!
//! `:propagate_at_launch([bool])` is builder-wide, defaulting to true when
//! called bare and false when never called. Upstream puts the flag on each tag,
//! but a set of tags that disagree about propagation is not the case that
//! occurs, and spelling it per tag would need a nested table that fights the
//! `{[key] = value}` map. Split the call if you genuinely need both.
//!
//! Neither call returns data, so `:send()` resolves to `(envelope, nil)`.

use std::time::Duration;

use aws_sdk_autoscaling::Client;
use aws_sdk_autoscaling::types::{
    AutoScalingGroup, AutoScalingInstanceDetails, Instance, InstanceRefresh, InstanceRefreshStatus,
    RefreshPreferences, RefreshStrategy,
};
use mlua::{
    AnyUserData, Lua, MultiValue, Result as LuaResult, Table, UserData, UserDataMethods, Value,
};
use std::sync::{Arc, Mutex};

use super::{BoxError, ClientSource, lua_err, value_to_string_list};

/// Format a timestamp as RFC 3339, matching every other date in this crate.
fn date_to_string(dt: &aws_smithy_types::DateTime) -> Option<String> {
    dt.fmt(aws_smithy_types::date_time::Format::DateTime).ok()
}

/// Return `(envelope, data)`, the pair every `:send()` resolves to.
fn respond(lua: &Lua, ok: bool, data: Value) -> LuaResult<MultiValue> {
    let env = lua.create_table()?;
    env.raw_set("ok", ok)?;
    Ok(MultiValue::from_vec(vec![Value::Table(env), data]))
}

/// Whether a refresh status is one the refresh will not move on from.
///
/// Everything else (`Pending`, `InProgress`, `Baking`, `Cancelling`,
/// `RollbackInProgress`) is still in flight, so `:wait()` keeps polling. A
/// status the SDK does not know is treated as terminal: waiting forever on a
/// value we cannot interpret is the worse failure.
fn is_terminal(status: &InstanceRefreshStatus) -> bool {
    matches!(
        status,
        InstanceRefreshStatus::Successful
            | InstanceRefreshStatus::Failed
            | InstanceRefreshStatus::Cancelled
            | InstanceRefreshStatus::RollbackSuccessful
            | InstanceRefreshStatus::RollbackFailed
    ) || !InstanceRefreshStatus::values().contains(&status.as_str())
}

// --- describe_groups -------------------------------------------------------

/// Shared request state, mutated in place by the builder methods.
#[derive(Default, Clone)]
struct DescribeGroupsState {
    names: Vec<String>,
    filters: Vec<(String, Vec<String>)>,
}

/// Builder for `DescribeAutoScalingGroups`, finalized with `:send()`.
struct DescribeGroups {
    client: Client,
    state: Arc<Mutex<DescribeGroupsState>>,
}

impl UserData for DescribeGroups {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_function("name", |_, (ud, names): (AnyUserData, Value)| {
            let names = value_to_string_list(names)?;
            ud.borrow::<Self>()?
                .state
                .lock()
                .unwrap()
                .names
                .extend(names);
            Ok(ud)
        });

        // `tag:<key>` and `tag-key`, the same spelling `aws.ec2` uses.
        methods.add_function(
            "tag",
            |_, (ud, key, values): (AnyUserData, String, Value)| {
                let values = value_to_string_list(values)?;
                push_filter(&ud, format!("tag:{key}"), values)?;
                Ok(ud)
            },
        );

        methods.add_function("tag_key", |_, (ud, keys): (AnyUserData, Value)| {
            let keys = value_to_string_list(keys)?;
            push_filter(&ud, "tag-key".to_string(), keys)?;
            Ok(ud)
        });

        methods.add_function(
            "filter",
            |_, (ud, name, values): (AnyUserData, String, Value)| {
                let values = value_to_string_list(values)?;
                push_filter(&ud, name, values)?;
                Ok(ud)
            },
        );

        methods.add_async_function("send", |lua, ud: AnyUserData| async move {
            // The borrow is scoped: it must not be held across the await.
            let (client, state) = {
                let this = ud.borrow::<Self>()?;
                (this.client.clone(), this.state.lock().unwrap().clone())
            };

            let mut req = client.describe_auto_scaling_groups();
            if !state.names.is_empty() {
                req = req.set_auto_scaling_group_names(Some(state.names));
            }
            for (name, values) in state.filters {
                req = req.filters(
                    aws_sdk_autoscaling::types::Filter::builder()
                        .name(name)
                        .set_values(Some(values))
                        .build(),
                );
            }

            let out = lua.create_table()?;
            let mut idx = 0;
            let mut stream = req.into_paginator().items().send();
            while let Some(group) = stream.next().await {
                let group = group.map_err(|e| lua_err(BoxError::from(e)))?;
                idx += 1;
                out.raw_set(idx, group_to_table(&lua, group)?)?;
            }

            respond(&lua, true, Value::Table(out))
        });
    }
}

/// Append a filter to a [`DescribeGroups`] builder's shared state.
fn push_filter(ud: &AnyUserData, name: String, values: Vec<String>) -> LuaResult<()> {
    ud.borrow::<DescribeGroups>()?
        .state
        .lock()
        .unwrap()
        .filters
        .push((name, values));
    Ok(())
}

fn group_to_table(lua: &Lua, group: AutoScalingGroup) -> LuaResult<Table> {
    let t = lua.create_table()?;

    if let Some(v) = group.auto_scaling_group_name {
        t.raw_set("name", v)?;
    }
    if let Some(v) = group.auto_scaling_group_arn {
        t.raw_set("arn", v)?;
    }
    if let Some(v) = group.min_size {
        t.raw_set("min_size", v)?;
    }
    if let Some(v) = group.max_size {
        t.raw_set("max_size", v)?;
    }
    if let Some(v) = group.desired_capacity {
        t.raw_set("desired_capacity", v)?;
    }
    if let Some(v) = group.health_check_type {
        t.raw_set("health_check_type", v)?;
    }
    if let Some(v) = group.health_check_grace_period {
        t.raw_set("health_check_grace_period", v)?;
    }
    if let Some(v) = group.default_cooldown {
        t.raw_set("default_cooldown", v)?;
    }
    if let Some(v) = group.launch_configuration_name {
        t.raw_set("launch_configuration_name", v)?;
    }
    if let Some(v) = group.vpc_zone_identifier {
        t.raw_set("vpc_zone_identifier", v)?;
    }
    if let Some(v) = group.status {
        t.raw_set("status", v)?;
    }
    if let Some(v) = group.new_instances_protected_from_scale_in {
        t.raw_set("new_instances_protected_from_scale_in", v)?;
    }
    if let Some(v) = group.created_time.as_ref().and_then(date_to_string) {
        t.raw_set("created_time", v)?;
    }

    // The launch template, flattened to the two fields that identify it.
    if let Some(lt) = group.launch_template {
        let t2 = lua.create_table()?;
        if let Some(v) = lt.launch_template_name {
            t2.raw_set("name", v)?;
        }
        if let Some(v) = lt.version {
            t2.raw_set("version", v)?;
        }
        t.raw_set("launch_template", t2)?;
    }

    let azs = lua.create_table()?;
    for (idx, az) in group
        .availability_zones
        .unwrap_or_default()
        .into_iter()
        .enumerate()
    {
        azs.raw_set(idx + 1, az)?;
    }
    t.raw_set("availability_zones", azs)?;

    let tg = lua.create_table()?;
    for (idx, arn) in group
        .target_group_arns
        .unwrap_or_default()
        .into_iter()
        .enumerate()
    {
        tg.raw_set(idx + 1, arn)?;
    }
    t.raw_set("target_group_arns", tg)?;

    // Tags twice over, because neither shape answers both questions: `tags` is
    // the `{[key] = value}` map every other module hands back — what `.tags.Name`
    // reads, and what `create_or_update_tags` takes straight back — while
    // `tag_details` is the lossless one, keeping `propagate_at_launch`, which a
    // map has nowhere to put. Dropping it silently turned a round-trip through us
    // into an un-propagating copy of the group's tags.
    let tags = lua.create_table()?;
    let details = lua.create_table()?;
    let mut idx = 0;
    for tag in group.tags.unwrap_or_default() {
        let Some(key) = tag.key else { continue };
        if let Some(value) = tag.value.clone() {
            tags.raw_set(key.clone(), value)?;
        }
        let d = lua.create_table()?;
        d.raw_set("key", key)?;
        if let Some(value) = tag.value {
            d.raw_set("value", value)?;
        }
        // Absent means "the API did not say", not false; a group tag that was
        // never asked to propagate reports nothing rather than a made-up answer.
        if let Some(p) = tag.propagate_at_launch {
            d.raw_set("propagate_at_launch", p)?;
        }
        idx += 1;
        details.raw_set(idx, d)?;
    }
    t.raw_set("tags", tags)?;
    t.raw_set("tag_details", details)?;

    // Truncated at 50 by the API — `describe_instances` is the uncapped view.
    let instances = lua.create_table()?;
    for (idx, instance) in group.instances.unwrap_or_default().into_iter().enumerate() {
        instances.raw_set(idx + 1, member_to_table(lua, instance)?)?;
    }
    t.raw_set("instances", instances)?;

    Ok(t)
}

/// A group member, as embedded in the group table.
fn member_to_table(lua: &Lua, instance: Instance) -> LuaResult<Table> {
    let t = lua.create_table()?;
    if let Some(v) = instance.instance_id {
        t.raw_set("instance_id", v)?;
    }
    if let Some(v) = instance.instance_type {
        t.raw_set("instance_type", v)?;
    }
    if let Some(v) = instance.availability_zone {
        t.raw_set("availability_zone", v)?;
    }
    if let Some(v) = instance.lifecycle_state {
        t.raw_set("lifecycle_state", v.as_str().to_string())?;
    }
    if let Some(v) = instance.health_status {
        t.raw_set("health_status", v)?;
    }
    if let Some(v) = instance.image_id {
        t.raw_set("image_id", v)?;
    }
    if let Some(v) = instance.protected_from_scale_in {
        t.raw_set("protected_from_scale_in", v)?;
    }
    Ok(t)
}

// --- describe_instances ----------------------------------------------------

/// Builder for `DescribeAutoScalingInstances`, finalized with `:send()`.
///
/// The uncapped counterpart to a group table's `instances` field, which the API
/// truncates at 50. Also the only way to ask the reverse question — which group
/// an instance belongs to — since the answer carries `group_name`.
struct DescribeInstances {
    client: Client,
    ids: Arc<Mutex<Vec<String>>>,
}

impl UserData for DescribeInstances {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_async_function("send", |lua, ud: AnyUserData| async move {
            let (client, ids) = {
                let this = ud.borrow::<Self>()?;
                (this.client.clone(), this.ids.lock().unwrap().clone())
            };

            let mut req = client.describe_auto_scaling_instances();
            if !ids.is_empty() {
                req = req.set_instance_ids(Some(ids));
            }

            let out = lua.create_table()?;
            let mut idx = 0;
            let mut stream = req.into_paginator().items().send();
            while let Some(details) = stream.next().await {
                let details = details.map_err(|e| lua_err(BoxError::from(e)))?;
                idx += 1;
                out.raw_set(idx, instance_details_to_table(&lua, details)?)?;
            }

            respond(&lua, true, Value::Table(out))
        });
    }
}

fn instance_details_to_table(lua: &Lua, d: AutoScalingInstanceDetails) -> LuaResult<Table> {
    let t = lua.create_table()?;
    if let Some(v) = d.instance_id {
        t.raw_set("instance_id", v)?;
    }
    if let Some(v) = d.instance_type {
        t.raw_set("instance_type", v)?;
    }
    // The field the embedded form cannot give you.
    if let Some(v) = d.auto_scaling_group_name {
        t.raw_set("group_name", v)?;
    }
    if let Some(v) = d.availability_zone {
        t.raw_set("availability_zone", v)?;
    }
    if let Some(v) = d.lifecycle_state {
        t.raw_set("lifecycle_state", v)?;
    }
    if let Some(v) = d.health_status {
        t.raw_set("health_status", v)?;
    }
    if let Some(v) = d.image_id {
        t.raw_set("image_id", v)?;
    }
    if let Some(v) = d.protected_from_scale_in {
        t.raw_set("protected_from_scale_in", v)?;
    }
    Ok(t)
}

// --- set_desired_capacity --------------------------------------------------

/// Builder for `SetDesiredCapacity`, finalized with `:send()`.
struct SetDesiredCapacity {
    client: Client,
    group: String,
    capacity: i32,
    honor_cooldown: Arc<Mutex<bool>>,
}

impl UserData for SetDesiredCapacity {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        // Off by default, matching the API: a script asking for a capacity
        // usually means now, not "once the cooldown allows it".
        methods.add_function("honor_cooldown", |_, ud: AnyUserData| {
            *ud.borrow::<Self>()?.honor_cooldown.lock().unwrap() = true;
            Ok(ud)
        });

        methods.add_async_function("send", |lua, ud: AnyUserData| async move {
            let (client, group, capacity, honor_cooldown) = {
                let this = ud.borrow::<Self>()?;
                (
                    this.client.clone(),
                    this.group.clone(),
                    this.capacity,
                    *this.honor_cooldown.lock().unwrap(),
                )
            };

            client
                .set_desired_capacity()
                .auto_scaling_group_name(group)
                .desired_capacity(capacity)
                .honor_cooldown(honor_cooldown)
                .send()
                .await
                .map_err(|e| lua_err(BoxError::from(e)))?;

            // The API returns nothing; the envelope alone is the answer.
            respond(&lua, true, Value::Nil)
        });
    }
}

// --- update_group ----------------------------------------------------------

/// Shared request state, mutated in place by the builder methods.
#[derive(Default, Clone, PartialEq, Eq)]
struct UpdateGroupState {
    min_size: Option<i32>,
    max_size: Option<i32>,
    desired_capacity: Option<i32>,
    health_check_grace_period: Option<i32>,
    default_cooldown: Option<i32>,
}

/// Builder for `UpdateAutoScalingGroup`, finalized with `:send()`.
///
/// The call that reshapes a group. Every knob is optional and an unset one is
/// left alone, so this is a patch rather than a replacement.
struct UpdateGroup {
    client: Client,
    group: String,
    state: Arc<Mutex<UpdateGroupState>>,
}

impl UserData for UpdateGroup {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_function("min_size", |_, (ud, v): (AnyUserData, i32)| {
            ud.borrow::<Self>()?.state.lock().unwrap().min_size = Some(v);
            Ok(ud)
        });

        methods.add_function("max_size", |_, (ud, v): (AnyUserData, i32)| {
            ud.borrow::<Self>()?.state.lock().unwrap().max_size = Some(v);
            Ok(ud)
        });

        methods.add_function("desired_capacity", |_, (ud, v): (AnyUserData, i32)| {
            ud.borrow::<Self>()?.state.lock().unwrap().desired_capacity = Some(v);
            Ok(ud)
        });

        methods.add_function(
            "health_check_grace_period",
            |_, (ud, v): (AnyUserData, i32)| {
                ud.borrow::<Self>()?
                    .state
                    .lock()
                    .unwrap()
                    .health_check_grace_period = Some(v);
                Ok(ud)
            },
        );

        methods.add_function("default_cooldown", |_, (ud, v): (AnyUserData, i32)| {
            ud.borrow::<Self>()?.state.lock().unwrap().default_cooldown = Some(v);
            Ok(ud)
        });

        methods.add_async_function("send", |lua, ud: AnyUserData| async move {
            let (client, group, state) = {
                let this = ud.borrow::<Self>()?;
                (
                    this.client.clone(),
                    this.group.clone(),
                    this.state.lock().unwrap().clone(),
                )
            };

            // An update setting nothing would be a request that silently does
            // nothing, which is a script bug worth naming.
            if state == UpdateGroupState::default() {
                return Err(mlua::Error::runtime(
                    "update_group: set at least one of ':min_size()', ':max_size()', \
                     ':desired_capacity()', ':health_check_grace_period()' or \
                     ':default_cooldown()'",
                ));
            }

            let mut req = client
                .update_auto_scaling_group()
                .auto_scaling_group_name(group);
            if let Some(v) = state.min_size {
                req = req.min_size(v);
            }
            if let Some(v) = state.max_size {
                req = req.max_size(v);
            }
            if let Some(v) = state.desired_capacity {
                req = req.desired_capacity(v);
            }
            if let Some(v) = state.health_check_grace_period {
                req = req.health_check_grace_period(v);
            }
            if let Some(v) = state.default_cooldown {
                req = req.default_cooldown(v);
            }

            req.send().await.map_err(|e| lua_err(BoxError::from(e)))?;

            respond(&lua, true, Value::Nil)
        });
    }
}

// --- instance refresh ------------------------------------------------------

/// How often `:wait()` asks the API for progress.
///
/// A refresh takes minutes at least — it drains and replaces instances — so a
/// tighter interval would only spend rate limit to learn nothing.
const POLL_INTERVAL: Duration = Duration::from_secs(15);

/// Shared request state, mutated in place by the builder methods.
#[derive(Default, Clone)]
struct StartRefreshState {
    min_healthy_percentage: Option<i32>,
    max_healthy_percentage: Option<i32>,
    instance_warmup: Option<i32>,
    skip_matching: Option<bool>,
    auto_rollback: Option<bool>,
    strategy: Option<String>,
    wait: bool,
}

/// Builder for `StartInstanceRefresh`, finalized with `:send()`.
struct StartRefresh {
    client: Client,
    group: String,
    state: Arc<Mutex<StartRefreshState>>,
}

impl UserData for StartRefresh {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_function(
            "min_healthy_percentage",
            |_, (ud, v): (AnyUserData, i32)| {
                ud.borrow::<Self>()?
                    .state
                    .lock()
                    .unwrap()
                    .min_healthy_percentage = Some(v);
                Ok(ud)
            },
        );

        methods.add_function(
            "max_healthy_percentage",
            |_, (ud, v): (AnyUserData, i32)| {
                ud.borrow::<Self>()?
                    .state
                    .lock()
                    .unwrap()
                    .max_healthy_percentage = Some(v);
                Ok(ud)
            },
        );

        // Seconds to wait after an instance reaches InService before counting
        // it healthy; falls through to the group's own warmup when unset.
        methods.add_function("instance_warmup", |_, (ud, v): (AnyUserData, i32)| {
            ud.borrow::<Self>()?.state.lock().unwrap().instance_warmup = Some(v);
            Ok(ud)
        });

        // Leave instances that already match the desired configuration alone.
        methods.add_function("skip_matching", |_, ud: AnyUserData| {
            ud.borrow::<Self>()?.state.lock().unwrap().skip_matching = Some(true);
            Ok(ud)
        });

        methods.add_function("auto_rollback", |_, ud: AnyUserData| {
            ud.borrow::<Self>()?.state.lock().unwrap().auto_rollback = Some(true);
            Ok(ud)
        });

        methods.add_function("strategy", |_, (ud, v): (AnyUserData, String)| {
            ud.borrow::<Self>()?.state.lock().unwrap().strategy = Some(v);
            Ok(ud)
        });

        // Poll until the refresh settles, rather than returning the id and
        // leaving the loop to the script.
        methods.add_function("wait", |_, ud: AnyUserData| {
            ud.borrow::<Self>()?.state.lock().unwrap().wait = true;
            Ok(ud)
        });

        methods.add_async_function("send", |lua, ud: AnyUserData| async move {
            let (client, group, state) = {
                let this = ud.borrow::<Self>()?;
                (
                    this.client.clone(),
                    this.group.clone(),
                    this.state.lock().unwrap().clone(),
                )
            };

            let mut req = client
                .start_instance_refresh()
                .auto_scaling_group_name(&group);

            let prefs = RefreshPreferences::builder()
                .set_min_healthy_percentage(state.min_healthy_percentage)
                .set_max_healthy_percentage(state.max_healthy_percentage)
                .set_instance_warmup(state.instance_warmup)
                .set_skip_matching(state.skip_matching)
                .set_auto_rollback(state.auto_rollback)
                .build();
            req = req.preferences(prefs);

            if let Some(s) = &state.strategy {
                req = req.strategy(RefreshStrategy::from(s.as_str()));
            }

            let out = req.send().await.map_err(|e| lua_err(BoxError::from(e)))?;
            let refresh_id = out.instance_refresh_id.unwrap_or_default();

            if !state.wait {
                let t = lua.create_table()?;
                t.raw_set("instance_refresh_id", refresh_id)?;
                return respond(&lua, true, Value::Table(t));
            }

            // Poll to a terminal state. `sleep` yields, so other tasks — a
            // refresh on another group, say — run while this one waits.
            loop {
                tokio::time::sleep(POLL_INTERVAL).await;

                let page = client
                    .describe_instance_refreshes()
                    .auto_scaling_group_name(&group)
                    .instance_refresh_ids(&refresh_id)
                    .send()
                    .await
                    .map_err(|e| lua_err(BoxError::from(e)))?;

                let Some(refresh) = page
                    .instance_refreshes
                    .unwrap_or_default()
                    .into_iter()
                    .next()
                else {
                    // The refresh we just started is not being reported; there
                    // is nothing left to wait for.
                    return Err(mlua::Error::runtime(format!(
                        "start_instance_refresh: refresh '{refresh_id}' on '{group}' \
                         disappeared while waiting"
                    )));
                };

                let terminal = refresh.status.as_ref().is_some_and(is_terminal);
                if terminal {
                    // `ok` reflects the outcome: a failed or cancelled refresh
                    // is a real answer to "how did it go", not an error.
                    let ok = matches!(
                        refresh.status,
                        Some(InstanceRefreshStatus::Successful)
                            | Some(InstanceRefreshStatus::RollbackSuccessful)
                    );
                    let t = refresh_to_table(&lua, refresh)?;
                    return respond(&lua, ok, Value::Table(t));
                }
            }
        });
    }
}

/// Shared request state for the refresh calls that name a group and act on it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RefreshAction {
    Cancel,
    Rollback,
}

impl RefreshAction {
    fn name(self) -> &'static str {
        match self {
            Self::Cancel => "cancel_instance_refresh",
            Self::Rollback => "rollback_instance_refresh",
        }
    }
}

/// Builder for `CancelInstanceRefresh`/`RollbackInstanceRefresh`.
///
/// One type over an enum: both take just a group name and both return just the
/// refresh id, so two copies would be two places to keep in step.
struct RefreshControl {
    client: Client,
    action: RefreshAction,
    group: String,
}

impl UserData for RefreshControl {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_async_function("send", |lua, ud: AnyUserData| async move {
            let (client, action, group) = {
                let this = ud.borrow::<Self>()?;
                (this.client.clone(), this.action, this.group.clone())
            };

            // Both raise when there is no refresh in progress on the group;
            // the action's own name makes that message say which call it was.
            let named = |e: BoxError| {
                mlua::Error::runtime(format!(
                    "{}: {}",
                    action.name(),
                    super::format_error_chain(&e)
                ))
            };

            let refresh_id = match action {
                RefreshAction::Cancel => {
                    client
                        .cancel_instance_refresh()
                        .auto_scaling_group_name(group)
                        .send()
                        .await
                        .map_err(|e| named(BoxError::from(e)))?
                        .instance_refresh_id
                }
                RefreshAction::Rollback => {
                    client
                        .rollback_instance_refresh()
                        .auto_scaling_group_name(group)
                        .send()
                        .await
                        .map_err(|e| named(BoxError::from(e)))?
                        .instance_refresh_id
                }
            };

            let t = lua.create_table()?;
            if let Some(v) = refresh_id {
                t.raw_set("instance_refresh_id", v)?;
            }
            respond(&lua, true, Value::Table(t))
        });
    }
}

/// Builder for `DescribeInstanceRefreshes`, finalized with `:send()`.
struct DescribeRefreshes {
    client: Client,
    group: String,
    ids: Arc<Mutex<Vec<String>>>,
}

impl UserData for DescribeRefreshes {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_function("id", |_, (ud, id): (AnyUserData, String)| {
            ud.borrow::<Self>()?.ids.lock().unwrap().push(id);
            Ok(ud)
        });

        methods.add_async_function("send", |lua, ud: AnyUserData| async move {
            let (client, group, ids) = {
                let this = ud.borrow::<Self>()?;
                (
                    this.client.clone(),
                    this.group.clone(),
                    this.ids.lock().unwrap().clone(),
                )
            };

            let mut req = client
                .describe_instance_refreshes()
                .auto_scaling_group_name(group);
            if !ids.is_empty() {
                req = req.set_instance_refresh_ids(Some(ids));
            }

            // This paginator has no `items()` flattening, so the refreshes come
            // out of each page rather than off the stream.
            let out = lua.create_table()?;
            let mut idx = 0;
            let mut stream = req.into_paginator().send();
            while let Some(page) = stream.next().await {
                let page = page.map_err(|e| lua_err(BoxError::from(e)))?;
                for refresh in page.instance_refreshes.unwrap_or_default() {
                    idx += 1;
                    out.raw_set(idx, refresh_to_table(&lua, refresh)?)?;
                }
            }

            // Newest first, as the API returns them.
            respond(&lua, true, Value::Table(out))
        });
    }
}

fn refresh_to_table(lua: &Lua, refresh: InstanceRefresh) -> LuaResult<Table> {
    let t = lua.create_table()?;

    if let Some(v) = refresh.instance_refresh_id {
        t.raw_set("instance_refresh_id", v)?;
    }
    if let Some(v) = refresh.auto_scaling_group_name {
        t.raw_set("group_name", v)?;
    }
    if let Some(v) = refresh.status {
        t.raw_set("status", v.as_str().to_string())?;
    }
    if let Some(v) = refresh.status_reason {
        t.raw_set("status_reason", v)?;
    }
    if let Some(v) = refresh.percentage_complete {
        t.raw_set("percentage_complete", v)?;
    }
    if let Some(v) = refresh.instances_to_update {
        t.raw_set("instances_to_update", v)?;
    }
    if let Some(v) = refresh.start_time.as_ref().and_then(date_to_string) {
        t.raw_set("start_time", v)?;
    }
    if let Some(v) = refresh.end_time.as_ref().and_then(date_to_string) {
        t.raw_set("end_time", v)?;
    }

    Ok(t)
}

// --- tagging ---------------------------------------------------------------

/// Which of the two tag-writing calls a [`TagWrite`] is.
#[derive(Clone, Copy, PartialEq, Eq)]
enum TagAction {
    CreateOrUpdate,
    Delete,
}

impl TagAction {
    fn name(self) -> &'static str {
        match self {
            Self::CreateOrUpdate => "create_or_update_tags",
            Self::Delete => "delete_tags",
        }
    }
}

/// A tag as the script gave it: a key, and a value only when one was named.
///
/// `create_or_update_tags` always has a value; `delete_tags` may not, and there
/// the absence means "whatever value it currently holds" rather than empty.
#[derive(Clone, Debug)]
struct TagEntry {
    key: String,
    value: Option<String>,
}

/// Shared request state, mutated in place by the builder methods.
#[derive(Default, Clone)]
struct TagWriteState {
    /// `None` until `:tags()` is called — the state that must be an error.
    tags: Option<Vec<TagEntry>>,
    propagate_at_launch: bool,
}

/// Builder for `CreateOrUpdateTags`/`DeleteTags`, finalized with `:send()`.
///
/// One type over an enum, as `RefreshControl` is: both name a group, both take a
/// tag set and both return nothing, so the only real difference is which shapes
/// `:tags()` accepts.
struct TagWrite {
    client: Client,
    action: TagAction,
    group: String,
    state: Arc<Mutex<TagWriteState>>,
}

/// What `:propagate_at_launch([bool])` means, given its optional argument.
///
/// Bare is true: reaching for the method at all is asking for propagation, and a
/// caller who wants it off simply does not call it (or says so explicitly, which
/// stays readable next to a conditional).
fn propagate_arg(on: Option<bool>) -> bool {
    on.unwrap_or(true)
}

/// Read a `:tags()` argument into entries, per the action's accepted shapes.
///
/// Both halves of the table count: the sequence part is a list of bare keys (a
/// delete, where a missing value means "whatever it currently holds") and the
/// string keys are a `{[key] = value}` map. A table carrying both means both,
/// rather than one half silently losing to the other — the same rule
/// `aws.ec2`'s tag writers follow. Entries come back sorted by key, `pairs`
/// order over a Luau table being unspecified.
fn tags_from_value(action: TagAction, value: Value) -> LuaResult<Vec<TagEntry>> {
    let Value::Table(t) = value else {
        return Err(mlua::Error::runtime(format!(
            "{}: expected a table of tags, got {}",
            action.name(),
            value.type_name()
        )));
    };

    let mut out = Vec::new();

    // The sequence part is bare keys — what a delete needs, and meaningless to
    // a create, which has no value to set.
    let seq_len = t.raw_len();
    for idx in 1..=seq_len {
        let key: String = t.raw_get(idx)?;
        if action == TagAction::CreateOrUpdate {
            return Err(mlua::Error::runtime(format!(
                "{}: expected a {{[key] = value}} map of tags, not a list of keys",
                action.name()
            )));
        }
        out.push(TagEntry { key, value: None });
    }

    // Then the `{[key] = value}` half, skipping the sequence indices already
    // consumed. A table carrying both means both, rather than one silently
    // losing to the other — the same rule `aws.ec2`'s tag writers follow.
    for pair in t.pairs::<Value, Value>() {
        let (k, v) = pair?;
        // Luau may hand an integer key back as either representation, so both
        // are checked rather than trusting one.
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
                    "{}: tag keys must be strings, got {}",
                    action.name(),
                    other.type_name()
                )));
            }
        };
        let value = match v {
            Value::String(s) => s.to_str()?.to_string(),
            other => {
                return Err(mlua::Error::runtime(format!(
                    "{}: tag value for {key:?} must be a string, got {}",
                    action.name(),
                    other.type_name()
                )));
            }
        };
        out.push(TagEntry {
            key,
            value: Some(value),
        });
    }
    // Deterministic order: `pairs` over a Luau table is unspecified, and a
    // request whose tag order changes between runs is needlessly hard to read.
    out.sort_by(|a, b| a.key.cmp(&b.key));
    Ok(out)
}

impl UserData for TagWrite {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_function("tags", |_, (ud, value): (AnyUserData, Value)| {
            let action = ud.borrow::<Self>()?.action;
            let tags = tags_from_value(action, value)?;
            ud.borrow::<Self>()?.state.lock().unwrap().tags = Some(tags);
            Ok(ud)
        });

        // Builder-wide, and true when called bare: reaching for the method at
        // all is asking for propagation.
        methods.add_function(
            "propagate_at_launch",
            |_, (ud, on): (AnyUserData, Option<bool>)| {
                {
                    let this = ud.borrow::<Self>()?;
                    if this.action == TagAction::Delete {
                        return Err(mlua::Error::runtime(
                            "delete_tags: ':propagate_at_launch()' applies to \
                             'create_or_update_tags'; a delete removes the tag either way",
                        ));
                    }
                    this.state.lock().unwrap().propagate_at_launch = propagate_arg(on);
                }
                Ok(ud)
            },
        );

        methods.add_async_function("send", |lua, ud: AnyUserData| async move {
            let (client, action, group, state) = {
                let this = ud.borrow::<Self>()?;
                (
                    this.client.clone(),
                    this.action,
                    this.group.clone(),
                    this.state.lock().unwrap().clone(),
                )
            };

            if group.is_empty() {
                return Err(mlua::Error::runtime(format!(
                    "{}: expected an Auto Scaling group name, got an empty string",
                    action.name()
                )));
            }

            // Never set is an error rather than an empty request: upstream reads
            // an absent tag set on a delete as "every tag on the group".
            let Some(tags) = state.tags else {
                return Err(mlua::Error::runtime(format!(
                    "{}: set the tags with ':tags()'",
                    action.name()
                )));
            };
            if tags.is_empty() {
                return Err(mlua::Error::runtime(format!(
                    "{}: ':tags()' was given an empty table",
                    action.name()
                )));
            }

            let tags = tags
                .into_iter()
                .map(|entry| {
                    let mut b = aws_sdk_autoscaling::types::Tag::builder()
                        .resource_id(group.clone())
                        // Always this: a group is the only thing this API tags.
                        .resource_type("auto-scaling-group")
                        .key(entry.key);
                    if let Some(value) = entry.value {
                        b = b.value(value);
                    }
                    if action == TagAction::CreateOrUpdate {
                        b = b.propagate_at_launch(state.propagate_at_launch);
                    }
                    b.build()
                })
                .collect::<Vec<_>>();

            let named = |e: BoxError| {
                mlua::Error::runtime(format!(
                    "{}: {}",
                    action.name(),
                    super::format_error_chain(&e)
                ))
            };

            match action {
                TagAction::CreateOrUpdate => {
                    client
                        .create_or_update_tags()
                        .set_tags(Some(tags))
                        .send()
                        .await
                        .map_err(|e| named(BoxError::from(e)))?;
                }
                TagAction::Delete => {
                    client
                        .delete_tags()
                        .set_tags(Some(tags))
                        .send()
                        .await
                        .map_err(|e| named(BoxError::from(e)))?;
                }
            }

            // Neither API returns anything; the envelope alone is the answer.
            respond(&lua, true, Value::Nil)
        });
    }
}

// --- module ----------------------------------------------------------------

/// Build the `aws.autoscaling` module table over `source`.
///
/// Called once for the `aws.autoscaling` global (ambient configuration) and
/// again per `aws.config():build()`, so the surface cannot differ between them.
pub(crate) fn module(lua: &Lua, source: ClientSource) -> LuaResult<Table> {
    let exports = lua.create_table()?;

    let c = source.clone();
    exports.raw_set(
        "describe_groups",
        lua.create_async_function(move |_, ()| {
            let c = c.clone();
            async move {
                Ok(DescribeGroups {
                    client: c.autoscaling().await,
                    state: Arc::new(Mutex::new(DescribeGroupsState::default())),
                })
            }
        })?,
    )?;

    let c = source.clone();
    exports.raw_set(
        "describe_instances",
        lua.create_async_function(move |_, ids: Value| {
            let c = c.clone();
            async move {
                // Optional: no ids means every instance in the account.
                let ids = match ids {
                    Value::Nil => Vec::new(),
                    other => value_to_string_list(other)?,
                };
                Ok(DescribeInstances {
                    client: c.autoscaling().await,
                    ids: Arc::new(Mutex::new(ids)),
                })
            }
        })?,
    )?;

    let c = source.clone();
    exports.raw_set(
        "set_desired_capacity",
        lua.create_async_function(move |_, (group, capacity): (String, i32)| {
            let c = c.clone();
            async move {
                Ok(SetDesiredCapacity {
                    client: c.autoscaling().await,
                    group,
                    capacity,
                    honor_cooldown: Arc::new(Mutex::new(false)),
                })
            }
        })?,
    )?;

    let c = source.clone();
    exports.raw_set(
        "update_group",
        lua.create_async_function(move |_, group: String| {
            let c = c.clone();
            async move {
                Ok(UpdateGroup {
                    client: c.autoscaling().await,
                    group,
                    state: Arc::new(Mutex::new(UpdateGroupState::default())),
                })
            }
        })?,
    )?;

    let c = source.clone();
    exports.raw_set(
        "start_instance_refresh",
        lua.create_async_function(move |_, group: String| {
            let c = c.clone();
            async move {
                Ok(StartRefresh {
                    client: c.autoscaling().await,
                    group,
                    state: Arc::new(Mutex::new(StartRefreshState::default())),
                })
            }
        })?,
    )?;

    let c = source.clone();
    exports.raw_set(
        "describe_instance_refreshes",
        lua.create_async_function(move |_, group: String| {
            let c = c.clone();
            async move {
                Ok(DescribeRefreshes {
                    client: c.autoscaling().await,
                    group,
                    ids: Arc::new(Mutex::new(Vec::new())),
                })
            }
        })?,
    )?;

    // The two tag writers differ only in the enum they carry, so they are
    // registered from one list rather than two near-identical closures.
    for (name, action) in [
        ("create_or_update_tags", TagAction::CreateOrUpdate),
        ("delete_tags", TagAction::Delete),
    ] {
        let c = source.clone();
        exports.raw_set(
            name,
            lua.create_async_function(move |_, group: String| {
                let c = c.clone();
                async move {
                    Ok(TagWrite {
                        client: c.autoscaling().await,
                        action,
                        group,
                        state: Arc::new(Mutex::new(TagWriteState::default())),
                    })
                }
            })?,
        )?;
    }

    for (name, action) in [
        ("cancel_instance_refresh", RefreshAction::Cancel),
        ("rollback_instance_refresh", RefreshAction::Rollback),
    ] {
        let c = source.clone();
        exports.raw_set(
            name,
            lua.create_async_function(move |_, group: String| {
                let c = c.clone();
                async move {
                    Ok(RefreshControl {
                        client: c.autoscaling().await,
                        action,
                        group,
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
    use aws_sdk_autoscaling::types::{LifecycleState, TagDescription};

    #[test]
    fn converts_group_fields_and_members() {
        let lua = Lua::new();
        let group = AutoScalingGroup::builder()
            .auto_scaling_group_name("web-prod")
            .min_size(2)
            .max_size(10)
            .desired_capacity(4)
            .health_check_type("ELB")
            .availability_zones("eu-central-1a")
            .tags(
                TagDescription::builder()
                    .key("Environment")
                    .value("prod")
                    .propagate_at_launch(true)
                    .build(),
            )
            .instances(
                Instance::builder()
                    .instance_id("i-abc")
                    .lifecycle_state(LifecycleState::InService)
                    .health_status("Healthy")
                    .build(),
            )
            .build();

        let t = group_to_table(&lua, group).unwrap();

        assert_eq!(t.get::<String>("name").unwrap(), "web-prod");
        assert_eq!(t.get::<i32>("min_size").unwrap(), 2);
        assert_eq!(t.get::<i32>("max_size").unwrap(), 10);
        assert_eq!(t.get::<i32>("desired_capacity").unwrap(), 4);
        assert_eq!(t.get::<String>("health_check_type").unwrap(), "ELB");
        assert_eq!(
            t.get::<Table>("tags")
                .unwrap()
                .get::<String>("Environment")
                .unwrap(),
            "prod"
        );

        let instances = t.get::<Table>("instances").unwrap();
        let first = instances.get::<Table>(1).unwrap();
        assert_eq!(first.get::<String>("instance_id").unwrap(), "i-abc");
        assert_eq!(first.get::<String>("lifecycle_state").unwrap(), "InService");
        assert_eq!(first.get::<String>("health_status").unwrap(), "Healthy");
    }

    /// The collection fields are always tables, so indexing needs no guard.
    #[test]
    fn absent_collections_become_empty_tables() {
        let lua = Lua::new();
        let group = AutoScalingGroup::builder()
            .auto_scaling_group_name("empty")
            .build();

        let t = group_to_table(&lua, group).unwrap();

        for key in [
            "instances",
            "tags",
            "tag_details",
            "availability_zones",
            "target_group_arns",
        ] {
            let v = t
                .get::<Table>(key)
                .unwrap_or_else(|_| panic!("{key} must be a table"));
            assert_eq!(v.raw_len(), 0, "{key} must be empty");
        }
    }

    /// The standalone view carries the group name, which the embedded one cannot.
    #[test]
    fn instance_details_carry_the_group_name() {
        let lua = Lua::new();
        let details = AutoScalingInstanceDetails::builder()
            .instance_id("i-abc")
            .auto_scaling_group_name("web-prod")
            .lifecycle_state("InService")
            .health_status("Healthy")
            .build();

        let t = instance_details_to_table(&lua, details).unwrap();

        assert_eq!(t.get::<String>("instance_id").unwrap(), "i-abc");
        assert_eq!(t.get::<String>("group_name").unwrap(), "web-prod");
        assert_eq!(t.get::<String>("lifecycle_state").unwrap(), "InService");
    }

    #[test]
    fn converts_refresh_fields() {
        let lua = Lua::new();
        let refresh = InstanceRefresh::builder()
            .instance_refresh_id("ir-abc")
            .auto_scaling_group_name("web-prod")
            .status(InstanceRefreshStatus::InProgress)
            .percentage_complete(40)
            .instances_to_update(3)
            .start_time(aws_smithy_types::DateTime::from_secs(1_700_000_000))
            .build();

        let t = refresh_to_table(&lua, refresh).unwrap();

        assert_eq!(t.get::<String>("instance_refresh_id").unwrap(), "ir-abc");
        assert_eq!(t.get::<String>("group_name").unwrap(), "web-prod");
        assert_eq!(t.get::<String>("status").unwrap(), "InProgress");
        assert_eq!(t.get::<i32>("percentage_complete").unwrap(), 40);
        assert_eq!(
            t.get::<String>("start_time").unwrap(),
            "2023-11-14T22:13:20Z"
        );
    }

    /// `:wait()` must stop on exactly the statuses a refresh does not leave.
    #[test]
    fn terminal_statuses_end_the_wait() {
        for status in [
            InstanceRefreshStatus::Successful,
            InstanceRefreshStatus::Failed,
            InstanceRefreshStatus::Cancelled,
            InstanceRefreshStatus::RollbackSuccessful,
            InstanceRefreshStatus::RollbackFailed,
        ] {
            assert!(is_terminal(&status), "{status:?} must be terminal");
        }

        for status in [
            InstanceRefreshStatus::Pending,
            InstanceRefreshStatus::InProgress,
            InstanceRefreshStatus::Baking,
            InstanceRefreshStatus::Cancelling,
            InstanceRefreshStatus::RollbackInProgress,
        ] {
            assert!(!is_terminal(&status), "{status:?} must keep polling");
        }
    }

    /// A status this SDK does not model is treated as terminal: waiting forever
    /// on a value we cannot interpret is the worse failure.
    #[test]
    fn unknown_status_is_terminal() {
        let status = InstanceRefreshStatus::from("SomethingNewAwsAdded");
        assert!(is_terminal(&status));
    }

    /// The map is the ergonomic read, `tag_details` the lossless one — and only
    /// the latter can carry `PropagateAtLaunch`.
    #[test]
    fn tag_details_keep_propagate_at_launch() {
        let lua = Lua::new();
        let group = AutoScalingGroup::builder()
            .auto_scaling_group_name("web-prod")
            .tags(
                TagDescription::builder()
                    .key("Environment")
                    .value("prod")
                    .propagate_at_launch(true)
                    .build(),
            )
            .tags(
                TagDescription::builder()
                    .key("Owner")
                    .value("platform")
                    .propagate_at_launch(false)
                    .build(),
            )
            .build();

        let t = group_to_table(&lua, group).unwrap();

        let tags = t.get::<Table>("tags").unwrap();
        assert_eq!(tags.get::<String>("Environment").unwrap(), "prod");
        assert_eq!(tags.get::<String>("Owner").unwrap(), "platform");

        let details = t.get::<Table>("tag_details").unwrap();
        assert_eq!(details.raw_len(), 2);
        let first = details.get::<Table>(1).unwrap();
        assert_eq!(first.get::<String>("key").unwrap(), "Environment");
        assert_eq!(first.get::<String>("value").unwrap(), "prod");
        assert!(first.get::<bool>("propagate_at_launch").unwrap());
        let second = details.get::<Table>(2).unwrap();
        assert_eq!(second.get::<String>("key").unwrap(), "Owner");
        assert!(!second.get::<bool>("propagate_at_launch").unwrap());
    }

    /// An unset `PropagateAtLaunch` is reported as absent, not as false.
    #[test]
    fn tag_details_omit_an_unset_propagate_flag() {
        let lua = Lua::new();
        let group = AutoScalingGroup::builder()
            .auto_scaling_group_name("web-prod")
            .tags(TagDescription::builder().key("Name").value("web").build())
            .build();

        let t = group_to_table(&lua, group).unwrap();
        let first = t
            .get::<Table>("tag_details")
            .unwrap()
            .get::<Table>(1)
            .unwrap();

        assert_eq!(first.get::<String>("key").unwrap(), "Name");
        assert!(first.get::<Value>("propagate_at_launch").unwrap().is_nil());
    }

    /// A delete takes either shape; the map form deletes only on a value match,
    /// the key list regardless of value.
    #[test]
    fn delete_tags_reads_both_a_map_and_a_key_list() {
        let lua = Lua::new();

        let map = lua.create_table().unwrap();
        map.raw_set("Env", "prod").unwrap();
        map.raw_set("Name", "web").unwrap();
        let entries = tags_from_value(TagAction::Delete, Value::Table(map)).unwrap();
        // Sorted, so the assertion does not depend on `pairs` order.
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].key, "Env");
        assert_eq!(entries[0].value.as_deref(), Some("prod"));
        assert_eq!(entries[1].key, "Name");

        let list = lua.create_table().unwrap();
        list.raw_set(1, "Env").unwrap();
        list.raw_set(2, "Name").unwrap();
        let entries = tags_from_value(TagAction::Delete, Value::Table(list)).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].key, "Env");
        // Key-only: the value is omitted from the request, so any value matches.
        assert!(entries[0].value.is_none());
        assert_eq!(entries[1].key, "Name");
    }

    /// A table carrying both halves means both — neither silently loses to the
    /// other, matching `aws.ec2`'s tag writers.
    #[test]
    fn delete_tags_reads_a_mixed_table_as_both() {
        let lua = Lua::new();
        let mixed = lua.create_table().unwrap();
        mixed.raw_set(1, "Legacy").unwrap();
        mixed.raw_set("Env", "prod").unwrap();

        let entries = tags_from_value(TagAction::Delete, Value::Table(mixed)).unwrap();
        assert_eq!(entries.len(), 2);
        // Sorted by key: "Env" (from the map) before "Legacy" (from the list).
        assert_eq!(entries[0].key, "Env");
        assert_eq!(entries[0].value.as_deref(), Some("prod"));
        assert_eq!(entries[1].key, "Legacy");
        assert!(entries[1].value.is_none());
    }

    /// A write needs values, so the key-list shape is refused there rather than
    /// silently writing empty tags.
    #[test]
    fn create_or_update_tags_refuses_a_key_list() {
        let lua = Lua::new();

        let list = lua.create_table().unwrap();
        list.raw_set(1, "Env").unwrap();
        let err = tags_from_value(TagAction::CreateOrUpdate, Value::Table(list)).unwrap_err();
        assert!(
            err.to_string().contains("create_or_update_tags"),
            "message names the action: {err}"
        );

        let map = lua.create_table().unwrap();
        map.raw_set("Env", "prod").unwrap();
        let entries = tags_from_value(TagAction::CreateOrUpdate, Value::Table(map)).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].value.as_deref(), Some("prod"));
    }

    /// Not a table at all is named as such, rather than reaching AWS.
    #[test]
    fn tags_rejects_a_non_table() {
        let err = tags_from_value(TagAction::Delete, Value::Integer(42)).unwrap_err();
        assert!(
            err.to_string().contains("expected a table of tags"),
            "unexpected message: {err}"
        );
    }

    /// Off unless asked, and true when `:propagate_at_launch()` is called bare.
    #[test]
    fn propagate_at_launch_defaults() {
        // Never called: the request carries `false`.
        assert!(!TagWriteState::default().propagate_at_launch);
        // Called bare, and called explicitly either way.
        assert!(propagate_arg(None));
        assert!(propagate_arg(Some(true)));
        assert!(!propagate_arg(Some(false)));
    }
}
