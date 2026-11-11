//! Luau bindings for an HTTP client, registered as the global `http`
//! (host-only: the network is ambient state).
//!
//! Modelled on reqwest, which backs it: a client builder, finalized by
//! `:build()` into a client, whose `:get(url)`/`:post(url)`/… return a request
//! builder finalized by an async `:send([target])`.
//!
//! ```lua
//! local api = http.client()
//!     :base_url("https://api.example.com/v1/")
//!     :bearer_auth(token)
//!     :retries(3)
//!     :build()
//!
//! local r, body = api:get("users/42"):query({ expand = "teams" }):send()
//! if r.ok then
//!     local user = json.parse(body)
//! end
//! ```
//!
//! `http.get(url)`/`http.post(url)`/… are the same requests on a default
//! client, built on first use and shared afterwards.
//!
//! Shapes worth knowing before touching this:
//!
//! - **The client builder copies** (like `aws.config`): every setter returns a
//!   new builder, so one base can be refined several ways. **The request
//!   builder mutates** (like the `aws` request builders and `proc.command`):
//!   every setter changes the handle in place and returns it.
//! - **A status is an answer, not an error.** `:send()` raises only for
//!   transport failures (DNS, connect, TLS, timeout, a body cut short) and for
//!   a status outside an explicit `:expect(...)`. Otherwise it returns
//!   `(envelope, body)` with `envelope.ok` meaning 2xx.
//! - **No deserializing.** The body is bytes, delivered through the `proc`
//!   redirect vocabulary every other byte stream uses: a string by default,
//!   `proc.buffer()`, `proc.file(path)` (streamed to disk), `proc.NULL`. The
//!   script composes `json.parse`/`yaml.parse`/… itself. A `proc.file`
//!   target is written only for a 2xx response unless `:keep_failure_body()`
//!   says otherwise, so a failed download leaves no error page behind.
//! - **Client headers are merged per request here, not by reqwest**: reqwest's
//!   `default_headers` keeps only the last value of a repeated header, and
//!   the rule wanted is reqwest's own anyway — a request header replaces the
//!   client default of the same name.
//! - **Retries rebuild the request** from the stored state each attempt, so a
//!   file body is reopened rather than needing a clonable stream.

use std::cell::{OnceCell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use base64::Engine as _;
use mlua::{
    AnyUserData, Lua, MultiValue, Result as LuaResult, Table, UserData, UserDataMethods, Value,
    Variadic,
};
use reqwest::header::{
    AUTHORIZATION, CONTENT_LENGTH, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue, RETRY_AFTER,
    USER_AGENT,
};
use reqwest::{Method, Url};

use crate::scripting::redirect::{
    Redirect, body_target_from_value, capture_to_value, open_shared_file, write_all_blocking,
};
use crate::scripting::time::parse_delta_value;

/// Longest `Retry-After` honoured. A server asking for more is answered with
/// its response instead of a silent stall of that length.
const MAX_RETRY_AFTER: Duration = Duration::from_secs(60);

/// Ceiling on the exponential backoff between attempts.
const MAX_BACKOFF: Duration = Duration::from_secs(8);

/// How much of an unexpected response body an `:expect()` error quotes.
const SNIPPET: usize = 512;

fn err_in(ctx: &str, msg: impl std::fmt::Display) -> mlua::Error {
    mlua::Error::runtime(format!("{ctx}: {msg}"))
}

// ---------------------------------------------------------------------------
// Argument coercion
// ---------------------------------------------------------------------------

/// A string argument (numbers are accepted and stringified, as Luau's own
/// concatenation would), optionally required to be non-empty.
fn text(ctx: &str, what: &str, v: &Value, non_empty: bool) -> LuaResult<String> {
    let s = match v {
        Value::String(s) => s
            .to_str()
            .map_err(|_| err_in(ctx, format!("{what} is not valid UTF-8")))?
            .to_string(),
        Value::Integer(i) => i.to_string(),
        Value::Number(n) => number(*n),
        Value::Boolean(b) => b.to_string(),
        other => {
            return Err(err_in(
                ctx,
                format!("expected a string for {what}, got {}", other.type_name()),
            ));
        }
    };
    if non_empty && s.is_empty() {
        return Err(err_in(ctx, format!("{what} must not be empty")));
    }
    Ok(s)
}

/// A Luau number spelled the way `tostring` would for whole values.
fn number(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        format!("{}", n as i64)
    } else {
        n.to_string()
    }
}

fn header_name(ctx: &str, v: &Value) -> LuaResult<HeaderName> {
    let s = text(ctx, "a header name", v, true)?;
    HeaderName::from_bytes(s.as_bytes())
        .map_err(|_| err_in(ctx, format!("'{s}' is not a valid header name")))
}

fn header_value(ctx: &str, v: &Value) -> LuaResult<HeaderValue> {
    let bytes = match v {
        Value::String(s) => s.as_bytes().to_vec(),
        other => text(ctx, "a header value", other, false)?.into_bytes(),
    };
    HeaderValue::from_bytes(&bytes)
        .map_err(|_| err_in(ctx, "header value contains a control character or newline"))
}

/// A header value table entry: one scalar, or an array of them for a header
/// sent several times.
fn header_values(ctx: &str, v: &Value) -> LuaResult<Vec<HeaderValue>> {
    match v {
        Value::Table(t) => t
            .sequence_values::<Value>()
            .map(|v| header_value(ctx, &v?))
            .collect(),
        other => Ok(vec![header_value(ctx, other)?]),
    }
}

/// A positive delta (`time.parse_delta`'s spellings) as a `Duration`.
fn delta(ctx: &str, v: &Value) -> LuaResult<Duration> {
    let secs = parse_delta_value(v, ctx)?;
    if secs <= 0.0 || secs >= 1e9 {
        return Err(err_in(ctx, format!("expected a positive delta, got {secs}s")));
    }
    Ok(Duration::from_secs_f64(secs))
}

/// A whole number in `[min, max]`.
fn int_in(ctx: &str, what: &str, v: &Value, min: i64, max: i64) -> LuaResult<i64> {
    let n = match v {
        Value::Integer(i) => *i,
        Value::Number(f) if f.fract() == 0.0 => *f as i64,
        other => {
            return Err(err_in(
                ctx,
                format!("expected an integer for {what}, got {}", other.type_name()),
            ));
        }
    };
    if n < min || n > max {
        return Err(err_in(ctx, format!("{what} must be in {min}..{max}, got {n}")));
    }
    Ok(n)
}

/// A `{key = value}` table as sorted `(key, value)` pairs, an array value
/// repeating its key. Sorted because a table's iteration order is not stable,
/// and a request must not differ between two runs of one script.
fn pairs(ctx: &str, t: &Table) -> LuaResult<Vec<(String, String)>> {
    let mut out = Vec::new();
    for entry in t.pairs::<Value, Value>() {
        let (k, v) = entry?;
        let key = text(ctx, "a key", &k, true)?;
        match &v {
            Value::Table(items) => {
                for item in items.sequence_values::<Value>() {
                    out.push((key.clone(), text(ctx, "a value", &item?, false)?));
                }
            }
            other => out.push((key, text(ctx, "a value", other, false)?)),
        }
    }
    // Stable sort by key only, so an array's own order survives.
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

/// A table keyed by header name, sorted the same way `pairs` is.
fn header_table(ctx: &str, t: &Table) -> LuaResult<Vec<(HeaderName, Vec<HeaderValue>)>> {
    let mut out = Vec::new();
    for entry in t.pairs::<Value, Value>() {
        let (k, v) = entry?;
        out.push((header_name(ctx, &k)?, header_values(ctx, &v)?));
    }
    out.sort_by(|a, b| a.0.as_str().cmp(b.0.as_str()));
    Ok(out)
}

fn basic_auth_value(user: &str, pass: Option<&str>) -> HeaderValue {
    let raw = format!("{user}:{}", pass.unwrap_or(""));
    let encoded = base64::engine::general_purpose::STANDARD.encode(raw);
    let mut v = HeaderValue::from_str(&format!("Basic {encoded}")).expect("base64 is a valid header");
    v.set_sensitive(true);
    v
}

fn bearer_auth_value(ctx: &str, token: &str) -> LuaResult<HeaderValue> {
    let mut v = HeaderValue::from_str(&format!("Bearer {token}"))
        .map_err(|_| err_in(ctx, "token contains a control character or newline"))?;
    v.set_sensitive(true);
    Ok(v)
}

fn auth_args(ctx: &str, user: &Value, pass: &Value) -> LuaResult<HeaderValue> {
    let user = text(ctx, "the user", user, false)?;
    let pass = match pass {
        Value::Nil => None,
        other => Some(text(ctx, "the password", other, false)?),
    };
    Ok(basic_auth_value(&user, pass.as_deref()))
}

/// A path from a `proc.file(path)` handle or a plain string.
fn path_arg(ctx: &str, v: &Value) -> LuaResult<PathBuf> {
    match v {
        Value::String(s) => Ok(PathBuf::from(crate::scripting::redirect::os_string(s))),
        Value::UserData(ud) => match ud.borrow::<Redirect>().as_deref() {
            Ok(Redirect::File { path, .. }) => Ok(path.clone()),
            _ => Err(err_in(ctx, "expected a path or proc.file(path)")),
        },
        other => Err(err_in(
            ctx,
            format!("expected a path or proc.file(path), got {}", other.type_name()),
        )),
    }
}

// ---------------------------------------------------------------------------
// Client builder
// ---------------------------------------------------------------------------

#[derive(Clone)]
enum ProxySetting {
    /// reqwest's default: `HTTP(S)_PROXY`/`ALL_PROXY`/`NO_PROXY` and the
    /// system configuration.
    System,
    /// No proxy at all, environment included.
    Off,
    /// One proxy for every scheme.
    Url(String),
}

/// The client knobs. Every one optional, so "not configured" and "configured to
/// the default" stay distinguishable.
#[derive(Clone)]
struct ClientState {
    base_url: Option<Url>,
    headers: HeaderMap,
    timeout: Option<Duration>,
    connect_timeout: Option<Duration>,
    attempts: u32,
    redirects: usize,
    proxy: ProxySetting,
    ca_certs: Vec<PathBuf>,
    identity: Option<PathBuf>,
    insecure: bool,
    keep_failure_body: bool,
}

impl Default for ClientState {
    fn default() -> Self {
        let mut headers = HeaderMap::new();
        headers.insert(
            USER_AGENT,
            HeaderValue::from_static(concat!("ito/", env!("CARGO_PKG_VERSION"))),
        );
        Self {
            base_url: None,
            headers,
            timeout: None,
            connect_timeout: None,
            attempts: 1,
            redirects: 10,
            proxy: ProxySetting::System,
            ca_certs: Vec::new(),
            identity: None,
            insecure: false,
            keep_failure_body: false,
        }
    }
}

impl ClientState {
    /// Finalize into a reqwest client. The only place a certificate or key
    /// file is read, so a bad one raises at `:build()`.
    fn build(&self) -> LuaResult<ClientInner> {
        const CTX: &str = "http.client:build";
        let mut b = reqwest::Client::builder()
            .redirect(if self.redirects == 0 {
                reqwest::redirect::Policy::none()
            } else {
                reqwest::redirect::Policy::limited(self.redirects)
            })
            .tls_danger_accept_invalid_certs(self.insecure);
        if let Some(t) = self.timeout {
            b = b.timeout(t);
        }
        if let Some(t) = self.connect_timeout {
            b = b.connect_timeout(t);
        }
        match &self.proxy {
            ProxySetting::System => {}
            ProxySetting::Off => b = b.no_proxy(),
            ProxySetting::Url(u) => {
                let p = reqwest::Proxy::all(u.as_str())
                    .map_err(|e| err_in(CTX, format!("bad proxy '{u}': {e}")))?;
                b = b.proxy(p);
            }
        }
        for path in &self.ca_certs {
            let pem = std::fs::read(path)
                .map_err(|e| err_in(CTX, format!("failed to read '{}': {e}", path.display())))?;
            let certs = reqwest::Certificate::from_pem_bundle(&pem).map_err(|e| {
                err_in(CTX, format!("'{}' is not a PEM certificate bundle: {e}", path.display()))
            })?;
            if certs.is_empty() {
                return Err(err_in(
                    CTX,
                    format!("'{}' contains no PEM certificate", path.display()),
                ));
            }
            b = b.tls_certs_merge(certs);
        }
        if let Some(path) = &self.identity {
            let pem = std::fs::read(path)
                .map_err(|e| err_in(CTX, format!("failed to read '{}': {e}", path.display())))?;
            let id = reqwest::Identity::from_pem(&pem).map_err(|e| {
                err_in(
                    CTX,
                    format!(
                        "'{}' is not a PEM certificate plus private key: {e}",
                        path.display()
                    ),
                )
            })?;
            b = b.identity(id);
        }
        let http = b.build().map_err(|e| err_in(CTX, error_chain(&e)))?;
        Ok(ClientInner {
            http,
            base_url: self.base_url.clone(),
            headers: self.headers.clone(),
            attempts: self.attempts,
            keep_failure_body: self.keep_failure_body,
        })
    }
}

/// `http.client()`: immutable, each setter yields a fresh builder.
struct ClientBuilder {
    state: ClientState,
}

fn with(
    lua: &Lua,
    ud: &AnyUserData,
    f: impl FnOnce(&mut ClientState),
) -> LuaResult<AnyUserData> {
    let mut state = ud.borrow::<ClientBuilder>()?.state.clone();
    f(&mut state);
    lua.create_userdata(ClientBuilder { state })
}

impl UserData for ClientBuilder {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_function("base_url", |lua, (ud, v): (AnyUserData, Value)| {
            const CTX: &str = "http.client:base_url";
            let s = text(CTX, "the URL", &v, true)?;
            let url = Url::parse(&s).map_err(|e| err_in(CTX, format!("bad URL '{s}': {e}")))?;
            check_scheme(CTX, &url)?;
            with(lua, &ud, |st| st.base_url = Some(url))
        });

        methods.add_function(
            "header",
            |lua, (ud, name, value): (AnyUserData, Value, Value)| {
                const CTX: &str = "http.client:header";
                let name = header_name(CTX, &name)?;
                let value = header_value(CTX, &value)?;
                with(lua, &ud, |st| {
                    st.headers.append(name, value);
                })
            },
        );

        methods.add_function("headers", |lua, (ud, t): (AnyUserData, Table)| {
            let entries = header_table("http.client:headers", &t)?;
            with(lua, &ud, |st| {
                for (name, values) in entries {
                    st.headers.remove(&name);
                    for v in values {
                        st.headers.append(name.clone(), v);
                    }
                }
            })
        });

        methods.add_function(
            "basic_auth",
            |lua, (ud, user, pass): (AnyUserData, Value, Value)| {
                let v = auth_args("http.client:basic_auth", &user, &pass)?;
                with(lua, &ud, |st| {
                    st.headers.insert(AUTHORIZATION, v);
                })
            },
        );

        methods.add_function("bearer_auth", |lua, (ud, token): (AnyUserData, Value)| {
            const CTX: &str = "http.client:bearer_auth";
            let v = bearer_auth_value(CTX, &text(CTX, "the token", &token, true)?)?;
            with(lua, &ud, |st| {
                st.headers.insert(AUTHORIZATION, v);
            })
        });

        methods.add_function("user_agent", |lua, (ud, v): (AnyUserData, Value)| {
            let v = header_value("http.client:user_agent", &v)?;
            with(lua, &ud, |st| {
                st.headers.insert(USER_AGENT, v);
            })
        });

        methods.add_function("timeout", |lua, (ud, v): (AnyUserData, Value)| {
            let d = delta("http.client:timeout", &v)?;
            with(lua, &ud, |st| st.timeout = Some(d))
        });

        methods.add_function("connect_timeout", |lua, (ud, v): (AnyUserData, Value)| {
            let d = delta("http.client:connect_timeout", &v)?;
            with(lua, &ud, |st| st.connect_timeout = Some(d))
        });

        methods.add_function("retries", |lua, (ud, v): (AnyUserData, Value)| {
            let n = int_in("http.client:retries", "the attempt count", &v, 1, 100)? as u32;
            with(lua, &ud, |st| st.attempts = n)
        });

        methods.add_function("redirect", |lua, (ud, v): (AnyUserData, Value)| {
            let n = int_in("http.client:redirect", "the redirect limit", &v, 0, 100)? as usize;
            with(lua, &ud, |st| st.redirects = n)
        });

        methods.add_function("proxy", |lua, (ud, v): (AnyUserData, Value)| {
            const CTX: &str = "http.client:proxy";
            let s = text(CTX, "the proxy URL", &v, true)?;
            // Validated here, so a typo raises at the line that wrote it.
            reqwest::Proxy::all(s.as_str()).map_err(|e| err_in(CTX, format!("bad proxy '{s}': {e}")))?;
            with(lua, &ud, |st| st.proxy = ProxySetting::Url(s))
        });

        methods.add_function("no_proxy", |lua, ud: AnyUserData| {
            with(lua, &ud, |st| st.proxy = ProxySetting::Off)
        });

        methods.add_function("ca_cert", |lua, (ud, v): (AnyUserData, Value)| {
            let p = path_arg("http.client:ca_cert", &v)?;
            with(lua, &ud, |st| st.ca_certs.push(p))
        });

        methods.add_function("identity", |lua, (ud, v): (AnyUserData, Value)| {
            let p = path_arg("http.client:identity", &v)?;
            with(lua, &ud, |st| st.identity = Some(p))
        });

        methods.add_function("insecure", |lua, (ud, v): (AnyUserData, Option<bool>)| {
            with(lua, &ud, |st| st.insecure = v.unwrap_or(true))
        });

        methods.add_function(
            "keep_failure_body",
            |lua, (ud, v): (AnyUserData, Option<bool>)| {
                with(lua, &ud, |st| st.keep_failure_body = v.unwrap_or(true))
            },
        );

        methods.add_function("build", |_, ud: AnyUserData| {
            let inner = ud.borrow::<ClientBuilder>()?.state.build()?;
            Ok(Client {
                inner: Rc::new(inner),
            })
        });
    }
}

fn check_scheme(ctx: &str, url: &Url) -> LuaResult<()> {
    match url.scheme() {
        "http" | "https" => Ok(()),
        other => Err(err_in(
            ctx,
            format!("unsupported scheme '{other}' in '{url}', expected http or https"),
        )),
    }
}

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

struct ClientInner {
    http: reqwest::Client,
    base_url: Option<Url>,
    headers: HeaderMap,
    attempts: u32,
    keep_failure_body: bool,
}

/// A built client: cheap to share (reqwest pools connections behind it).
struct Client {
    inner: Rc<ClientInner>,
}

const METHODS: [(&str, Method); 6] = [
    ("get", Method::GET),
    ("post", Method::POST),
    ("put", Method::PUT),
    ("patch", Method::PATCH),
    ("delete", Method::DELETE),
    ("head", Method::HEAD),
];

/// A method name as given to `request(method, url)`, upper-cased: every
/// registered method is, and a lower-case one would only be rejected by the
/// server.
fn method_arg(ctx: &str, v: &Value) -> LuaResult<Method> {
    let s = text(ctx, "the method", v, true)?.to_ascii_uppercase();
    Method::from_bytes(s.as_bytes()).map_err(|_| err_in(ctx, format!("'{s}' is not a valid method")))
}

/// Resolve `url` against the client's base URL (RFC 3986 reference
/// resolution, so an absolute URL stands alone).
fn resolve(inner: &ClientInner, ctx: &str, url: &Value) -> LuaResult<Url> {
    let s = text(ctx, "the URL", url, true)?;
    let parsed = match &inner.base_url {
        Some(base) => base.join(&s),
        None => Url::parse(&s),
    };
    let url = parsed.map_err(|e| {
        if inner.base_url.is_none() && !s.contains("://") {
            err_in(ctx, format!("'{s}' is relative and the client has no :base_url()"))
        } else {
            err_in(ctx, format!("bad URL '{s}': {e}"))
        }
    })?;
    check_scheme(ctx, &url)?;
    Ok(url)
}

fn new_request(inner: &Rc<ClientInner>, ctx: &str, method: Method, url: &Value) -> LuaResult<Request> {
    let url = resolve(inner, ctx, url)?;
    Ok(Request {
        client: inner.clone(),
        method,
        url,
        state: RefCell::new(RequestState::default()),
    })
}

impl UserData for Client {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        for (name, method) in METHODS {
            methods.add_function(name, move |_, (ud, url): (AnyUserData, Value)| {
                let inner = ud.borrow::<Client>()?.inner.clone();
                new_request(&inner, &format!("http.client:{name}"), method.clone(), &url)
            });
        }
        methods.add_function(
            "request",
            |_, (ud, method, url): (AnyUserData, Value, Value)| {
                const CTX: &str = "http.client:request";
                let inner = ud.borrow::<Client>()?.inner.clone();
                let method = method_arg(CTX, &method)?;
                new_request(&inner, CTX, method, &url)
            },
        );
    }
}

// ---------------------------------------------------------------------------
// Request
// ---------------------------------------------------------------------------

#[derive(Clone, Default)]
enum BodySpec {
    #[default]
    None,
    Bytes {
        data: Vec<u8>,
        /// Sent unless the request already names a `Content-Type`.
        content_type: Option<&'static str>,
    },
    File(PathBuf),
    Multipart(Vec<(String, Part)>),
}

#[derive(Clone)]
enum PartSource {
    Bytes(Vec<u8>),
    File(PathBuf),
}

#[derive(Clone)]
struct Part {
    source: PartSource,
    filename: Option<String>,
    content_type: Option<String>,
}

#[derive(Clone, Default)]
struct RequestState {
    headers: HeaderMap,
    query: Vec<(String, String)>,
    body: BodySpec,
    timeout: Option<Duration>,
    attempts: Option<u32>,
    expect: Option<Vec<u16>>,
    keep_failure_body: Option<bool>,
}

/// A request builder: mutable, every setter changes this handle and returns it.
struct Request {
    client: Rc<ClientInner>,
    method: Method,
    url: Url,
    state: RefCell<RequestState>,
}

fn update(ud: &AnyUserData, f: impl FnOnce(&mut RequestState)) -> LuaResult<AnyUserData> {
    {
        let this = ud.borrow::<Request>()?;
        f(&mut this.state.borrow_mut());
    }
    Ok(ud.clone())
}

/// A request body source: a string or buffer is the body, `proc.file(path)`
/// streams a file, `proc.NULL` is an empty body.
fn body_arg(ctx: &str, v: &Value) -> LuaResult<BodySpec> {
    match v {
        Value::String(s) => Ok(BodySpec::Bytes {
            data: s.as_bytes().to_vec(),
            content_type: None,
        }),
        Value::Buffer(b) => Ok(BodySpec::Bytes {
            data: b.to_vec(),
            content_type: None,
        }),
        Value::UserData(ud) => match ud.borrow::<Redirect>().as_deref() {
            Ok(Redirect::File { path, .. }) => Ok(BodySpec::File(path.clone())),
            Ok(Redirect::Null) => Ok(BodySpec::None),
            _ => Err(bad_body(ctx, "a userdata of another kind")),
        },
        other => Err(bad_body(ctx, other.type_name())),
    }
}

fn bad_body(ctx: &str, got: &str) -> mlua::Error {
    err_in(
        ctx,
        format!("expected a body (a string, a buffer, proc.file(path) or proc.NULL), got {got}"),
    )
}

/// One multipart part: a string/buffer, a `proc.file(path)`, or a table
/// `{ value = <either>, filename = ?, content_type = ? }`.
fn part_arg(ctx: &str, v: &Value) -> LuaResult<Part> {
    let plain = |source| Part {
        source,
        filename: None,
        content_type: None,
    };
    match v {
        Value::String(s) => Ok(plain(PartSource::Bytes(s.as_bytes().to_vec()))),
        Value::Buffer(b) => Ok(plain(PartSource::Bytes(b.to_vec()))),
        Value::UserData(ud) => match ud.borrow::<Redirect>().as_deref() {
            Ok(Redirect::File { path, .. }) => Ok(plain(PartSource::File(path.clone()))),
            _ => Err(bad_part(ctx, "a userdata of another kind")),
        },
        Value::Table(t) => {
            let value: Value = t.get("value")?;
            if value.is_nil() {
                return Err(err_in(ctx, "a part table needs a `value` field"));
            }
            if let Value::Table(_) = value {
                return Err(err_in(ctx, "a part's `value` cannot itself be a table"));
            }
            let mut part = part_arg(ctx, &value)?;
            let opt = |key: &str| -> LuaResult<Option<String>> {
                match t.get::<Value>(key)? {
                    Value::Nil => Ok(None),
                    v => Ok(Some(text(ctx, key, &v, true)?)),
                }
            };
            if let Some(f) = opt("filename")? {
                part.filename = Some(f);
            }
            if let Some(ct) = opt("content_type")? {
                ct.parse::<mime_guess::mime::Mime>()
                    .map_err(|_| err_in(ctx, format!("'{ct}' is not a valid content type")))?;
                part.content_type = Some(ct);
            }
            Ok(part)
        }
        other => Err(bad_part(ctx, other.type_name())),
    }
}

fn bad_part(ctx: &str, got: &str) -> mlua::Error {
    err_in(
        ctx,
        format!(
            "expected a part (a string, a buffer, proc.file(path) or {{ value = … }}), got {got}"
        ),
    )
}

fn multipart_arg(ctx: &str, t: &Table) -> LuaResult<Vec<(String, Part)>> {
    let mut out = Vec::new();
    for entry in t.pairs::<Value, Value>() {
        let (k, v) = entry?;
        let name = text(ctx, "a field name", &k, true)?;
        // An array is several parts under one name; a table with `value` is
        // one part with options.
        match &v {
            Value::Table(items) if items.raw_len() > 0 => {
                for item in items.sequence_values::<Value>() {
                    out.push((name.clone(), part_arg(ctx, &item?)?));
                }
            }
            other => out.push((name, part_arg(ctx, other)?)),
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

impl UserData for Request {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_function(
            "header",
            |_, (ud, name, value): (AnyUserData, Value, Value)| {
                const CTX: &str = "http.request:header";
                let name = header_name(CTX, &name)?;
                let value = header_value(CTX, &value)?;
                update(&ud, |st| {
                    st.headers.append(name, value);
                })
            },
        );

        methods.add_function("headers", |_, (ud, t): (AnyUserData, Table)| {
            let entries = header_table("http.request:headers", &t)?;
            update(&ud, |st| {
                for (name, values) in entries {
                    st.headers.remove(&name);
                    for v in values {
                        st.headers.append(name.clone(), v);
                    }
                }
            })
        });

        methods.add_function(
            "basic_auth",
            |_, (ud, user, pass): (AnyUserData, Value, Value)| {
                let v = auth_args("http.request:basic_auth", &user, &pass)?;
                update(&ud, |st| {
                    st.headers.insert(AUTHORIZATION, v);
                })
            },
        );

        methods.add_function("bearer_auth", |_, (ud, token): (AnyUserData, Value)| {
            const CTX: &str = "http.request:bearer_auth";
            let v = bearer_auth_value(CTX, &text(CTX, "the token", &token, true)?)?;
            update(&ud, |st| {
                st.headers.insert(AUTHORIZATION, v);
            })
        });

        methods.add_function("query", |_, (ud, t): (AnyUserData, Table)| {
            let p = pairs("http.request:query", &t)?;
            update(&ud, |st| st.query.extend(p))
        });

        methods.add_function("body", |_, (ud, v): (AnyUserData, Value)| {
            let b = body_arg("http.request:body", &v)?;
            update(&ud, |st| st.body = b)
        });

        methods.add_function("json", |lua, (ud, v): (AnyUserData, Value)| {
            let json = crate::scripting::json::encode(lua, &v)?;
            let data = serde_json::to_vec(&json)
                .map_err(|e| err_in("http.request:json", format!("serialize error: {e}")))?;
            update(&ud, |st| {
                st.body = BodySpec::Bytes {
                    data,
                    content_type: Some("application/json"),
                }
            })
        });

        methods.add_function("form", |_, (ud, t): (AnyUserData, Table)| {
            let p = pairs("http.request:form", &t)?;
            let data = form_urlencoded::Serializer::new(String::new())
                .extend_pairs(p)
                .finish()
                .into_bytes();
            update(&ud, |st| {
                st.body = BodySpec::Bytes {
                    data,
                    content_type: Some("application/x-www-form-urlencoded"),
                }
            })
        });

        methods.add_function("multipart", |_, (ud, t): (AnyUserData, Table)| {
            let parts = multipart_arg("http.request:multipart", &t)?;
            update(&ud, |st| st.body = BodySpec::Multipart(parts))
        });

        methods.add_function("timeout", |_, (ud, v): (AnyUserData, Value)| {
            let d = delta("http.request:timeout", &v)?;
            update(&ud, |st| st.timeout = Some(d))
        });

        methods.add_function("retries", |_, (ud, v): (AnyUserData, Value)| {
            let n = int_in("http.request:retries", "the attempt count", &v, 1, 100)? as u32;
            update(&ud, |st| st.attempts = Some(n))
        });

        methods.add_function(
            "expect",
            |_, (ud, statuses): (AnyUserData, Variadic<Value>)| {
                const CTX: &str = "http.request:expect";
                if statuses.is_empty() {
                    return Err(err_in(CTX, "expected at least one status"));
                }
                let list = statuses
                    .iter()
                    .map(|v| int_in(CTX, "a status", v, 100, 599).map(|n| n as u16))
                    .collect::<LuaResult<Vec<_>>>()?;
                update(&ud, |st| st.expect = Some(list))
            },
        );

        methods.add_function(
            "keep_failure_body",
            |_, (ud, v): (AnyUserData, Option<bool>)| {
                update(&ud, |st| st.keep_failure_body = Some(v.unwrap_or(true)))
            },
        );

        methods.add_async_function(
            "send",
            |lua, (ud, target): (AnyUserData, Value)| async move {
                let target = body_target_from_value(&target)
                    .map_err(|e| err_in("http.request:send", e))?;
                // Cloned out so no borrow is held across an await.
                let (client, method, url, st) = {
                    let this = ud.borrow::<Request>()?;
                    let st = this.state.borrow().clone();
                    (this.client.clone(), this.method.clone(), this.url.clone(), st)
                };
                send(&lua, &client, method, url, st, target).await
            },
        );
    }
}

// ---------------------------------------------------------------------------
// Sending
// ---------------------------------------------------------------------------

/// The error's own text plus its sources, which reqwest keeps terse at the top
/// ("error sending request") and specific underneath ("connection refused").
fn error_chain(e: &(dyn std::error::Error + 'static)) -> String {
    let mut msg = e.to_string();
    let mut src = e.source();
    while let Some(s) = src {
        let s = s.to_string();
        if !msg.contains(&s) {
            msg.push_str(": ");
            msg.push_str(&s);
        }
        src = src.and_then(|e| e.source());
    }
    msg
}

fn transport_err(method: &Method, url: &Url, e: reqwest::Error) -> mlua::Error {
    mlua::Error::runtime(format!("http: {method} {url}: {}", error_chain(&e.without_url())))
}

/// Build one attempt's request from the stored state. Files are opened here,
/// so a retry reads them afresh.
async fn build_request(
    client: &ClientInner,
    method: &Method,
    url: &Url,
    st: &RequestState,
) -> LuaResult<reqwest::Request> {
    let mut url = url.clone();
    if !st.query.is_empty() {
        url.query_pairs_mut().extend_pairs(&st.query);
    }

    // Client defaults, with every name the request sets replaced wholesale.
    let mut headers = client.headers.clone();
    for name in st.headers.keys() {
        headers.remove(name);
    }
    for (name, value) in &st.headers {
        headers.append(name.clone(), value.clone());
    }

    let ctx = format!("http: {method} {url}");
    let mut body: Option<reqwest::Body> = None;
    let mut form: Option<reqwest::multipart::Form> = None;
    match &st.body {
        BodySpec::None => {}
        BodySpec::Bytes { data, content_type } => {
            if let Some(ct) = content_type
                && !headers.contains_key(CONTENT_TYPE)
            {
                headers.insert(CONTENT_TYPE, HeaderValue::from_static(ct));
            }
            body = Some(data.clone().into());
        }
        BodySpec::File(path) => {
            let (file, len) = open_for_upload(&ctx, path).await?;
            // A streamed body has no size of its own; naming it keeps the
            // upload un-chunked, which presigned URLs and many servers need.
            headers.insert(CONTENT_LENGTH, HeaderValue::from(len));
            body = Some(reqwest::Body::from(file));
        }
        BodySpec::Multipart(parts) => {
            let mut f = reqwest::multipart::Form::new();
            for (name, part) in parts {
                f = f.part(name.clone(), build_part(&ctx, part).await?);
            }
            form = Some(f);
        }
    }

    let mut rb = client.http.request(method.clone(), url).headers(headers);
    if let Some(b) = body {
        rb = rb.body(b);
    }
    if let Some(f) = form {
        rb = rb.multipart(f);
    }
    if let Some(t) = st.timeout {
        rb = rb.timeout(t);
    }
    rb.build().map_err(|e| err_in(&ctx, error_chain(&e.without_url())))
}

async fn open_for_upload(ctx: &str, path: &PathBuf) -> LuaResult<(tokio::fs::File, u64)> {
    let file = tokio::fs::File::open(path)
        .await
        .map_err(|e| err_in(ctx, format!("failed to open '{}': {e}", path.display())))?;
    let len = file
        .metadata()
        .await
        .map_err(|e| err_in(ctx, format!("failed to stat '{}': {e}", path.display())))?
        .len();
    Ok((file, len))
}

async fn build_part(ctx: &str, part: &Part) -> LuaResult<reqwest::multipart::Part> {
    let (mut p, guessed) = match &part.source {
        PartSource::Bytes(b) => (reqwest::multipart::Part::bytes(b.clone()), None),
        PartSource::File(path) => {
            let (file, len) = open_for_upload(ctx, path).await?;
            let mut p = reqwest::multipart::Part::stream_with_length(reqwest::Body::from(file), len);
            if let Some(name) = path.file_name() {
                p = p.file_name(name.to_string_lossy().into_owned());
            }
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            (p, Some(mime.to_string()))
        }
    };
    if let Some(f) = &part.filename {
        p = p.file_name(f.clone());
    }
    if let Some(ct) = part.content_type.as_ref().or(guessed.as_ref()) {
        p = p
            .mime_str(ct)
            .map_err(|e| err_in(ctx, format!("bad content type '{ct}': {e}")))?;
    }
    Ok(p)
}

/// Statuses worth another attempt: rate limiting and the gateway family.
/// A plain 500 is left alone; it is as often a deterministic bug as a blip.
fn retryable_status(s: reqwest::StatusCode) -> bool {
    matches!(s.as_u16(), 429 | 502 | 503 | 504)
}

fn backoff(attempt: u32) -> Duration {
    let d = Duration::from_millis(500).saturating_mul(1 << (attempt - 1).min(10));
    d.min(MAX_BACKOFF)
}

/// `Retry-After` as seconds or an HTTP date; a date in the past is "now".
fn retry_after(resp: &reqwest::Response) -> Option<Duration> {
    let v = resp.headers().get(RETRY_AFTER)?.to_str().ok()?.trim();
    if let Ok(secs) = v.parse::<u64>() {
        return Some(Duration::from_secs(secs));
    }
    let when = jiff::fmt::rfc2822::parse(v).ok()?.timestamp();
    let d = when.duration_since(jiff::Timestamp::now());
    Some(Duration::try_from(d).unwrap_or(Duration::ZERO))
}

async fn send(
    lua: &Lua,
    client: &ClientInner,
    method: Method,
    url: Url,
    st: RequestState,
    target: Option<Redirect>,
) -> LuaResult<MultiValue> {
    let attempts = st.attempts.unwrap_or(client.attempts).max(1);
    // Only an idempotent request may be sent twice: a retried POST that had
    // reached the server would act twice.
    let may_retry = method.is_idempotent();
    let mut attempt = 1;
    let mut resp = loop {
        let req = build_request(client, &method, &url, &st).await?;
        let last = attempt >= attempts || !may_retry;
        match client.http.execute(req).await {
            Ok(resp) => {
                if !last && retryable_status(resp.status()) {
                    let wait = retry_after(&resp).unwrap_or_else(|| backoff(attempt));
                    if wait <= MAX_RETRY_AFTER {
                        drop(resp);
                        tokio::time::sleep(wait).await;
                        attempt += 1;
                        continue;
                    }
                }
                break resp;
            }
            Err(e) if !last && (e.is_connect() || e.is_timeout()) => {
                tokio::time::sleep(backoff(attempt)).await;
                attempt += 1;
            }
            Err(e) => return Err(transport_err(&method, &url, e)),
        }
    };

    let status = resp.status();
    if let Some(expect) = &st.expect
        && !expect.contains(&status.as_u16())
    {
        // Read a little of the body for the message; the target is never
        // touched, so an error page cannot land in a download.
        let mut snippet = Vec::new();
        while snippet.len() < SNIPPET {
            match resp.chunk().await {
                Ok(Some(c)) => snippet.extend_from_slice(&c),
                _ => break,
            }
        }
        snippet.truncate(SNIPPET);
        let text = String::from_utf8_lossy(&snippet);
        let text = text.trim();
        let wanted = expect
            .iter()
            .map(u16::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        let mut msg = format!("http: {method} {url}: status {status}, expected {wanted}");
        if !text.is_empty() {
            msg.push_str(": ");
            msg.push_str(text);
        }
        return Err(mlua::Error::runtime(msg));
    }

    let env = envelope(lua, &resp)?;
    let body_err = |e: reqwest::Error| {
        mlua::Error::runtime(format!(
            "http: {method} {url}: reading the body: {}",
            error_chain(&e.without_url())
        ))
    };
    let second = match target {
        None | Some(Redirect::Capture { .. }) => {
            let mut bytes = Vec::new();
            while let Some(c) = resp.chunk().await.map_err(body_err)? {
                bytes.extend_from_slice(&c);
            }
            match target {
                Some(Redirect::Capture { kind, .. }) => capture_to_value(lua, kind, &bytes)?,
                _ => Value::String(lua.create_string(&bytes)?),
            }
        }
        // Still drained, so an error mid-body is reported, not dropped.
        Some(Redirect::Null) => {
            while resp.chunk().await.map_err(body_err)?.is_some() {}
            Value::Nil
        }
        // A failed response never reaches a file unless asked: a download that
        // got an error page must not leave a plausible-looking file behind.
        // The body is not read, so nothing is opened or truncated.
        Some(Redirect::File { .. })
            if !status.is_success() && !st.keep_failure_body.unwrap_or(client.keep_failure_body) =>
        {
            Value::Nil
        }
        // Chunk by chunk as it arrives, so a large body never sits in memory.
        Some(Redirect::File { path, file, append }) => {
            let mut handle = Some(open_shared_file(&path, &file, append)?);
            while let Some(c) = resp.chunk().await.map_err(body_err)? {
                let f = handle.take().expect("handle round-trips each chunk");
                handle = Some(write_all_blocking(f, c.to_vec(), &path).await?);
            }
            Value::String(lua.create_string(path.to_string_lossy().as_bytes())?)
        }
        Some(Redirect::Inherit(_) | Redirect::AliasStdout | Redirect::AliasStderr) => {
            unreachable!("rejected by body_target_from_value")
        }
    };
    Ok(MultiValue::from_vec(vec![Value::Table(env), second]))
}

/// `{ ok, status, url, version, headers }` for a response.
fn envelope(lua: &Lua, resp: &reqwest::Response) -> LuaResult<Table> {
    let t = lua.create_table()?;
    t.raw_set("ok", resp.status().is_success())?;
    t.raw_set("status", resp.status().as_u16())?;
    t.raw_set("url", resp.url().as_str())?;
    t.raw_set("version", format!("{:?}", resp.version()))?;
    let headers = lua.create_table()?;
    for name in resp.headers().keys() {
        // RFC 9110 §5.3: repeated fields combine with ", ".
        let joined = resp
            .headers()
            .get_all(name)
            .iter()
            .map(|v| String::from_utf8_lossy(v.as_bytes()).into_owned())
            .collect::<Vec<_>>()
            .join(", ");
        headers.raw_set(name.as_str(), joined)?;
    }
    t.raw_set("headers", headers)?;
    Ok(t)
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

/// The default client behind `http.get(...)` and friends, built on first use.
type Ambient = Rc<OnceCell<Rc<ClientInner>>>;

fn ambient_client(cell: &Ambient) -> LuaResult<Rc<ClientInner>> {
    if let Some(c) = cell.get() {
        return Ok(c.clone());
    }
    let c = Rc::new(ClientState::default().build()?);
    let _ = cell.set(c.clone());
    Ok(c)
}

/// Install the `http` global.
pub fn register(lua: &Lua) -> LuaResult<()> {
    let http = lua.create_table()?;
    http.raw_set(
        "client",
        lua.create_function(|_, ()| {
            Ok(ClientBuilder {
                state: ClientState::default(),
            })
        })?,
    )?;

    let ambient: Ambient = Rc::new(OnceCell::new());
    for (name, method) in METHODS {
        let cell = ambient.clone();
        http.raw_set(
            name,
            lua.create_function(move |_, url: Value| {
                let inner = ambient_client(&cell)?;
                new_request(&inner, &format!("http.{name}"), method.clone(), &url)
            })?,
        )?;
    }
    let cell = ambient.clone();
    http.raw_set(
        "request",
        lua.create_function(move |_, (method, url): (Value, Value)| {
            const CTX: &str = "http.request";
            let method = method_arg(CTX, &method)?;
            let inner = ambient_client(&cell)?;
            new_request(&inner, CTX, method, &url)
        })?,
    )?;

    lua.globals().raw_set("http", http)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// A tiny HTTP/1.1 server, one request per connection:
    ///
    /// - `/echo` → 200, a JSON description of the request
    /// - `/status/<n>` → status `n`, body `status <n>`
    /// - `/redirect` → 302 to `/echo`
    /// - `/flaky/<key>/<n>` → 503 for the first `n` hits of `key`, then 200 `ok`
    /// - `/bytes` → 200, the bytes 0..=255
    async fn serve() -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let hits: Rc<RefCell<HashMap<String, u32>>> = Rc::default();
        tokio::task::spawn_local(async move {
            loop {
                let (sock, _) = listener.accept().await.unwrap();
                let hits = hits.clone();
                tokio::task::spawn_local(async move { handle(sock, hits).await });
            }
        });
        format!("http://{addr}")
    }

    async fn handle(mut sock: tokio::net::TcpStream, hits: Rc<RefCell<HashMap<String, u32>>>) {
        let mut buf = Vec::new();
        let mut chunk = [0u8; 4096];
        let head_end = loop {
            let n = sock.read(&mut chunk).await.unwrap();
            if n == 0 {
                return;
            }
            buf.extend_from_slice(&chunk[..n]);
            if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                break i + 4;
            }
        };
        let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
        let mut lines = head.split("\r\n");
        let mut first = lines.next().unwrap().split(' ');
        let method = first.next().unwrap().to_string();
        let target = first.next().unwrap().to_string();
        let mut headers = serde_json::Map::new();
        for line in lines.filter(|l| !l.is_empty()) {
            let (k, v) = line.split_once(':').unwrap();
            let k = k.trim().to_ascii_lowercase();
            let v = v.trim().to_string();
            match headers.get_mut(&k) {
                Some(serde_json::Value::String(prev)) => *prev = format!("{prev}, {v}"),
                _ => {
                    headers.insert(k, serde_json::Value::String(v));
                }
            }
        }
        let len: usize = headers
            .get("content-length")
            .and_then(|v| v.as_str())
            .map(|v| v.parse().unwrap())
            .unwrap_or(0);
        let mut body = buf[head_end..].to_vec();
        while body.len() < len {
            let n = sock.read(&mut chunk).await.unwrap();
            body.extend_from_slice(&chunk[..n]);
        }
        let (path, query) = target.split_once('?').unwrap_or((&target, ""));
        let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();

        let (status, extra, out): (u16, String, Vec<u8>) = match parts.as_slice() {
            ["echo"] => {
                let j = serde_json::json!({
                    "method": method,
                    "path": path,
                    "query": query,
                    "headers": headers,
                    "body": String::from_utf8_lossy(&body),
                });
                (200, "Content-Type: application/json\r\n".into(), j.to_string().into_bytes())
            }
            ["status", n] => (n.parse().unwrap(), String::new(), format!("status {n}").into_bytes()),
            ["redirect"] => (302, "Location: /echo\r\n".into(), Vec::new()),
            ["flaky", key, n] => {
                let mut h = hits.borrow_mut();
                let c = h.entry(key.to_string()).or_default();
                *c += 1;
                if *c <= n.parse().unwrap() {
                    (503, "Retry-After: 0\r\n".into(), b"busy".to_vec())
                } else {
                    (200, String::new(), format!("ok after {c}").into_bytes())
                }
            }
            ["bytes"] => (200, String::new(), (0..=255u8).collect()),
            _ => (404, String::new(), b"nope".to_vec()),
        };
        let resp = format!(
            "HTTP/1.1 {status} X\r\n{extra}Set-Cookie: a=1\r\nSet-Cookie: b=2\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            out.len()
        );
        sock.write_all(resp.as_bytes()).await.unwrap();
        sock.write_all(&out).await.unwrap();
        let _ = sock.shutdown().await;
    }

    fn eval(script: &str) -> LuaResult<()> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let local = tokio::task::LocalSet::new();
        local.block_on(&runtime, async {
            let base = serve().await;
            let lua = Lua::new();
            crate::scripting::null::register(&lua)?;
            crate::scripting::codec::register_array(&lua)?;
            crate::scripting::json::register(&lua)?;
            crate::scripting::proc::register(&lua)?;
            register(&lua)?;
            lua.globals().set("BASE", base)?;
            lua.load(script).exec_async().await
        })
    }

    fn eval_err(script: &str) -> String {
        eval(script).expect_err("expected an error").to_string()
    }

    #[test]
    fn get_returns_envelope_and_string_body() {
        eval(
            r#"
            local r, body = http.get(BASE .. "/echo"):send()
            assert(r.ok and r.status == 200, "status")
            assert(r.url == BASE .. "/echo", r.url)
            assert(r.version == "HTTP/1.1", r.version)
            assert(r.headers["content-type"] == "application/json")
            assert(r.headers["set-cookie"] == "a=1, b=2", r.headers["set-cookie"])
            assert(type(body) == "string")
            local e = json.parse(body)
            assert(e.method == "GET")
            assert(e.headers["user-agent"]:match("^ito/"), e.headers["user-agent"])
            "#,
        )
        .unwrap();
    }

    #[test]
    fn non_2xx_is_an_answer() {
        eval(
            r#"
            local r, body = http.get(BASE .. "/status/404"):send()
            assert(r.ok == false and r.status == 404)
            assert(body == "status 404")
            "#,
        )
        .unwrap();
    }

    #[test]
    fn expect_raises_with_snippet() {
        let e = eval_err(r#"http.get(BASE .. "/status/500"):expect(200, 204):send()"#);
        assert!(e.contains("status 500"), "{e}");
        assert!(e.contains("expected 200, 204"), "{e}");
        assert!(e.contains(": status 500"), "{e}");
    }

    #[test]
    fn expect_failure_leaves_file_target_alone() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out");
        let script = format!(
            r#"
            local ok = pcall(function()
                http.get(BASE .. "/status/503"):expect(200):send(proc.file("{}"))
            end)
            assert(not ok)
            "#,
            path.display()
        );
        eval(&script).unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn targets() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bytes.bin");
        let script = format!(
            r#"
            local _, b = http.get(BASE .. "/bytes"):send(proc.buffer())
            assert(type(b) == "buffer" and buffer.len(b) == 256)
            assert(buffer.readu8(b, 255) == 255)
            local _, s = http.get(BASE .. "/bytes"):send(proc.string())
            assert(#s == 256)
            local _, n = http.get(BASE .. "/bytes"):send(proc.NULL)
            assert(n == nil)
            local _, p = http.get(BASE .. "/bytes"):send(proc.file("{0}"))
            assert(p == "{0}")
            "#,
            path.display()
        );
        eval(&script).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), (0..=255u8).collect::<Vec<_>>());
    }

    #[test]
    fn failed_response_skips_file_unless_kept() {
        let dir = tempfile::tempdir().unwrap();
        let skipped = dir.path().join("skipped");
        let kept = dir.path().join("kept");
        let client_kept = dir.path().join("client_kept");
        let script = format!(
            r#"
            local r, p = http.get(BASE .. "/status/404"):send(proc.file("{0}"))
            assert(r.status == 404 and p == nil)
            local r, p = http.get(BASE .. "/status/404"):keep_failure_body():send(proc.file("{1}"))
            assert(p == "{1}")
            local c = http.client():keep_failure_body():build()
            c:get(BASE .. "/status/500"):send(proc.file("{2}"))
            local r, p = c:get(BASE .. "/status/500"):keep_failure_body(false):send(proc.file("{0}"))
            assert(p == nil)
            "#,
            skipped.display(),
            kept.display(),
            client_kept.display()
        );
        eval(&script).unwrap();
        assert!(!skipped.exists());
        assert_eq!(std::fs::read_to_string(&kept).unwrap(), "status 404");
        assert_eq!(std::fs::read_to_string(&client_kept).unwrap(), "status 500");
    }

    #[test]
    fn rejects_stream_targets() {
        let e = eval_err(r#"http.get(BASE .. "/echo"):send(proc.INHERIT)"#);
        assert!(e.contains("expected a body target"), "{e}");
    }

    #[test]
    fn client_base_url_headers_and_auth() {
        eval(
            r#"
            local api = http.client()
                :base_url(BASE .. "/")
                :header("X-A", "1")
                :header("X-A", "2")
                :header("X-B", "client")
                :bearer_auth("tok")
                :user_agent("test/1")
                :build()
            local _, body = api:get("echo"):header("X-B", "request"):send()
            local h = json.parse(body).headers
            assert(h["x-a"] == "1, 2", h["x-a"])
            assert(h["x-b"] == "request", h["x-b"])
            assert(h["authorization"] == "Bearer tok")
            assert(h["user-agent"] == "test/1")
            local _, body = api:get("echo"):basic_auth("u", "p"):send()
            assert(json.parse(body).headers.authorization == "Basic dTpw")
            "#,
        )
        .unwrap();
    }

    #[test]
    fn client_builder_copies_request_builder_mutates() {
        eval(
            r#"
            local base = http.client():base_url(BASE .. "/")
            local _ = base:header("X-Only", "variant")
            local _, body = base:build():get("echo"):send()
            assert(json.parse(body).headers["x-only"] == nil)

            local req = http.get(BASE .. "/echo")
            req:header("X-Late", "yes")   -- return value discarded
            local _, body = req:send()
            assert(json.parse(body).headers["x-late"] == "yes")
            "#,
        )
        .unwrap();
    }

    #[test]
    fn relative_url_without_base_raises() {
        let e = eval_err(r#"http.get("echo")"#);
        assert!(e.contains("has no :base_url()"), "{e}");
        let e = eval_err(r#"http.get("ftp://x/y")"#);
        assert!(e.contains("unsupported scheme"), "{e}");
    }

    #[test]
    fn query_is_sorted_and_repeats_arrays() {
        eval(
            r#"
            local _, body = http.get(BASE .. "/echo?z=0")
                :query({ b = "2", a = { "x", "y" }, n = 3 })
                :query({ s = "a b&c" })
                :send()
            local q = json.parse(body).query
            assert(q == "z=0&a=x&a=y&b=2&n=3&s=a+b%26c", q)
            "#,
        )
        .unwrap();
    }

    #[test]
    fn json_form_and_raw_bodies() {
        eval(
            r#"
            local _, body = http.post(BASE .. "/echo"):json({ a = 1, list = table.array() }):send()
            local e = json.parse(body)
            assert(e.method == "POST")
            assert(e.headers["content-type"] == "application/json")
            assert(json.parse(e.body).a == 1)
            assert(e.body:find('"list":%[%]'), e.body)

            local _, body = http.put(BASE .. "/echo"):form({ b = "x y", a = "1" }):send()
            local e = json.parse(body)
            assert(e.headers["content-type"] == "application/x-www-form-urlencoded")
            assert(e.body == "a=1&b=x+y", e.body)

            local _, body = http.post(BASE .. "/echo")
                :header("Content-Type", "text/plain")
                :json({ x = 1 })
                :send()
            assert(json.parse(body).headers["content-type"] == "text/plain")

            local _, body = http.request("patch", BASE .. "/echo"):body("raw"):send()
            local e = json.parse(body)
            assert(e.method == "PATCH" and e.body == "raw")
            "#,
        )
        .unwrap();
    }

    #[test]
    fn file_body_is_streamed_with_length() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("up.txt");
        std::fs::write(&path, "uploaded").unwrap();
        let script = format!(
            r#"
            local _, body = http.put(BASE .. "/echo"):body(proc.file("{}")):send()
            local e = json.parse(body)
            assert(e.body == "uploaded", e.body)
            assert(e.headers["content-length"] == "8")
            assert(e.headers["transfer-encoding"] == nil)
            "#,
            path.display()
        );
        eval(&script).unwrap();
    }

    #[test]
    fn multipart_parts() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("report.json");
        std::fs::write(&path, "{}").unwrap();
        let script = format!(
            r#"
            local _, body = http.post(BASE .. "/echo"):multipart({{
                title = "hi",
                file = proc.file("{}"),
                blob = {{ value = "raw", filename = "b.bin", content_type = "application/x-test" }},
                tags = {{ "a", "b" }},
            }}):send()
            local e = json.parse(body)
            assert(e.headers["content-type"]:match("^multipart/form%-data; boundary="))
            local b = e.body
            assert(b:find('name="title"\r\n\r\nhi', 1, true), b)
            assert(b:find('name="file"; filename="report.json"\r\nContent%-Type: application/json'), b)
            assert(b:find('filename="b.bin"\r\nContent%-Type: application/x%-test'), b)
            local _, n = b:gsub('name="tags"', "")
            assert(n == 2)
            "#,
            path.display()
        );
        eval(&script).unwrap();
    }

    #[test]
    fn redirects_follow_or_stop() {
        eval(
            r#"
            local r = http.get(BASE .. "/redirect"):send()
            assert(r.status == 200 and r.url == BASE .. "/echo")
            local c = http.client():redirect(0):build()
            local r = c:get(BASE .. "/redirect"):send()
            assert(r.status == 302 and r.headers.location == "/echo")
            "#,
        )
        .unwrap();
    }

    #[test]
    fn retries_idempotent_only() {
        eval(
            r#"
            local r, body = http.get(BASE .. "/flaky/g/2"):retries(3):send()
            assert(r.status == 200 and body == "ok after 3", body)

            local r = http.get(BASE .. "/flaky/g2/5"):retries(2):send()
            assert(r.status == 503)

            local c = http.client():retries(3):build()
            local r = c:post(BASE .. "/flaky/p/1"):send()
            assert(r.status == 503, "POST must not be retried")
            local r, body = c:put(BASE .. "/flaky/u/1"):send()
            assert(r.status == 200 and body == "ok after 2")
            "#,
        )
        .unwrap();
    }

    #[test]
    fn transport_error_raises() {
        // A port nothing listens on.
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let e = eval_err(&format!(r#"http.get("http://127.0.0.1:{port}/"):send()"#));
        assert!(e.contains(&format!("http: GET http://127.0.0.1:{port}/")), "{e}");
        assert!(e.to_lowercase().contains("connect"), "{e}");
    }

    #[test]
    fn setter_validation() {
        for (src, want) in [
            (r#"http.get(BASE):header("bad name", "x")"#, "not a valid header name"),
            (r#"http.get(BASE):header("X", "a\nb")"#, "control character"),
            (r#"http.get(BASE):timeout(0)"#, "positive delta"),
            (r#"http.get(BASE):timeout("1M")"#, "http.request:timeout"),
            (r#"http.get(BASE):expect(42)"#, "a status must be in"),
            (r#"http.get(BASE):expect()"#, "at least one status"),
            (r#"http.get(BASE):retries(0)"#, "attempt count"),
            (r#"http.get(BASE):body({})"#, "expected a body"),
            (r#"http.client():base_url("nope")"#, "bad URL"),
            (r#"http.client():proxy("::")"#, "bad proxy"),
            (r#"http.request("b@d", BASE)"#, "not a valid method"),
        ] {
            let e = eval_err(src);
            assert!(e.contains(want), "{src}: {e}");
        }
    }

    #[test]
    fn bad_ca_cert_raises_at_build() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ca.pem");
        std::fs::write(&path, "not a cert").unwrap();
        let e = eval_err(&format!(
            r#"http.client():ca_cert("{}"):build()"#,
            path.display()
        ));
        assert!(e.contains("http.client:build"), "{e}");
    }

    #[test]
    fn backoff_grows_and_caps() {
        assert_eq!(backoff(1), Duration::from_millis(500));
        assert_eq!(backoff(3), Duration::from_secs(2));
        assert_eq!(backoff(30), MAX_BACKOFF);
    }
}
