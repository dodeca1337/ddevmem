//! Optional web UI for viewing and editing register maps.
//!
//! Enable the `web` feature to pull in [`axum`] and get a ready-made HTTP
//! interface for any type that implements [`RegisterMapInfo`] (automatically
//! derived by [`register_map!`](crate::register_map)).
//!
//! Typed bitfields (`as bool`, `as enum`) are rendered as dropdown selectors
//! in the web UI instead of plain numeric inputs.
//!
//! [`WebUi`] is a builder that hosts one or more register maps on a single
//! page. The resulting [`Router`] has **no root path baked in**, so it can be
//! nested freely under any prefix with [`Router::nest`].
//!
//! # Security
//!
//! [`WebUi::with_auth`] uses HTTP Basic authentication. Credentials are sent
//! `base64`-encoded — **not** encrypted. The web UI is intended for trusted
//! networks (lab benches, internal VLANs, SSH-tunnels). For any exposed
//! deployment **always terminate TLS in front of the server** (e.g. with
//! `nginx`, `caddy`, or `axum-server` + `rustls`).
//!
//! When implementing the credential check, compare secrets in **constant
//! time** to avoid leaking the password through response-time side channels.
//! Use [`ct_eq`] for a ready-made constant-time string comparison:
//!
//! ```rust,no_run
//! # use std::sync::Arc;
//! # use tokio::sync::Mutex;
//! # use ddevmem::{register_map, DevMem};
//! # use ddevmem::web::{WebUi, ct_eq};
//! # register_map! { pub unsafe map R (u32) { 0x00 => rw x: u32 } }
//! # async fn run() {
//! # let devmem = unsafe { DevMem::new(0x0, None).unwrap() };
//! # let regs = unsafe { R::new(Arc::new(devmem)).unwrap() };
//! # let regs = Arc::new(Mutex::new(regs));
//! let app = WebUi::new()
//!     .add("r", regs)
//!     .with_auth(|user, pass| async move {
//!         ct_eq(&user, "admin") & ct_eq(&pass, "hunter2")
//!     })
//!     .build();
//! # }
//! ```
//!
//! Note the bitwise `&` (not `&&`): short-circuit evaluation would re-introduce
//! the timing leak.
//!
//! There is currently **no built-in CSRF protection or rate limiting**. If
//! the same browser session may visit untrusted origins while authenticated,
//! protect the deployment with a reverse proxy that enforces `Origin` /
//! `Referer` checks or that rate-limits failed authentications.
//!
//! # One map
//!
//! ```rust,no_run
//! use std::sync::Arc;
//! use tokio::sync::Mutex;
//! use ddevmem::{register_map, DevMem};
//! use ddevmem::web::WebUi;
//!
//! register_map! {
//!     pub unsafe map Regs (u32) {
//!         0x00 =>
//!             /// Control register
//!             rw control: u32 { enable: 0, mode: 1..=3 },
//!         0x04 =>
//!             /// Status register
//!             ro status: u32
//!     }
//! }
//!
//! # async fn run() {
//! let devmem = unsafe { DevMem::new(0x4000_0000, None).unwrap() };
//! let regs = unsafe { Regs::new(Arc::new(devmem)).unwrap() };
//!
//! let app = axum::Router::new().nest(
//!     "/registers/axi",
//!     WebUi::new()
//!         .add("axi", Arc::new(Mutex::new(regs)))
//!         .build(),
//! );
//!
//! let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
//! axum::serve(listener, app).await.unwrap();
//! # }
//! ```
//!
//! # Multiple maps on one page
//!
//! ```rust,no_run
//! # use std::sync::Arc;
//! # use tokio::sync::Mutex;
//! # use ddevmem::{register_map, DevMem};
//! # use ddevmem::web::WebUi;
//! # register_map! { pub unsafe map Spi (u32) { 0x00 => rw cr: u32 } }
//! # register_map! { pub unsafe map Gpio (u32) { 0x00 => rw data: u32 } }
//! # async fn run() {
//! # let d1 = unsafe { DevMem::new(0x0, Some(256)).unwrap() };
//! # let d2 = unsafe { DevMem::new(0x0, Some(256)).unwrap() };
//! # let spi = unsafe { Spi::new(Arc::new(d1)).unwrap() };
//! # let gpio = unsafe { Gpio::new(Arc::new(d2)).unwrap() };
//! let app = axum::Router::new().nest(
//!     "/hw",
//!     WebUi::new()
//!         .add("spi", Arc::new(Mutex::new(spi)))
//!         .add("gpio", Arc::new(Mutex::new(gpio)))
//!         .build(),
//! );
//! # }
//! ```

use axum::{
    body::Body,
    extract::State,
    http::{Request, StatusCode},
    middleware::{self, Next},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::Mutex;

// ─── Metadata types (constructed by the `register_map!` macro) ───────────────

/// An enum variant exposed for the web UI.
#[derive(Debug, Clone, Serialize)]
pub struct VariantInfo {
    /// Variant name.
    pub name: &'static str,
    /// Raw integer value.
    pub value: u64,
}

/// Description of a single bitfield within a register.
#[derive(Debug, Clone, Serialize)]
pub struct BitfieldInfo {
    /// Name of the bitfield.
    pub name: &'static str,
    /// Documentation string (from `/// ...` comments in the macro).
    pub doc: &'static str,
    /// Low bit index (inclusive).
    pub lo: u32,
    /// High bit index (inclusive).
    pub hi: u32,
    /// Type hint: `"raw"`, `"bool"`, `"u8"`, or an enum name.
    pub field_type: &'static str,
    /// Enum/bool variants (empty for plain integer fields).
    pub variants: Vec<VariantInfo>,
}

/// Description of a single register in the map.
#[derive(Debug, Clone, Serialize)]
pub struct RegisterInfo {
    /// Register name.
    ///
    /// Owned `String` so the macro can synthesize names for array elements
    /// (`fifo[0]`, `fifo[1]`, …) at runtime.
    pub name: String,
    /// Documentation string.
    pub doc: &'static str,
    /// Byte offset from the base address.
    pub offset: usize,
    /// Access kind: `"rw"`, `"ro"`, or `"wo"`.
    pub access: &'static str,
    /// Width of the register value in bits (e.g. 32).
    pub width: usize,
    /// Bitfields declared within this register.
    pub bitfields: Vec<BitfieldInfo>,
}

// ─── Trait ───────────────────────────────────────────────────────────────────

/// Trait automatically implemented by [`register_map!`](crate::register_map)
/// when the `web` feature is enabled. Provides register metadata and raw
/// read/write access for the web UI.
pub trait RegisterMapInfo {
    /// Name of the register map (struct name).
    fn map_name(&self) -> &'static str;

    /// Bus width in bytes.
    fn bus_width(&self) -> usize;

    /// Physical base address of the mapped region.
    fn base_address(&self) -> usize;

    /// Returns a description of every declared register, including bitfields.
    fn registers(&self) -> Vec<RegisterInfo>;

    /// Read a register at the given byte offset, returning its value as `u64`.
    fn read_register(&self, offset: usize) -> Option<u64>;

    /// Write a register at the given byte offset from a `u64` value.
    fn write_register(&mut self, offset: usize, value: u64) -> Option<()>;
}

// ─── Internal serialization helpers ──────────────────────────────────────────

#[derive(Serialize)]
struct RegisterMapDescription {
    name: &'static str,
    bus_width: usize,
    base_address: usize,
    registers: Vec<RegisterInfo>,
}

#[derive(Deserialize)]
struct ReadReq {
    offset: usize,
}

#[derive(Serialize)]
struct ReadResp {
    value: u64,
}

#[derive(Deserialize)]
struct WriteReq {
    offset: usize,
    value: u64,
}

// ─── Auth ────────────────────────────────────────────────────────────────────

/// Compare two strings in constant time.
///
/// Use this inside the closure passed to [`WebUi::with_auth`] when checking
/// passwords or other secret material. A naive `==` exits at the first
/// differing byte and lets an attacker recover the secret one byte at a time
/// by timing responses.
///
/// ```
/// # use ddevmem::web::ct_eq;
/// assert!(ct_eq("hunter2", "hunter2"));
/// assert!(!ct_eq("hunter2", "hunter3"));
/// // Different lengths are still safe to compare:
/// assert!(!ct_eq("admin", "administrator"));
/// ```
///
/// When combining several checks, prefer the bitwise `&` operator over `&&`
/// so that all comparisons run unconditionally:
///
/// ```ignore
/// ct_eq(user, "admin") & ct_eq(pass, "hunter2")
/// ```
pub fn ct_eq(a: &str, b: &str) -> bool {
    use subtle::ConstantTimeEq;
    a.as_bytes().ct_eq(b.as_bytes()).into()
}

/// Boxed future returned by an async authentication callback.
pub type AuthFuture = Pin<Box<dyn Future<Output = bool> + Send>>;

type AuthFn = Arc<dyn Fn(String, String) -> AuthFuture + Send + Sync>;

fn extract_basic_credentials(req: &Request<Body>) -> Option<(String, String)> {
    let header = req.headers().get("Authorization")?.to_str().ok()?;
    let b64 = header.strip_prefix("Basic ")?;
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD.decode(b64).ok()?;
    let decoded = String::from_utf8(bytes).ok()?;
    let (user, pass) = decoded.split_once(':')?;
    Some((user.to_owned(), pass.to_owned()))
}

fn unauthorized_response() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [("WWW-Authenticate", "Basic realm=\"ddevmem register map\"")],
        "Unauthorized",
    )
        .into_response()
}

async fn index_page() -> Html<&'static str> {
    // Minified at build time by build.rs; source lives in src/web_ui.html.
    Html(include_str!(concat!(env!("OUT_DIR"), "/web_ui.min.html")))
}

// ─── Builder ─────────────────────────────────────────────────────────────────

type DynMap = Arc<Mutex<dyn RegisterMapInfo + Send>>;

struct WebUiState {
    maps: Vec<(String, DynMap)>,
    auth: Option<AuthFn>,
    title: Option<String>,
}

impl Clone for WebUiState {
    fn clone(&self) -> Self {
        Self {
            maps: self.maps.clone(),
            auth: self.auth.clone(),
            title: self.title.clone(),
        }
    }
}

/// Builder for hosting one or more register maps as a web UI.
///
/// Each map is exposed under a URL slug. The resulting [`Router`] has no root
/// path baked in and can be nested under any prefix.
///
/// ```rust,no_run
/// # use std::sync::Arc;
/// # use tokio::sync::Mutex;
/// # use ddevmem::{register_map, DevMem};
/// # use ddevmem::web::WebUi;
/// # register_map! { pub unsafe map Spi (u32) { 0x00 => rw cr: u32 } }
/// # register_map! { pub unsafe map Gpio (u32) { 0x00 => rw data: u32 } }
/// # async fn run() {
/// # let d1 = unsafe { DevMem::new(0x0, Some(256)).unwrap() };
/// # let d2 = unsafe { DevMem::new(0x0, Some(256)).unwrap() };
/// # let spi = unsafe { Spi::new(Arc::new(d1)).unwrap() };
/// # let gpio = unsafe { Gpio::new(Arc::new(d2)).unwrap() };
/// let app = axum::Router::new().nest(
///     "/hw/regs",
///     WebUi::new()
///         .add("spi", Arc::new(Mutex::new(spi)))
///         .add("gpio", Arc::new(Mutex::new(gpio)))
///         .with_auth(|u, p| async move { u == "admin" && p == "secret" })
///         .build(),
/// );
/// # }
/// ```
pub struct WebUi {
    maps: Vec<(String, DynMap)>,
    auth: Option<AuthFn>,
    title: Option<String>,
}

impl Default for WebUi {
    fn default() -> Self {
        Self::new()
    }
}

impl WebUi {
    /// Create an empty builder.
    pub fn new() -> Self {
        Self {
            maps: Vec::new(),
            auth: None,
            title: None,
        }
    }

    /// Register a map under the given URL slug (e.g. `"spi"`, `"gpio"`).
    ///
    /// The slug must consist of ASCII alphanumerics, hyphens, or underscores.
    ///
    /// # Panics
    ///
    /// Panics if `slug` is empty or contains characters other than
    /// `[a-zA-Z0-9_-]`.
    pub fn add<T: RegisterMapInfo + Send + 'static>(
        mut self,
        slug: &str,
        regs: Arc<Mutex<T>>,
    ) -> Self {
        assert!(
            !slug.is_empty()
                && slug
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'),
            "slug must be non-empty ASCII [a-zA-Z0-9_-], got: {slug:?}"
        );
        self.maps.push((slug.to_owned(), regs as DynMap));
        self
    }

    /// Require HTTP Basic authentication on every endpoint. `check` is an
    /// async callback that receives `(username, password)` and must resolve
    /// to `true` to allow access. Because it is async, it can perform I/O
    /// such as querying a database or an external auth service.
    ///
    /// ```rust,no_run
    /// # use std::sync::Arc;
    /// # use tokio::sync::Mutex;
    /// # use ddevmem::{register_map, DevMem};
    /// # use ddevmem::web::WebUi;
    /// # register_map! { pub unsafe map R (u32) { 0x00 => rw x: u32 } }
    /// # async fn lookup_user(_u: &str, _p: &str) -> bool { true }
    /// # async fn run() {
    /// # let d = unsafe { DevMem::new(0x0, Some(256)).unwrap() };
    /// # let r = unsafe { R::new(Arc::new(d)).unwrap() };
    /// let app = WebUi::new()
    ///     .add("r", Arc::new(Mutex::new(r)))
    ///     .with_auth(|user, pass| async move {
    ///         lookup_user(&user, &pass).await
    ///     })
    ///     .build();
    /// # }
    /// ```
    pub fn with_auth<F, Fut>(mut self, check: F) -> Self
    where
        F: Fn(String, String) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = bool> + Send + 'static,
    {
        self.auth = Some(Arc::new(move |u, p| Box::pin(check(u, p)) as AuthFuture));
        self
    }

    /// Override the title shown in the browser tab and in the page header.
    ///
    /// When unset, the UI falls back to the built-in default
    /// (`"ddevmem — Register Maps"` in multi-map mode, or the map's own
    /// name in single-map mode).
    ///
    /// ```rust,no_run
    /// # use std::sync::Arc;
    /// # use tokio::sync::Mutex;
    /// # use ddevmem::{register_map, DevMem};
    /// # use ddevmem::web::WebUi;
    /// # register_map! { pub unsafe map R (u32) { 0x00 => rw x: u32 } }
    /// # async fn run() {
    /// # let d = unsafe { DevMem::new(0x0, Some(256)).unwrap() };
    /// # let r = unsafe { R::new(Arc::new(d)).unwrap() };
    /// let app = WebUi::new()
    ///     .add("r", Arc::new(Mutex::new(r)))
    ///     .with_title("Acme SoC — Hardware Registers")
    ///     .build();
    /// # }
    /// ```
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Consume the builder and produce an [`axum::Router`].
    ///
    /// The router serves:
    /// - `GET /` — single HTML page showing all maps
    /// - `GET /api/maps` — `{ title?: String, maps: [{ slug, name }, ...] }`
    /// - `GET /api/{slug}/info` — register metadata (name, base, registers)
    /// - `POST /api/{slug}/read` — body `{ offset }`, returns `{ value }`
    /// - `POST /api/{slug}/write` — body `{ offset, value }`, returns `200 OK`
    pub fn build(self) -> Router {
        let state = WebUiState {
            maps: self.maps,
            auth: self.auth,
            title: self.title,
        };

        let api = Router::new()
            .route("/maps", get(api_list))
            .route("/{slug}/info", get(api_info))
            .route("/{slug}/read", post(api_read))
            .route("/{slug}/write", post(api_write));

        Router::new()
            .route("/", get(index_page))
            .nest("/api", api)
            .layer(middleware::from_fn_with_state(
                state.clone(),
                auth_middleware,
            ))
            .with_state(state)
    }
}

async fn auth_middleware(
    State(state): State<WebUiState>,
    req: Request<Body>,
    next: Next,
) -> Response {
    if let Some(check) = state.auth.clone() {
        let creds = extract_basic_credentials(&req);
        let allowed = match creds {
            Some((u, p)) => check(u, p).await,
            None => false,
        };
        if !allowed {
            return unauthorized_response();
        }
    }
    next.run(req).await
}

#[derive(Serialize)]
struct MapEntry {
    slug: String,
    name: String,
}

#[derive(Serialize)]
struct MapList {
    /// User-supplied title (set via [`WebUi::with_title`]). `None` lets
    /// the front-end pick its built-in default.
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    maps: Vec<MapEntry>,
}

async fn api_list(State(state): State<WebUiState>) -> Json<MapList> {
    let mut entries = Vec::with_capacity(state.maps.len());
    for (slug, regs) in &state.maps {
        let regs = regs.lock().await;
        entries.push(MapEntry {
            slug: slug.clone(),
            name: regs.map_name().to_owned(),
        });
    }
    Json(MapList {
        title: state.title.clone(),
        maps: entries,
    })
}

fn find_map<'a>(maps: &'a [(String, DynMap)], slug: &str) -> Result<&'a DynMap, StatusCode> {
    maps.iter()
        .find(|(s, _)| *s == slug)
        .map(|(_, regs)| regs)
        .ok_or(StatusCode::NOT_FOUND)
}

async fn api_info(
    State(state): State<WebUiState>,
    axum::extract::Path(slug): axum::extract::Path<String>,
) -> Result<Json<RegisterMapDescription>, StatusCode> {
    let regs = find_map(&state.maps, &slug)?;
    let regs = regs.lock().await;
    Ok(Json(RegisterMapDescription {
        name: regs.map_name(),
        bus_width: regs.bus_width(),
        base_address: regs.base_address(),
        registers: regs.registers(),
    }))
}

async fn api_read(
    State(state): State<WebUiState>,
    axum::extract::Path(slug): axum::extract::Path<String>,
    Json(req): Json<ReadReq>,
) -> Result<Json<ReadResp>, StatusCode> {
    let regs = find_map(&state.maps, &slug)?;
    let regs = regs.lock().await;
    regs.read_register(req.offset)
        .map(|value| Json(ReadResp { value }))
        .ok_or(StatusCode::BAD_REQUEST)
}

async fn api_write(
    State(state): State<WebUiState>,
    axum::extract::Path(slug): axum::extract::Path<String>,
    Json(req): Json<WriteReq>,
) -> Result<StatusCode, StatusCode> {
    let regs = find_map(&state.maps, &slug)?;
    let mut regs = regs.lock().await;
    regs.write_register(req.offset, req.value)
        .map(|()| StatusCode::OK)
        .ok_or(StatusCode::BAD_REQUEST)
}
