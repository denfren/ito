//! Luau bindings for AWS S3, registered as `aws.s3`.
//!
//! Mostly read-only: the operations that answer questions about buckets and
//! objects, plus the tag writes (see *Tagging*). Object bodies are read-only —
//! nothing here uploads or deletes an object.
//!
//! Required arguments go to the entry function — `get_object(bucket, key)`,
//! since two arguments in that order are unambiguous — and the returned builder
//! carries the optional parts until `:send()`.
//!
//! # The two return values
//!
//! Every `:send()` either raises or returns **two** values:
//!
//! 1. a table describing the call, always carrying `ok` — "what you expect when
//!    everything goes right". Test it first, then read the rest.
//! 2. the response as native Luau data, or — when `:send()` was given a target —
//!    a reference to where the body went.
//!
//! ```lua
//! local r, buckets = aws.s3.list_buckets():send()
//! local r, body = aws.s3.get_object("b", "k"):send()
//! local r, path = aws.s3.get_object("b", "k"):send(proc.file("out.tar"))
//! ```
//!
//! `ok` is false only where non-existence is a normal answer (a missing object).
//! Credentials, permissions, connectivity and a missing *bucket* raise instead:
//! those are bugs, not answers.
//!
//! A target is accepted by every operation, not just `get_object`, and streams
//! the body verbatim — `proc`'s own redirect values, so there is one vocabulary
//! for bytes across `proc`, `ssh` and `s3`. Note that only `get_object` has a
//! payload body: the list operations' wire format is XML, so a target on those
//! receives the SDK-parsed data re-serialized as JSON rather than the raw
//! response.
//!
//! # Versions
//!
//! `list_object_versions` is the version-aware listing: on a versioned bucket
//! `list_objects` only ever reports current keys, so noncurrent versions and
//! delete markers are invisible to it. It is a separate operation rather than a
//! flag on `list_objects` because its result has a different *shape* — two
//! arrays, one of versions and one of delete markers, since a delete marker is
//! not an object and carries no size or storage class. Reaching a specific
//! version once you have its id is `get_object`/`head_object` with
//! `:version_id(id)`.
//!
//! # Tagging
//!
//! Tags are their own six operations because S3 gives them no other door. A
//! listing never carries them and `head_object` only reports how *many* an
//! object has (the `x-amz-tagging-count` header), so reading one object's tags
//! is always one more request — which is why a sweep over a prefix costs a
//! request per key, and why these functions have to exist rather than being a
//! field on `list_objects`.
//!
//! `put_object_tagging`/`put_bucket_tagging` **replace the whole tag set**. They
//! are not a merge, and S3 offers no merging form: adding one tag means
//! `get_*_tagging`, modifying the returned table, and putting it back. The read
//! side hands back exactly the shape the write side takes — a `{[key]=value}`
//! map — so that round-trip needs no reshaping.
//!
//! ```lua
//! local r, tags = aws.s3.get_object_tagging("b", "k"):send()
//! tags.Reviewed = "yes"
//! aws.s3.put_object_tagging("b", "k"):tags(tags):send()
//! ```
//!
//! `:tags()` is required on a put: an omitted one would silently wipe the set,
//! and wiping is what `delete_*_tagging` says out loud.
//!
//! A bucket with no tag set answers `GetBucketTagging` with the `NoSuchTagSet`
//! error. That is not a failure — it is "no tags" — so it comes back `ok = true`
//! with an empty table, the same as an object with no tags. `ok` stays reserved
//! for existence questions, and a tag read has none: an untagged bucket is a
//! bucket.
//!
//! # Input, for whenever writing objects lands
//!
//! Object *bodies* are not part of this module yet. When they arrive the
//! convention is `:body(<value>)` on the builder — a string or buffer is the
//! body verbatim, a table is serialized to JSON, and `proc.file(path)` streams
//! from disk — so `:send()` keeps meaning one thing in both directions.

use aws_sdk_s3::Client;
use aws_sdk_s3::error::SdkError;
use aws_sdk_s3::operation::get_object::GetObjectError;
use aws_sdk_s3::operation::head_object::HeadObjectError;
use aws_sdk_s3::types::{Bucket, DeleteMarkerEntry, Object, ObjectVersion, Tag, Tagging};
use aws_smithy_types::date_time::Format;
use aws_smithy_types::error::metadata::ProvideErrorMetadata;
use mlua::{
    AnyUserData, Lua, LuaSerdeExt, MultiValue, Result as LuaResult, Table, UserData,
    UserDataMethods, Value,
};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use super::{BoxError, ClientSource, lua_err};
use crate::scripting::redirect::{Redirect, capture_to_value, open_shared_file};

/// Format a timestamp as RFC 3339, which keeps it comparable and printable in
/// Luau, which has no native date type.
fn date_to_string(dt: &aws_smithy_types::DateTime) -> Option<String> {
    dt.fmt(Format::DateTime).ok()
}

/// The `{ ok = .. }` envelope every operation returns as its first value.
fn envelope(lua: &Lua, ok: bool) -> LuaResult<Table> {
    let t = lua.create_table()?;
    t.raw_set("ok", ok)?;
    Ok(t)
}

/// Pull a body target out of a Luau value.
///
/// The same `Redirect` values `proc` and `ssh` take, minus the two that cannot
/// mean anything for a single response body: `INHERIT` would write to stdout,
/// which a report owning stdout must not be interleaved with, and `STDOUT`/
/// `STDERR` have no other stream here to alias.
fn target_from_value(value: &Value) -> LuaResult<Option<Redirect>> {
    let redirect = match value {
        Value::Nil => return Ok(None),
        Value::UserData(ud) => ud.borrow::<Redirect>()?.clone(),
        other => {
            return Err(mlua::Error::runtime(format!(
                "expected a body target (proc.string(), proc.buffer(), proc.file(path) \
                 or proc.null()), got {}",
                other.type_name()
            )));
        }
    };
    match redirect {
        Redirect::Inherit(_) | Redirect::SameAsOther => Err(mlua::Error::runtime(
            "expected a body target (proc.string(), proc.buffer(), proc.file(path) \
             or proc.null())",
        )),
        concrete => Ok(Some(concrete)),
    }
}

/// Send `bytes` to a target and return the second return value: the captured
/// value, or the path for a file target, or nil for a discard.
///
/// Used for the operations whose body is not a stream — the SDK has already
/// parsed those, so a target receives the data re-serialized rather than the
/// original XML.
async fn deliver_bytes(lua: &Lua, target: &Redirect, bytes: &[u8]) -> LuaResult<Value> {
    match target {
        Redirect::Capture { kind, .. } => capture_to_value(lua, *kind, bytes),
        Redirect::Null => Ok(Value::Nil),
        Redirect::File { path, file } => {
            let handle = open_shared_file(path, file)?;
            write_all_blocking(handle, bytes.to_vec(), path).await?;
            Ok(Value::String(
                lua.create_string(path.to_string_lossy().as_bytes())?,
            ))
        }
        Redirect::Inherit(_) | Redirect::SameAsOther => {
            unreachable!("rejected by target_from_value")
        }
    }
}

/// Write on the blocking pool: a blocking write on this thread would pin the
/// runtime's only thread and serialize every concurrent task.
async fn write_all_blocking(
    mut file: std::fs::File,
    bytes: Vec<u8>,
    path: &std::path::Path,
) -> LuaResult<std::fs::File> {
    let display = path.display().to_string();
    let (file, res) = tokio::task::spawn_blocking(move || {
        use std::io::Write;
        let res = file.write_all(&bytes);
        (file, res)
    })
    .await
    .map_err(|e| mlua::Error::runtime(format!("write task failed: {e}")))?;
    res.map_err(|e| mlua::Error::runtime(format!("failed to write '{display}': {e}")))?;
    Ok(file)
}

/// Serialize a Luau value to compact JSON, for the case where a target asked for
/// bytes but the operation only ever had parsed data.
fn to_json_bytes(lua: &Lua, value: &Value) -> LuaResult<Vec<u8>> {
    let json: serde_json::Value = lua.from_value(value.clone())?;
    serde_json::to_vec(&json)
        .map_err(|e| mlua::Error::runtime(format!("failed to serialize response: {e}")))
}

/// Return `(envelope, data)` — or, when a target was given, `(envelope, ref)`
/// with the data re-serialized as JSON and sent to the target.
///
/// A `nil` `data` means there was no response to deliver (the object does not
/// exist), so a target is left untouched: a not-found must not create a file or
/// write `null` into one.
async fn respond(
    lua: &Lua,
    ok: bool,
    data: Value,
    target: Option<Redirect>,
) -> LuaResult<MultiValue> {
    let env = envelope(lua, ok)?;
    let second = match target {
        Some(target) if !data.is_nil() => {
            let bytes = to_json_bytes(lua, &data)?;
            deliver_bytes(lua, &target, &bytes).await?
        }
        _ => data,
    };
    Ok(MultiValue::from_vec(vec![Value::Table(env), second]))
}

// --- list_buckets ----------------------------------------------------------

/// Builder for `ListBuckets`. Takes no arguments; it exists so that every
/// operation is configured and finalized the same way.
struct ListBuckets {
    client: Client,
}

impl UserData for ListBuckets {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_async_function(
            "send",
            |lua, (ud, target): (AnyUserData, Value)| async move {
                let target = target_from_value(&target)?;
                // The borrow is scoped: it must not be held across the await, or
                // a task touching this builder meanwhile would hit a borrow error.
                let client = ud.borrow::<Self>()?.client.clone();

                // Paginated: the account may hold more buckets than one page.
                let mut stream = client.list_buckets().into_paginator().send();
                let buckets = lua.create_table()?;
                let mut idx = 0;
                while let Some(page) = stream.next().await {
                    let page = page.map_err(|e| lua_err(BoxError::from(e)))?;
                    for bucket in page.buckets.unwrap_or_default() {
                        idx += 1;
                        buckets.raw_set(idx, bucket_to_table(&lua, bucket)?)?;
                    }
                }

                // A missing bucket list is not a thing; anything that could make
                // this fail has already raised.
                respond(&lua, true, Value::Table(buckets), target).await
            },
        );
    }
}

fn bucket_to_table(lua: &Lua, bucket: Bucket) -> LuaResult<Table> {
    let t = lua.create_table()?;
    if let Some(v) = bucket.name {
        t.raw_set("name", v)?;
    }
    if let Some(v) = bucket.bucket_region {
        t.raw_set("region", v)?;
    }
    if let Some(v) = bucket.bucket_arn {
        t.raw_set("arn", v)?;
    }
    if let Some(v) = bucket.creation_date.as_ref().and_then(date_to_string) {
        t.raw_set("creation_date", v)?;
    }
    Ok(t)
}

// --- list_objects ----------------------------------------------------------

/// Shared request state, mutated in place by the builder methods.
#[derive(Clone)]
struct ListObjectsState {
    bucket: String,
    prefix: Option<String>,
    delimiter: Option<String>,
    start_after: Option<String>,
    max_keys: Option<i32>,
}

/// Builder for `ListObjectsV2`, finalized with `:send()`.
struct ListObjects {
    client: Client,
    state: Arc<Mutex<ListObjectsState>>,
}

impl UserData for ListObjects {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_function("prefix", |_, (ud, v): (AnyUserData, String)| {
            ud.borrow::<Self>()?.state.lock().unwrap().prefix = Some(v);
            Ok(ud)
        });

        // A delimiter turns the flat key space into one directory level:
        // `common_prefixes` gets the "subdirectories", `objects` the leaves.
        methods.add_function("delimiter", |_, (ud, v): (AnyUserData, String)| {
            ud.borrow::<Self>()?.state.lock().unwrap().delimiter = Some(v);
            Ok(ud)
        });

        methods.add_function("start_after", |_, (ud, v): (AnyUserData, String)| {
            ud.borrow::<Self>()?.state.lock().unwrap().start_after = Some(v);
            Ok(ud)
        });

        // A cap on total keys, for the "just show me some" case. Pagination is
        // otherwise followed to exhaustion.
        methods.add_function("max_keys", |_, (ud, v): (AnyUserData, i32)| {
            ud.borrow::<Self>()?.state.lock().unwrap().max_keys = Some(v);
            Ok(ud)
        });

        methods.add_async_function(
            "send",
            |lua, (ud, target): (AnyUserData, Value)| async move {
                let target = target_from_value(&target)?;
                let (client, state) = {
                    let this = ud.borrow::<Self>()?;
                    (this.client.clone(), this.state.lock().unwrap().clone())
                };

                let mut req = client.list_objects_v2().bucket(&state.bucket);
                if let Some(v) = &state.prefix {
                    req = req.prefix(v);
                }
                if let Some(v) = &state.delimiter {
                    req = req.delimiter(v);
                }
                if let Some(v) = &state.start_after {
                    req = req.start_after(v);
                }

                let objects = lua.create_table()?;
                let prefixes = lua.create_table()?;
                let mut n_objects = 0;
                let mut n_prefixes = 0;

                let mut stream = req.into_paginator().send();
                'pages: while let Some(page) = stream.next().await {
                    // A missing bucket raises: it is a bug in the script's
                    // input, not an answer. An empty prefix is already an empty
                    // result, which covers "nothing there".
                    let page = page.map_err(|e| lua_err(BoxError::from(e)))?;

                    for object in page.contents.unwrap_or_default() {
                        n_objects += 1;
                        objects.raw_set(n_objects, object_to_table(&lua, object)?)?;
                        // Checked here rather than via `max_keys` on the request:
                        // that caps each *page*, not the total.
                        if state.max_keys.is_some_and(|max| n_objects >= max) {
                            break 'pages;
                        }
                    }

                    for cp in page.common_prefixes.unwrap_or_default() {
                        if let Some(p) = cp.prefix {
                            n_prefixes += 1;
                            prefixes.raw_set(n_prefixes, p)?;
                        }
                    }
                }

                // Both keys are always present, so neither needs a nil guard.
                let data = lua.create_table()?;
                data.raw_set("objects", objects)?;
                data.raw_set("common_prefixes", prefixes)?;

                respond(&lua, true, Value::Table(data), target).await
            },
        );
    }
}

fn object_to_table(lua: &Lua, object: Object) -> LuaResult<Table> {
    let t = lua.create_table()?;
    if let Some(v) = object.key {
        t.raw_set("key", v)?;
    }
    if let Some(v) = object.size {
        t.raw_set("size", v)?;
    }
    if let Some(v) = object.e_tag {
        t.raw_set("etag", v)?;
    }
    if let Some(v) = object.storage_class {
        t.raw_set("storage_class", v.as_str().to_string())?;
    }
    if let Some(v) = object.last_modified.as_ref().and_then(date_to_string) {
        t.raw_set("last_modified", v)?;
    }
    Ok(t)
}

// --- list_object_versions --------------------------------------------------

/// Shared request state, mutated in place by the builder methods.
///
/// The markers are a *pair*: a key can hold more versions than one page, so
/// resuming needs both halves. They are exposed as one `:start_after(key[, id])`
/// rather than two methods, since a version id marker without its key marker is
/// meaningless to S3.
#[derive(Clone)]
struct ListObjectVersionsState {
    bucket: String,
    prefix: Option<String>,
    delimiter: Option<String>,
    key_marker: Option<String>,
    version_id_marker: Option<String>,
    max_keys: Option<i32>,
}

/// Builder for `ListObjectVersions`, finalized with `:send()`.
struct ListObjectVersions {
    client: Client,
    state: Arc<Mutex<ListObjectVersionsState>>,
}

impl UserData for ListObjectVersions {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_function("prefix", |_, (ud, v): (AnyUserData, String)| {
            ud.borrow::<Self>()?.state.lock().unwrap().prefix = Some(v);
            Ok(ud)
        });

        methods.add_function("delimiter", |_, (ud, v): (AnyUserData, String)| {
            ud.borrow::<Self>()?.state.lock().unwrap().delimiter = Some(v);
            Ok(ud)
        });

        // Both halves of S3's version cursor, the second optional: a key marker
        // alone resumes at the next key, and adding the version id marker
        // resumes mid-key.
        methods.add_function(
            "start_after",
            |_, (ud, key, version_id): (AnyUserData, String, Option<String>)| {
                {
                    let this = ud.borrow::<Self>()?;
                    let mut state = this.state.lock().unwrap();
                    state.key_marker = Some(key);
                    state.version_id_marker = version_id;
                }
                Ok(ud)
            },
        );

        // A cap on total entries, counting versions and delete markers together:
        // that is what S3's own `max-keys` bounds per page, and counting them
        // separately would make the limit depend on the mix.
        methods.add_function("max_keys", |_, (ud, v): (AnyUserData, i32)| {
            ud.borrow::<Self>()?.state.lock().unwrap().max_keys = Some(v);
            Ok(ud)
        });

        methods.add_async_function(
            "send",
            |lua, (ud, target): (AnyUserData, Value)| async move {
                let target = target_from_value(&target)?;
                let (client, state) = {
                    let this = ud.borrow::<Self>()?;
                    (this.client.clone(), this.state.lock().unwrap().clone())
                };

                let versions = lua.create_table()?;
                let delete_markers = lua.create_table()?;
                let prefixes = lua.create_table()?;
                let mut n_versions = 0;
                let mut n_markers = 0;
                let mut n_prefixes = 0;

                // Paged by hand: unlike `ListObjectsV2` this operation has no
                // SDK paginator, and its cursor is the two `next_*_marker`
                // fields fed back as the two `*_marker` inputs.
                let mut key_marker = state.key_marker.clone();
                let mut version_id_marker = state.version_id_marker.clone();
                'pages: loop {
                    let mut req = client.list_object_versions().bucket(&state.bucket);
                    if let Some(v) = &state.prefix {
                        req = req.prefix(v);
                    }
                    if let Some(v) = &state.delimiter {
                        req = req.delimiter(v);
                    }
                    if let Some(v) = &key_marker {
                        req = req.key_marker(v);
                    }
                    if let Some(v) = &version_id_marker {
                        req = req.version_id_marker(v);
                    }

                    // A missing bucket raises, as for every other listing: it is
                    // a bug in the script's input, not an answer.
                    let page = req.send().await.map_err(|e| lua_err(BoxError::from(e)))?;

                    for version in page.versions.unwrap_or_default() {
                        n_versions += 1;
                        versions.raw_set(n_versions, version_to_table(&lua, version)?)?;
                        if state
                            .max_keys
                            .is_some_and(|max| n_versions + n_markers >= max)
                        {
                            break 'pages;
                        }
                    }

                    for marker in page.delete_markers.unwrap_or_default() {
                        n_markers += 1;
                        delete_markers.raw_set(n_markers, delete_marker_to_table(&lua, marker)?)?;
                        if state
                            .max_keys
                            .is_some_and(|max| n_versions + n_markers >= max)
                        {
                            break 'pages;
                        }
                    }

                    for cp in page.common_prefixes.unwrap_or_default() {
                        if let Some(p) = cp.prefix {
                            n_prefixes += 1;
                            prefixes.raw_set(n_prefixes, p)?;
                        }
                    }

                    if page.is_truncated != Some(true) {
                        break;
                    }
                    // A truncated page always names its continuation; without one
                    // there is nothing to ask for, so stop rather than re-send
                    // the same request forever.
                    if page.next_key_marker.is_none() && page.next_version_id_marker.is_none() {
                        break;
                    }
                    key_marker = page.next_key_marker;
                    version_id_marker = page.next_version_id_marker;
                }

                // All three keys are always present, so none needs a nil guard.
                let data = lua.create_table()?;
                data.raw_set("versions", versions)?;
                data.raw_set("delete_markers", delete_markers)?;
                data.raw_set("common_prefixes", prefixes)?;

                respond(&lua, true, Value::Table(data), target).await
            },
        );
    }
}

fn version_to_table(lua: &Lua, version: ObjectVersion) -> LuaResult<Table> {
    let t = lua.create_table()?;
    if let Some(v) = version.key {
        t.raw_set("key", v)?;
    }
    if let Some(v) = version.version_id {
        t.raw_set("version_id", v)?;
    }
    // Always set: "is this the current version?" is the question this listing
    // exists to answer, so it must not be absent-means-false.
    t.raw_set("is_latest", version.is_latest.unwrap_or(false))?;
    if let Some(v) = version.size {
        t.raw_set("size", v)?;
    }
    if let Some(v) = version.e_tag {
        t.raw_set("etag", v)?;
    }
    if let Some(v) = version.storage_class {
        t.raw_set("storage_class", v.as_str().to_string())?;
    }
    if let Some(v) = version.last_modified.as_ref().and_then(date_to_string) {
        t.raw_set("last_modified", v)?;
    }
    Ok(t)
}

/// A delete marker is not an object: it has no size, etag or storage class, so
/// its table carries only the four fields S3 reports.
fn delete_marker_to_table(lua: &Lua, marker: DeleteMarkerEntry) -> LuaResult<Table> {
    let t = lua.create_table()?;
    if let Some(v) = marker.key {
        t.raw_set("key", v)?;
    }
    if let Some(v) = marker.version_id {
        t.raw_set("version_id", v)?;
    }
    t.raw_set("is_latest", marker.is_latest.unwrap_or(false))?;
    if let Some(v) = marker.last_modified.as_ref().and_then(date_to_string) {
        t.raw_set("last_modified", v)?;
    }
    Ok(t)
}

// --- head_object -----------------------------------------------------------

/// Builder for `HeadObject`, finalized with `:send()`.
struct HeadObject {
    client: Client,
    bucket: String,
    key: String,
    version_id: Arc<Mutex<Option<String>>>,
}

impl UserData for HeadObject {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_function("version_id", |_, (ud, v): (AnyUserData, String)| {
            *ud.borrow::<Self>()?.version_id.lock().unwrap() = Some(v);
            Ok(ud)
        });

        methods.add_async_function(
            "send",
            |lua, (ud, target): (AnyUserData, Value)| async move {
                let target = target_from_value(&target)?;
                let (client, bucket, key, version_id) = {
                    let this = ud.borrow::<Self>()?;
                    (
                        this.client.clone(),
                        this.bucket.clone(),
                        this.key.clone(),
                        this.version_id.lock().unwrap().clone(),
                    )
                };

                let mut req = client.head_object().bucket(bucket).key(key);
                if let Some(v) = version_id {
                    req = req.version_id(v);
                }

                let out = match req.send().await {
                    Ok(out) => out,
                    // HEAD has no response body, so S3 cannot send an error
                    // code; the SDK models it as a bare `NotFound`. That is a
                    // normal answer to "is it there?" — everything else raises.
                    Err(SdkError::ServiceError(e))
                        if matches!(e.err(), HeadObjectError::NotFound(_)) =>
                    {
                        return respond(&lua, false, Value::Nil, target).await;
                    }
                    Err(e) => return Err(lua_err(BoxError::from(e))),
                };

                let t = lua.create_table()?;
                if let Some(v) = out.content_length {
                    t.raw_set("content_length", v)?;
                }
                if let Some(v) = out.content_type {
                    t.raw_set("content_type", v)?;
                }
                if let Some(v) = out.e_tag {
                    t.raw_set("etag", v)?;
                }
                if let Some(v) = out.version_id {
                    t.raw_set("version_id", v)?;
                }
                if let Some(v) = out.storage_class {
                    t.raw_set("storage_class", v.as_str().to_string())?;
                }
                if let Some(v) = out.last_modified.as_ref().and_then(date_to_string) {
                    t.raw_set("last_modified", v)?;
                }

                // Always a table, possibly empty, so `.metadata.x` needs no guard.
                let metadata = lua.create_table()?;
                for (k, v) in out.metadata.unwrap_or_default() {
                    metadata.raw_set(k, v)?;
                }
                t.raw_set("metadata", metadata)?;

                respond(&lua, true, Value::Table(t), target).await
            },
        );
    }
}

// --- get_object ------------------------------------------------------------

/// Builder for `GetObject`, finalized with `:send([target])`.
struct GetObject {
    client: Client,
    bucket: String,
    key: String,
    version_id: Arc<Mutex<Option<String>>>,
    range: Arc<Mutex<Option<String>>>,
}

impl UserData for GetObject {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_function("version_id", |_, (ud, v): (AnyUserData, String)| {
            *ud.borrow::<Self>()?.version_id.lock().unwrap() = Some(v);
            Ok(ud)
        });

        // A raw HTTP range header (`bytes=0-1023`), the spelling S3 documents.
        methods.add_function("range", |_, (ud, v): (AnyUserData, String)| {
            *ud.borrow::<Self>()?.range.lock().unwrap() = Some(v);
            Ok(ud)
        });

        methods.add_async_function(
            "send",
            |lua, (ud, target): (AnyUserData, Value)| async move {
                let target = target_from_value(&target)?;
                let (client, bucket, key, version_id, range) = {
                    let this = ud.borrow::<Self>()?;
                    (
                        this.client.clone(),
                        this.bucket.clone(),
                        this.key.clone(),
                        this.version_id.lock().unwrap().clone(),
                        this.range.lock().unwrap().clone(),
                    )
                };

                let mut req = client.get_object().bucket(bucket).key(key);
                if let Some(v) = version_id {
                    req = req.version_id(v);
                }
                if let Some(v) = range {
                    req = req.range(v);
                }

                let out = match req.send().await {
                    Ok(out) => out,
                    // A missing key is a normal answer; a missing bucket, denied
                    // access or bad credential is not.
                    Err(SdkError::ServiceError(e))
                        if matches!(e.err(), GetObjectError::NoSuchKey(_)) =>
                    {
                        let env = envelope(&lua, false)?;
                        return Ok(MultiValue::from_vec(vec![Value::Table(env), Value::Nil]));
                    }
                    Err(e) => return Err(lua_err(BoxError::from(e))),
                };

                let env = envelope(&lua, true)?;
                if let Some(v) = &out.content_type {
                    env.raw_set("content_type", v.clone())?;
                }
                if let Some(v) = out.content_length {
                    env.raw_set("content_length", v)?;
                }
                if let Some(v) = &out.e_tag {
                    env.raw_set("etag", v.clone())?;
                }
                if let Some(v) = &out.version_id {
                    env.raw_set("version_id", v.clone())?;
                }
                if let Some(v) = out.last_modified.as_ref().and_then(date_to_string) {
                    env.raw_set("last_modified", v)?;
                }

                let content_type = out.content_type.clone();
                let mut body = out.body;

                let second = match target {
                    // No target: the body comes back as data — parsed when the
                    // reported content type says it is a format we can parse,
                    // otherwise the raw string.
                    None => {
                        let bytes = body
                            .collect()
                            .await
                            .map_err(|e| lua_err(BoxError::from(e)))?
                            .into_bytes();
                        parse_body(&lua, content_type.as_deref(), &bytes)?
                    }
                    Some(Redirect::Capture { kind, .. }) => {
                        let bytes = body
                            .collect()
                            .await
                            .map_err(|e| lua_err(BoxError::from(e)))?
                            .into_bytes();
                        capture_to_value(&lua, kind, &bytes)?
                    }
                    Some(Redirect::Null) => {
                        // Still drained, so an error mid-body is reported rather
                        // than silently dropped.
                        while let Some(chunk) = body.next().await {
                            chunk.map_err(|e| lua_err(BoxError::from(e)))?;
                        }
                        Value::Nil
                    }
                    // Written chunk by chunk as it arrives, so a large object
                    // never has to fit in memory.
                    Some(Redirect::File { path, file }) => {
                        let mut handle = Some(open_shared_file(&path, &file)?);
                        while let Some(chunk) = body.next().await {
                            let chunk = chunk.map_err(|e| lua_err(BoxError::from(e)))?;
                            let f = handle.take().expect("handle round-trips each chunk");
                            handle = Some(write_all_blocking(f, chunk.to_vec(), &path).await?);
                        }
                        Value::String(lua.create_string(path.to_string_lossy().as_bytes())?)
                    }
                    Some(Redirect::Inherit(_) | Redirect::SameAsOther) => {
                        unreachable!("rejected by target_from_value")
                    }
                };

                Ok(MultiValue::from_vec(vec![Value::Table(env), second]))
            },
        );
    }
}

/// Turn a body into Luau data, using the content type S3 reported.
///
/// Parsing is best-effort by design: a body whose type claims JSON but does not
/// parse comes back as the string, so a fetch never fails over a header the
/// uploader controls.
fn parse_body(lua: &Lua, content_type: Option<&str>, bytes: &[u8]) -> LuaResult<Value> {
    // The type may carry parameters (`application/json; charset=utf-8`).
    let base = content_type
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();

    let text = std::str::from_utf8(bytes).ok();
    if let Some(text) = text {
        let parsed = match base.as_str() {
            "application/json" | "text/json" => serde_json::from_str::<serde_json::Value>(text)
                .ok()
                .and_then(|v| lua.to_value(&v).ok()),
            "application/yaml" | "text/yaml" | "application/x-yaml" | "text/x-yaml" => {
                yamlito::parse(text)
                    .ok()
                    .and_then(|tree| yamlito::Yaml::from_tree(&tree).ok())
                    .and_then(|v| lua.to_value(&v).ok())
            }
            "application/toml" | "text/toml" => text
                .parse::<::toml::Value>()
                .ok()
                .and_then(|v| lua.to_value(&v).ok()),
            _ => None,
        };
        if let Some(value) = parsed {
            return Ok(value);
        }
    }

    // Not a parseable type, not valid UTF-8, or malformed: hand back the bytes.
    // A Luau string is 8-bit clean, so binary survives intact.
    Ok(Value::String(lua.create_string(bytes)?))
}

// --- tagging ---------------------------------------------------------------

/// Turn S3's `TagSet` into a `{[key]=value}` table.
///
/// Always a table, possibly empty, so `.tags.Env` needs no guard — the same
/// judgement `head_object`'s `metadata` makes. A tag with no key or no value
/// cannot exist in S3; one is skipped rather than invented.
fn tag_set_to_table(lua: &Lua, tags: &[Tag]) -> LuaResult<Table> {
    let t = lua.create_table()?;
    for tag in tags {
        t.raw_set(tag.key(), tag.value())?;
    }
    Ok(t)
}

/// Turn a `{[key]=value}` table into S3's `Tagging` document.
///
/// Ordered by key: S3 does not care, but a stable order makes a request
/// reproducible and a test assertable. Non-string keys and values are rejected
/// by name, since a tag is a string pair on the wire and a silent `tostring`
/// would put `"1"` on a bucket where the script wrote `1`.
fn table_to_tagging(tags: &Table) -> LuaResult<Tagging> {
    let mut pairs: BTreeMap<String, String> = BTreeMap::new();
    for entry in tags.pairs::<Value, Value>() {
        let (k, v) = entry?;
        let (Value::String(k), Value::String(v)) = (&k, &v) else {
            return Err(mlua::Error::runtime(format!(
                "tags must be a table of string keys to string values, got {} = {}",
                k.type_name(),
                v.type_name()
            )));
        };
        pairs.insert(k.to_str()?.to_owned(), v.to_str()?.to_owned());
    }

    let mut set = Vec::with_capacity(pairs.len());
    for (k, v) in pairs {
        set.push(
            Tag::builder()
                .key(k)
                .value(v)
                .build()
                .map_err(|e| mlua::Error::runtime(format!("invalid tag: {e}")))?,
        );
    }
    Tagging::builder()
        .set_tag_set(Some(set))
        .build()
        .map_err(|e| mlua::Error::runtime(format!("invalid tag set: {e}")))
}

/// Whether an SDK error is S3 reporting that a bucket simply carries no tags.
///
/// `GetBucketTagging` answers an untagged bucket with a service error rather
/// than an empty document, and the SDK models it as unhandled — no variant to
/// match, so the error code is the only thing there is. It is a normal answer,
/// not a failure: an untagged bucket is a bucket.
fn is_no_such_tag_set(err: &impl ProvideErrorMetadata) -> bool {
    err.code() == Some("NoSuchTagSet")
}

/// Reject an empty bucket name or object key here rather than letting S3 answer
/// with a signing or 400 error the script author has to decode.
fn require_non_empty(what: &str, value: &str) -> LuaResult<()> {
    if value.is_empty() {
        return Err(mlua::Error::runtime(format!("{what} must not be empty")));
    }
    Ok(())
}

/// `(envelope, data)` for the tagging operations.
///
/// Tagging has no body to stream — the result is a handful of string pairs — so
/// `:send()` takes no target and this needs none of `respond`'s delivery
/// machinery.
fn tagging_result(env: Table, data: Value) -> LuaResult<MultiValue> {
    Ok(MultiValue::from_vec(vec![Value::Table(env), data]))
}

/// Builder for `GetObjectTagging`, finalized with `:send()`.
struct GetObjectTagging {
    client: Client,
    bucket: String,
    key: String,
    version_id: Arc<Mutex<Option<String>>>,
}

impl UserData for GetObjectTagging {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_function("version_id", |_, (ud, v): (AnyUserData, String)| {
            *ud.borrow::<Self>()?.version_id.lock().unwrap() = Some(v);
            Ok(ud)
        });

        methods.add_async_function("send", |lua, ud: AnyUserData| async move {
            let (client, bucket, key, version_id) = {
                let this = ud.borrow::<Self>()?;
                (
                    this.client.clone(),
                    this.bucket.clone(),
                    this.key.clone(),
                    this.version_id.lock().unwrap().clone(),
                )
            };

            let mut req = client.get_object_tagging().bucket(bucket).key(key);
            if let Some(v) = version_id {
                req = req.version_id(v);
            }

            // A missing key or bucket raises: unlike `head_object`, this is not
            // an existence question, so there is no false `ok` to report.
            let out = req.send().await.map_err(|e| lua_err(BoxError::from(e)))?;

            let env = envelope(&lua, true)?;
            if let Some(v) = &out.version_id {
                env.raw_set("version_id", v.clone())?;
            }
            let tags = tag_set_to_table(&lua, out.tag_set())?;
            tagging_result(env, Value::Table(tags))
        });
    }
}

/// Builder for `PutObjectTagging`, finalized with `:send()`.
struct PutObjectTagging {
    client: Client,
    bucket: String,
    key: String,
    version_id: Arc<Mutex<Option<String>>>,
    tags: Arc<Mutex<Option<Tagging>>>,
}

impl UserData for PutObjectTagging {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_function("version_id", |_, (ud, v): (AnyUserData, String)| {
            *ud.borrow::<Self>()?.version_id.lock().unwrap() = Some(v);
            Ok(ud)
        });

        // Converted here rather than at `:send()`, so a malformed table raises
        // at the call that wrote it.
        methods.add_function("tags", |_, (ud, tags): (AnyUserData, Table)| {
            let tagging = table_to_tagging(&tags)?;
            *ud.borrow::<Self>()?.tags.lock().unwrap() = Some(tagging);
            Ok(ud)
        });

        methods.add_async_function("send", |lua, ud: AnyUserData| async move {
            let (client, bucket, key, version_id, tags) = {
                let this = ud.borrow::<Self>()?;
                (
                    this.client.clone(),
                    this.bucket.clone(),
                    this.key.clone(),
                    this.version_id.lock().unwrap().clone(),
                    this.tags.lock().unwrap().clone(),
                )
            };

            // A put replaces the whole set, so an omitted `:tags()` would clear
            // it. Refuse rather than do that by accident: erasing is
            // `delete_object_tagging`, which says so.
            let Some(tags) = tags else {
                return Err(mlua::Error::runtime(
                    "put_object_tagging requires :tags(t) — a put replaces the \
                     entire tag set; use delete_object_tagging to clear it",
                ));
            };

            let mut req = client
                .put_object_tagging()
                .bucket(bucket)
                .key(key)
                .tagging(tags);
            if let Some(v) = version_id {
                req = req.version_id(v);
            }

            let out = req.send().await.map_err(|e| lua_err(BoxError::from(e)))?;

            let env = envelope(&lua, true)?;
            // S3 names the version the tags landed on, which on a versioned
            // bucket is the only way to know which one that was.
            if let Some(v) = &out.version_id {
                env.raw_set("version_id", v.clone())?;
            }
            tagging_result(env, Value::Nil)
        });
    }
}

/// Builder for `DeleteObjectTagging`, finalized with `:send()`.
struct DeleteObjectTagging {
    client: Client,
    bucket: String,
    key: String,
    version_id: Arc<Mutex<Option<String>>>,
}

impl UserData for DeleteObjectTagging {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_function("version_id", |_, (ud, v): (AnyUserData, String)| {
            *ud.borrow::<Self>()?.version_id.lock().unwrap() = Some(v);
            Ok(ud)
        });

        methods.add_async_function("send", |lua, ud: AnyUserData| async move {
            let (client, bucket, key, version_id) = {
                let this = ud.borrow::<Self>()?;
                (
                    this.client.clone(),
                    this.bucket.clone(),
                    this.key.clone(),
                    this.version_id.lock().unwrap().clone(),
                )
            };

            let mut req = client.delete_object_tagging().bucket(bucket).key(key);
            if let Some(v) = version_id {
                req = req.version_id(v);
            }

            let out = req.send().await.map_err(|e| lua_err(BoxError::from(e)))?;

            let env = envelope(&lua, true)?;
            if let Some(v) = &out.version_id {
                env.raw_set("version_id", v.clone())?;
            }
            tagging_result(env, Value::Nil)
        });
    }
}

/// Builder for `GetBucketTagging`, finalized with `:send()`.
struct GetBucketTagging {
    client: Client,
    bucket: String,
}

impl UserData for GetBucketTagging {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_async_function("send", |lua, ud: AnyUserData| async move {
            let (client, bucket) = {
                let this = ud.borrow::<Self>()?;
                (this.client.clone(), this.bucket.clone())
            };

            let tags = match client.get_bucket_tagging().bucket(bucket).send().await {
                Ok(out) => tag_set_to_table(&lua, out.tag_set())?,
                // "No tags" is an answer, and S3 spells it as an error. A
                // missing bucket, denied access or bad credential is not, and
                // still raises.
                Err(SdkError::ServiceError(e)) if is_no_such_tag_set(e.err()) => {
                    lua.create_table()?
                }
                Err(e) => return Err(lua_err(BoxError::from(e))),
            };

            // `ok` is true either way: reading "no tags" succeeded.
            let env = envelope(&lua, true)?;
            tagging_result(env, Value::Table(tags))
        });
    }
}

/// Builder for `PutBucketTagging`, finalized with `:send()`.
struct PutBucketTagging {
    client: Client,
    bucket: String,
    tags: Arc<Mutex<Option<Tagging>>>,
}

impl UserData for PutBucketTagging {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_function("tags", |_, (ud, tags): (AnyUserData, Table)| {
            let tagging = table_to_tagging(&tags)?;
            *ud.borrow::<Self>()?.tags.lock().unwrap() = Some(tagging);
            Ok(ud)
        });

        methods.add_async_function("send", |lua, ud: AnyUserData| async move {
            let (client, bucket, tags) = {
                let this = ud.borrow::<Self>()?;
                (
                    this.client.clone(),
                    this.bucket.clone(),
                    this.tags.lock().unwrap().clone(),
                )
            };

            let Some(tags) = tags else {
                return Err(mlua::Error::runtime(
                    "put_bucket_tagging requires :tags(t) — a put replaces the \
                     entire tag set; use delete_bucket_tagging to clear it",
                ));
            };

            client
                .put_bucket_tagging()
                .bucket(bucket)
                .tagging(tags)
                .send()
                .await
                .map_err(|e| lua_err(BoxError::from(e)))?;

            let env = envelope(&lua, true)?;
            tagging_result(env, Value::Nil)
        });
    }
}

/// Builder for `DeleteBucketTagging`, finalized with `:send()`.
struct DeleteBucketTagging {
    client: Client,
    bucket: String,
}

impl UserData for DeleteBucketTagging {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_async_function("send", |lua, ud: AnyUserData| async move {
            let (client, bucket) = {
                let this = ud.borrow::<Self>()?;
                (this.client.clone(), this.bucket.clone())
            };

            client
                .delete_bucket_tagging()
                .bucket(bucket)
                .send()
                .await
                .map_err(|e| lua_err(BoxError::from(e)))?;

            let env = envelope(&lua, true)?;
            tagging_result(env, Value::Nil)
        });
    }
}

// --- module ----------------------------------------------------------------

/// Build the `aws.s3` module table over `source`.
///
/// Every entry point shares the one source, so a script making several calls
/// resolves credentials once. Called for the `aws.s3` global (ambient
/// configuration) and again per `aws.config():build()`, so the surface cannot
/// differ between them.
pub(crate) fn module(lua: &Lua, source: ClientSource) -> LuaResult<Table> {
    let exports = lua.create_table()?;

    let c = source.clone();
    exports.raw_set(
        "list_buckets",
        lua.create_async_function(move |_, ()| {
            let c = c.clone();
            async move {
                Ok(ListBuckets {
                    client: c.s3().await,
                })
            }
        })?,
    )?;

    let c = source.clone();
    exports.raw_set(
        "list_objects",
        lua.create_async_function(move |_, bucket: String| {
            let c = c.clone();
            async move {
                Ok(ListObjects {
                    client: c.s3().await,
                    state: Arc::new(Mutex::new(ListObjectsState {
                        bucket,
                        prefix: None,
                        delimiter: None,
                        start_after: None,
                        max_keys: None,
                    })),
                })
            }
        })?,
    )?;

    let c = source.clone();
    exports.raw_set(
        "list_object_versions",
        lua.create_async_function(move |_, bucket: String| {
            let c = c.clone();
            async move {
                Ok(ListObjectVersions {
                    client: c.s3().await,
                    state: Arc::new(Mutex::new(ListObjectVersionsState {
                        bucket,
                        prefix: None,
                        delimiter: None,
                        key_marker: None,
                        version_id_marker: None,
                        max_keys: None,
                    })),
                })
            }
        })?,
    )?;

    let c = source.clone();
    exports.raw_set(
        "head_object",
        lua.create_async_function(move |_, (bucket, key): (String, String)| {
            let c = c.clone();
            async move {
                Ok(HeadObject {
                    client: c.s3().await,
                    bucket,
                    key,
                    version_id: Arc::new(Mutex::new(None)),
                })
            }
        })?,
    )?;

    let c = source.clone();
    exports.raw_set(
        "get_object",
        lua.create_async_function(move |_, (bucket, key): (String, String)| {
            let c = c.clone();
            async move {
                Ok(GetObject {
                    client: c.s3().await,
                    bucket,
                    key,
                    version_id: Arc::new(Mutex::new(None)),
                    range: Arc::new(Mutex::new(None)),
                })
            }
        })?,
    )?;

    // Tagging. The object forms take `(bucket, key)` and the bucket forms take
    // `bucket` — positional, since at most two arguments in that order cannot
    // be read the wrong way round.
    let c = source.clone();
    exports.raw_set(
        "get_object_tagging",
        lua.create_async_function(move |_, (bucket, key): (String, String)| {
            let c = c.clone();
            async move {
                require_non_empty("bucket", &bucket)?;
                require_non_empty("key", &key)?;
                Ok(GetObjectTagging {
                    client: c.s3().await,
                    bucket,
                    key,
                    version_id: Arc::new(Mutex::new(None)),
                })
            }
        })?,
    )?;

    let c = source.clone();
    exports.raw_set(
        "put_object_tagging",
        lua.create_async_function(move |_, (bucket, key): (String, String)| {
            let c = c.clone();
            async move {
                require_non_empty("bucket", &bucket)?;
                require_non_empty("key", &key)?;
                Ok(PutObjectTagging {
                    client: c.s3().await,
                    bucket,
                    key,
                    version_id: Arc::new(Mutex::new(None)),
                    tags: Arc::new(Mutex::new(None)),
                })
            }
        })?,
    )?;

    let c = source.clone();
    exports.raw_set(
        "delete_object_tagging",
        lua.create_async_function(move |_, (bucket, key): (String, String)| {
            let c = c.clone();
            async move {
                require_non_empty("bucket", &bucket)?;
                require_non_empty("key", &key)?;
                Ok(DeleteObjectTagging {
                    client: c.s3().await,
                    bucket,
                    key,
                    version_id: Arc::new(Mutex::new(None)),
                })
            }
        })?,
    )?;

    let c = source.clone();
    exports.raw_set(
        "get_bucket_tagging",
        lua.create_async_function(move |_, bucket: String| {
            let c = c.clone();
            async move {
                require_non_empty("bucket", &bucket)?;
                Ok(GetBucketTagging {
                    client: c.s3().await,
                    bucket,
                })
            }
        })?,
    )?;

    let c = source.clone();
    exports.raw_set(
        "put_bucket_tagging",
        lua.create_async_function(move |_, bucket: String| {
            let c = c.clone();
            async move {
                require_non_empty("bucket", &bucket)?;
                Ok(PutBucketTagging {
                    client: c.s3().await,
                    bucket,
                    tags: Arc::new(Mutex::new(None)),
                })
            }
        })?,
    )?;

    exports.raw_set(
        "delete_bucket_tagging",
        lua.create_async_function(move |_, bucket: String| {
            let c = source.clone();
            async move {
                require_non_empty("bucket", &bucket)?;
                Ok(DeleteBucketTagging {
                    client: c.s3().await,
                    bucket,
                })
            }
        })?,
    )?;

    Ok(exports)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripting::redirect::InheritTarget;
    use aws_sdk_s3::types::{ObjectStorageClass, ObjectVersionStorageClass};

    #[test]
    fn converts_object_fields() {
        let lua = Lua::new();
        let object = Object::builder()
            .key("logs/a.txt")
            .size(1234)
            .e_tag("\"abc\"")
            .storage_class(ObjectStorageClass::Standard)
            .last_modified(aws_smithy_types::DateTime::from_secs(1_700_000_000))
            .build();

        let t = object_to_table(&lua, object).unwrap();

        assert_eq!(t.get::<String>("key").unwrap(), "logs/a.txt");
        assert_eq!(t.get::<i64>("size").unwrap(), 1234);
        assert_eq!(t.get::<String>("etag").unwrap(), "\"abc\"");
        assert_eq!(t.get::<String>("storage_class").unwrap(), "STANDARD");
        assert_eq!(
            t.get::<String>("last_modified").unwrap(),
            "2023-11-14T22:13:20Z"
        );
    }

    /// Absent scalars are omitted rather than set to nil.
    #[test]
    fn absent_object_fields_are_omitted() {
        let lua = Lua::new();
        let t = object_to_table(&lua, Object::builder().key("k").build()).unwrap();

        assert_eq!(t.get::<String>("key").unwrap(), "k");
        assert!(t.get::<Value>("size").unwrap().is_nil());
        assert!(t.get::<Value>("etag").unwrap().is_nil());
    }

    #[test]
    fn converts_version_fields() {
        let lua = Lua::new();
        let version = ObjectVersion::builder()
            .key("conf.json")
            .version_id("3sL4kqtJlcpXroDTDmJ+rmSpXd3dIbrHY+MTRCxf3vjVBH40Nr8X8gdRQBpUMLUo")
            .is_latest(true)
            .size(42)
            .e_tag("\"abc\"")
            .storage_class(ObjectVersionStorageClass::Standard)
            .last_modified(aws_smithy_types::DateTime::from_secs(1_700_000_000))
            .build();

        let t = version_to_table(&lua, version).unwrap();

        assert_eq!(t.get::<String>("key").unwrap(), "conf.json");
        assert_eq!(
            t.get::<String>("version_id").unwrap(),
            "3sL4kqtJlcpXroDTDmJ+rmSpXd3dIbrHY+MTRCxf3vjVBH40Nr8X8gdRQBpUMLUo"
        );
        assert!(t.get::<bool>("is_latest").unwrap());
        assert_eq!(t.get::<i64>("size").unwrap(), 42);
        assert_eq!(t.get::<String>("etag").unwrap(), "\"abc\"");
        assert_eq!(t.get::<String>("storage_class").unwrap(), "STANDARD");
        assert_eq!(
            t.get::<String>("last_modified").unwrap(),
            "2023-11-14T22:13:20Z"
        );
    }

    /// `is_latest` is the question this listing exists to answer, so an absent
    /// flag must read as `false` rather than as nil.
    #[test]
    fn absent_is_latest_reads_as_false() {
        let lua = Lua::new();
        let t = version_to_table(&lua, ObjectVersion::builder().key("k").build()).unwrap();
        assert!(!t.get::<bool>("is_latest").unwrap());

        let t =
            delete_marker_to_table(&lua, DeleteMarkerEntry::builder().key("k").build()).unwrap();
        assert!(!t.get::<bool>("is_latest").unwrap());
    }

    /// A delete marker is not an object: no size, etag or storage class.
    #[test]
    fn converts_delete_marker_fields() {
        let lua = Lua::new();
        let marker = DeleteMarkerEntry::builder()
            .key("gone.txt")
            .version_id("v9")
            .is_latest(true)
            .last_modified(aws_smithy_types::DateTime::from_secs(1_700_000_000))
            .build();

        let t = delete_marker_to_table(&lua, marker).unwrap();

        assert_eq!(t.get::<String>("key").unwrap(), "gone.txt");
        assert_eq!(t.get::<String>("version_id").unwrap(), "v9");
        assert!(t.get::<bool>("is_latest").unwrap());
        assert_eq!(
            t.get::<String>("last_modified").unwrap(),
            "2023-11-14T22:13:20Z"
        );
        assert!(t.get::<Value>("size").unwrap().is_nil());
        assert!(t.get::<Value>("etag").unwrap().is_nil());
        assert!(t.get::<Value>("storage_class").unwrap().is_nil());
    }

    #[test]
    fn converts_bucket_fields() {
        let lua = Lua::new();
        let bucket = Bucket::builder()
            .name("my-bucket")
            .bucket_region("eu-central-1")
            .creation_date(aws_smithy_types::DateTime::from_secs(1_600_000_000))
            .build();

        let t = bucket_to_table(&lua, bucket).unwrap();

        assert_eq!(t.get::<String>("name").unwrap(), "my-bucket");
        assert_eq!(t.get::<String>("region").unwrap(), "eu-central-1");
        assert_eq!(
            t.get::<String>("creation_date").unwrap(),
            "2020-09-13T12:26:40Z"
        );
    }

    #[test]
    fn parses_json_body_per_content_type() {
        let lua = Lua::new();
        let v = parse_body(&lua, Some("application/json"), br#"{"a":1}"#).unwrap();
        let t = match v {
            Value::Table(t) => t,
            other => panic!("expected a table, got {}", other.type_name()),
        };
        assert_eq!(t.get::<i64>("a").unwrap(), 1);
    }

    /// Content-type parameters must not defeat the match.
    #[test]
    fn parses_json_with_charset_parameter() {
        let lua = Lua::new();
        let v = parse_body(&lua, Some("application/json; charset=utf-8"), br#"{"a":2}"#).unwrap();
        assert!(matches!(v, Value::Table(_)));
    }

    /// A body whose type lies must come back as a string rather than fail.
    #[test]
    fn malformed_json_falls_back_to_string() {
        let lua = Lua::new();
        let v = parse_body(&lua, Some("application/json"), b"not json at all").unwrap();
        match v {
            Value::String(s) => assert_eq!(s.to_str().unwrap().to_owned(), "not json at all"),
            other => panic!("expected a string, got {}", other.type_name()),
        }
    }

    #[test]
    fn unknown_content_type_stays_a_string() {
        let lua = Lua::new();
        let v = parse_body(&lua, Some("text/plain"), b"hello").unwrap();
        assert!(matches!(v, Value::String(_)));
    }

    /// Binary bodies survive: a Luau string is 8-bit clean.
    #[test]
    fn binary_body_survives_intact() {
        let lua = Lua::new();
        let bytes = [0u8, 159, 146, 150];
        let v = parse_body(&lua, Some("application/octet-stream"), &bytes).unwrap();
        match v {
            Value::String(s) => assert_eq!(s.as_bytes().to_vec(), bytes),
            other => panic!("expected a string, got {}", other.type_name()),
        }
    }

    /// A non-userdata target names the accepted values, since the failure is a
    /// script-author mistake rather than an AWS one.
    #[test]
    fn rejects_non_redirect_target() {
        let msg = match target_from_value(&Value::Integer(1)) {
            Err(e) => e.to_string(),
            Ok(_) => panic!("an integer is not a target"),
        };
        assert!(msg.contains("expected a body target"), "{msg}");
        assert!(msg.contains("proc.file(path)"), "{msg}");
    }

    /// A missing argument is no target, not an error.
    #[test]
    fn nil_target_is_none() {
        assert!(target_from_value(&Value::Nil).unwrap().is_none());
    }

    /// `INHERIT` would write a body to stdout, which a report owning stdout must
    /// not be interleaved with.
    #[test]
    fn rejects_inherit_and_stream_aliases() {
        let lua = Lua::new();
        // Every inherit target, named or slot-matched: all three write to one of
        // this process's streams, which is the thing being rejected.
        for sentinel in [
            Redirect::Inherit(InheritTarget::Same),
            Redirect::Inherit(InheritTarget::Stdout),
            Redirect::Inherit(InheritTarget::Stderr),
            Redirect::SameAsOther,
        ] {
            let ud = lua.create_userdata(sentinel).unwrap();
            match target_from_value(&Value::UserData(ud)) {
                Err(e) => assert!(e.to_string().contains("expected a body target")),
                Ok(_) => panic!("this sentinel is not a body target"),
            }
        }
    }

    /// The runtime shape module tests need: current-thread plus a `LocalSet`,
    /// since the delivery paths are async and `spawn_blocking` needs a reactor.
    fn block_on<F: std::future::Future>(f: F) -> F::Output {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let local = tokio::task::LocalSet::new();
        local.block_on(&rt, f)
    }

    #[test]
    fn capture_target_returns_the_bytes() {
        let lua = Lua::new();
        let target = Redirect::Capture {
            kind: crate::scripting::redirect::CaptureKind::String,
            id: 0,
        };
        let v = block_on(deliver_bytes(&lua, &target, b"payload")).unwrap();
        match v {
            Value::String(s) => assert_eq!(s.to_str().unwrap().to_owned(), "payload"),
            other => panic!("expected a string, got {}", other.type_name()),
        }
    }

    #[test]
    fn file_target_writes_and_returns_the_path() {
        let lua = Lua::new();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.json");
        let target = Redirect::File {
            path: path.clone(),
            file: Arc::new(Mutex::new(None)),
        };

        let v = block_on(deliver_bytes(&lua, &target, b"{\"a\":1}")).unwrap();

        // The second return value is the path, not a handle.
        match v {
            Value::String(s) => assert_eq!(s.to_str().unwrap().to_owned(), path.to_str().unwrap()),
            other => panic!("expected a string, got {}", other.type_name()),
        }
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{\"a\":1}");
    }

    /// A discard yields no second value, but must not fail.
    #[test]
    fn null_target_returns_nil() {
        let lua = Lua::new();
        let v = block_on(deliver_bytes(&lua, &Redirect::Null, b"ignored")).unwrap();
        assert!(v.is_nil());
    }

    /// A not-found must leave a file target untouched rather than creating it
    /// and writing `null` into it.
    #[test]
    fn not_found_does_not_write_to_a_target() {
        let lua = Lua::new();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("absent.json");
        let target = Redirect::File {
            path: path.clone(),
            file: Arc::new(Mutex::new(None)),
        };

        let values = block_on(respond(&lua, false, Value::Nil, Some(target))).unwrap();

        let mut iter = values.into_iter();
        let env = match iter.next() {
            Some(Value::Table(t)) => t,
            _ => panic!("first value must be the envelope table"),
        };
        assert!(!env.get::<bool>("ok").unwrap());
        assert!(iter.next().is_none_or(|v| v.is_nil()));
        assert!(!path.exists(), "a not-found must not create the file");
    }

    /// With a target, a list response is re-serialized as JSON — the wire format
    /// is XML, which the SDK has already parsed by this point.
    #[test]
    fn list_data_is_reserialized_for_a_target() {
        let lua = Lua::new();
        let data = lua.create_table().unwrap();
        data.raw_set("objects", lua.create_table().unwrap())
            .unwrap();

        let values = block_on(respond(
            &lua,
            true,
            Value::Table(data),
            Some(Redirect::Capture {
                kind: crate::scripting::redirect::CaptureKind::String,
                id: 0,
            }),
        ))
        .unwrap();

        let second = values.into_iter().nth(1).expect("a second return value");
        match second {
            Value::String(s) => {
                let text = s.to_str().unwrap().to_owned();
                assert!(text.contains("objects"), "{text}");
            }
            other => panic!("expected a string, got {}", other.type_name()),
        }
    }

    #[test]
    fn converts_tag_set_to_a_map() {
        let lua = Lua::new();
        let tags = [
            Tag::builder().key("Env").value("prod").build().unwrap(),
            Tag::builder().key("Owner").value("infra").build().unwrap(),
        ];

        let t = tag_set_to_table(&lua, &tags).unwrap();

        assert_eq!(t.get::<String>("Env").unwrap(), "prod");
        assert_eq!(t.get::<String>("Owner").unwrap(), "infra");
    }

    /// An empty tag set is a table, not nil, so `.tags.Env` needs no guard.
    #[test]
    fn empty_tag_set_is_still_a_table() {
        let lua = Lua::new();
        let t = tag_set_to_table(&lua, &[]).unwrap();
        assert_eq!(t.len().unwrap(), 0);
        assert!(t.get::<Value>("Env").unwrap().is_nil());
    }

    /// An empty *value* is a legal tag: S3 allows it, so it must round-trip
    /// rather than be dropped as "absent".
    #[test]
    fn empty_tag_value_survives() {
        let lua = Lua::new();
        let tags = [Tag::builder().key("Marker").value("").build().unwrap()];
        let t = tag_set_to_table(&lua, &tags).unwrap();
        assert_eq!(t.get::<String>("Marker").unwrap(), "");
    }

    #[test]
    fn converts_a_map_to_a_tagging_document() {
        let lua = Lua::new();
        let tags = lua.create_table().unwrap();
        tags.raw_set("Owner", "infra").unwrap();
        tags.raw_set("Env", "prod").unwrap();

        let tagging = table_to_tagging(&tags).unwrap();

        // Sorted by key, so a request is reproducible.
        let pairs: Vec<(&str, &str)> = tagging
            .tag_set()
            .iter()
            .map(|t| (t.key(), t.value()))
            .collect();
        assert_eq!(pairs, vec![("Env", "prod"), ("Owner", "infra")]);
    }

    /// An empty table is a legal put: it is the "replace with nothing" form.
    #[test]
    fn empty_map_converts_to_an_empty_tagging() {
        let lua = Lua::new();
        let tagging = table_to_tagging(&lua.create_table().unwrap()).unwrap();
        assert!(tagging.tag_set().is_empty());
    }

    /// A tag is a string pair on the wire; coercing silently would put `"1"` on
    /// a bucket where the script wrote `1`.
    #[test]
    fn rejects_non_string_tag_values() {
        let lua = Lua::new();
        let tags = lua.create_table().unwrap();
        tags.raw_set("Count", 1).unwrap();

        let msg = match table_to_tagging(&tags) {
            Err(e) => e.to_string(),
            Ok(_) => panic!("a number is not a tag value"),
        };
        assert!(msg.contains("string keys to string values"), "{msg}");
    }

    /// The read side's shape is the write side's shape, so read/modify/write
    /// needs no reshaping.
    #[test]
    fn tag_conversion_round_trips() {
        let lua = Lua::new();
        let original = [
            Tag::builder().key("Env").value("prod").build().unwrap(),
            Tag::builder().key("Team").value("infra").build().unwrap(),
        ];

        let t = tag_set_to_table(&lua, &original).unwrap();
        let back = table_to_tagging(&t).unwrap();

        let pairs: Vec<(&str, &str)> = back
            .tag_set()
            .iter()
            .map(|t| (t.key(), t.value()))
            .collect();
        assert_eq!(pairs, vec![("Env", "prod"), ("Team", "infra")]);
    }

    /// The `NoSuchTagSet` code is matched by name: the SDK models it as an
    /// unhandled error, so there is no variant to match on.
    #[test]
    fn recognises_the_no_such_tag_set_code() {
        use aws_smithy_types::error::ErrorMetadata;

        let no_tags = ErrorMetadata::builder().code("NoSuchTagSet").build();
        assert!(is_no_such_tag_set(&no_tags));

        // A denied read is not "no tags": it must still raise.
        let denied = ErrorMetadata::builder().code("AccessDenied").build();
        assert!(!is_no_such_tag_set(&denied));

        let unknown = ErrorMetadata::builder().build();
        assert!(!is_no_such_tag_set(&unknown));
    }

    /// An empty bucket or key is caught here rather than becoming a 400 the
    /// script author has to decode.
    #[test]
    fn rejects_empty_bucket_and_key() {
        let msg = match require_non_empty("bucket", "") {
            Err(e) => e.to_string(),
            Ok(()) => panic!("an empty bucket is not usable"),
        };
        assert!(msg.contains("bucket must not be empty"), "{msg}");
        assert!(require_non_empty("key", "k").is_ok());
    }

    /// Without a target the data comes back as-is, not serialized.
    #[test]
    fn no_target_returns_the_data_itself() {
        let lua = Lua::new();
        let data = lua.create_table().unwrap();
        data.raw_set("name", "b").unwrap();

        let values = block_on(respond(&lua, true, Value::Table(data), None)).unwrap();

        match values.into_iter().nth(1) {
            Some(Value::Table(t)) => assert_eq!(t.get::<String>("name").unwrap(), "b"),
            _ => panic!("expected the table back unchanged"),
        }
    }
}
