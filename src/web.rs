//! Optional web UI for viewing and editing register maps.
//!
//! Enable the `web` feature to pull in [`axum`] and get a ready-made HTTP
//! interface for any type implementing [`RegisterMapInfo`] (implemented
//! automatically by [`register_map!`](crate::register_map)).
//!
//! [`WebUi`] is a builder that hosts one or more register maps on a single
//! page. The resulting [`Router`] has **no root path baked in**, so it can be
//! nested freely under any prefix with [`Router::nest`]. Typed bitfields
//! (`as bool`, `as enum`) render as dropdown selectors instead of numeric
//! inputs.
//!
//! Reads and writes are validated against the declared register map: only
//! offsets that belong to a readable (respectively writable) register are
//! accepted, so a stray HTTP request cannot poke undeclared device memory or
//! write to a read-only register.
//!
//! # Security
//!
//! [`WebUi::with_auth`] uses HTTP Basic authentication. Credentials travel
//! `base64`-encoded — **not** encrypted. The UI is intended for trusted
//! networks (lab benches, internal VLANs, SSH tunnels); for anything exposed,
//! terminate TLS in front of the server (`nginx`, `caddy`, or `axum-server` +
//! `rustls`).
//!
//! Compare secrets in **constant time** — a naive `==` leaks the password
//! through response timing. [`ct_eq`] is provided for exactly that, and the
//! bitwise `&` (not `&&`) keeps both comparisons unconditional:
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
//! let app = WebUi::new()
//!     .add("r", Arc::new(Mutex::new(regs)))
//!     .with_auth(|user, pass| async move {
//!         ct_eq(&user, "admin") & ct_eq(&pass, "hunter2")
//!     })
//!     .build();
//! # }
//! ```
//!
//! There is **no built-in CSRF protection or rate limiting**. If an
//! authenticated browser session may also visit untrusted origins, put the
//! deployment behind a reverse proxy that enforces `Origin`/`Referer` checks
//! and rate-limits failed authentications.
//!
//! # Example
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
//! // Any number of maps can be added; all appear on one page.
//! let app = axum::Router::new().nest(
//!     "/registers",
//!     WebUi::new()
//!         .add("axi", Arc::new(Mutex::new(regs)))
//!         .build(),
//! );
//!
//! let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
//! axum::serve(listener, app).await.unwrap();
//! # }
//! ```

use axum::{
    body::Body,
    extract::{Path, State},
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

// ─── Register metadata (as served to the UI) ─────────────────────────────────

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
    /// Effective access of this field.
    ///
    /// Usually the register's own kind, but a field may narrow it: `"ro"`
    /// (never written), `"wo"` (never read), or `"w1c"` (write-1-to-clear —
    /// the UI offers a *Clear* action instead of a value to set).
    pub access: &'static str,
    /// Type hint: `"raw"`, `"bool"`, an integer type name, or an enum name.
    pub field_type: &'static str,
    /// Enum/bool variants (empty for plain integer fields).
    pub variants: Vec<VariantInfo>,
}

/// Description of a single register in the map.
///
/// Register arrays are expanded into one entry per element, named
/// `fifo[0]`, `fifo[1]`, … — hence the owned `String` name.
#[derive(Debug, Clone, Serialize)]
pub struct RegisterInfo {
    /// Register name.
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

/// Static register-map metadata emitted by `register_map!`.
///
/// Not public API — the macro generates tables of these types and the
/// library interprets them. Everything here may change between minor
/// versions.
#[doc(hidden)]
pub mod spec {
    use super::{BitfieldInfo, RegisterInfo, VariantInfo};

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Access {
        Rw,
        Ro,
        Wo,
    }

    impl Access {
        pub fn as_str(self) -> &'static str {
            match self {
                Access::Rw => "rw",
                Access::Ro => "ro",
                Access::Wo => "wo",
            }
        }

        pub fn can_read(self) -> bool {
            matches!(self, Access::Rw | Access::Ro)
        }

        pub fn can_write(self) -> bool {
            matches!(self, Access::Rw | Access::Wo)
        }
    }

    /// Ready-made variant table for `as bool` bitfields, in the
    /// `(name, value)` form of [`FieldValue::VARIANTS`](crate::FieldValue::VARIANTS).
    pub const BOOL_VARIANTS: &[(&str, u64)] = &[("false", 0), ("true", 1)];

    #[derive(Debug, Clone, Copy)]
    pub struct Bitfield {
        pub name: &'static str,
        pub doc: &'static str,
        pub lo: u32,
        pub hi: u32,
        /// Effective access of this field: `"rw"`, `"ro"`, `"wo"`, or `"w1c"`.
        pub access: &'static str,
        pub type_name: &'static str,
        pub variants: &'static [(&'static str, u64)],
    }

    #[derive(Debug, Clone, Copy)]
    pub struct Register {
        pub name: &'static str,
        pub doc: &'static str,
        pub offset: usize,
        pub access: Access,
        /// Register value width in bits.
        pub width_bits: usize,
        /// Number of consecutive elements (1 for scalar registers).
        pub count: usize,
        pub bitfields: &'static [Bitfield],
    }

    impl Register {
        /// Whether `offset` addresses one of this register's elements
        /// (elements are laid out `bus_width` bytes apart).
        fn contains(&self, bus_width: usize, offset: usize) -> bool {
            offset >= self.offset
                && (offset - self.offset).is_multiple_of(bus_width)
                && (offset - self.offset) / bus_width < self.count
        }
    }

    /// Expands a spec table into per-element [`RegisterInfo`] values.
    pub fn expand(specs: &'static [Register], bus_width: usize) -> Vec<RegisterInfo> {
        let mut registers = Vec::new();
        for spec in specs {
            for i in 0..spec.count {
                let name = if spec.count == 1 {
                    spec.name.to_owned()
                } else {
                    format!("{}[{}]", spec.name, i)
                };
                registers.push(RegisterInfo {
                    name,
                    doc: spec.doc,
                    offset: spec.offset + i * bus_width,
                    access: spec.access.as_str(),
                    width: spec.width_bits,
                    bitfields: spec
                        .bitfields
                        .iter()
                        .map(|bf| BitfieldInfo {
                            name: bf.name,
                            doc: bf.doc,
                            lo: bf.lo,
                            hi: bf.hi,
                            access: bf.access,
                            field_type: bf.type_name,
                            variants: bf
                                .variants
                                .iter()
                                .map(|&(name, value)| VariantInfo { name, value })
                                .collect(),
                        })
                        .collect(),
                });
            }
        }
        registers
    }

    /// Whether `offset` addresses a readable declared register.
    pub fn is_readable(specs: &[Register], bus_width: usize, offset: usize) -> bool {
        specs
            .iter()
            .any(|s| s.access.can_read() && s.contains(bus_width, offset))
    }

    /// Whether `offset` addresses a writable declared register.
    pub fn is_writable(specs: &[Register], bus_width: usize, offset: usize) -> bool {
        specs
            .iter()
            .any(|s| s.access.can_write() && s.contains(bus_width, offset))
    }
}

// ─── Trait ───────────────────────────────────────────────────────────────────

/// Register-map metadata and raw access for the web UI.
///
/// Implemented automatically by [`register_map!`](crate::register_map) when
/// the `web` feature is enabled.
pub trait RegisterMapInfo {
    /// Name of the register map (the struct name).
    fn map_name(&self) -> &'static str;

    /// Bus width in bytes.
    fn bus_width(&self) -> usize;

    /// Physical base address of the mapped region.
    fn base_address(&self) -> usize;

    /// Describes every declared register, including bitfields. Register
    /// arrays are expanded into one entry per element.
    fn registers(&self) -> Vec<RegisterInfo>;

    /// Reads the register at the given byte offset as `u64`.
    ///
    /// Returns `None` when `offset` does not address a readable declared
    /// register.
    fn read_register(&self, offset: usize) -> Option<u64>;

    /// Writes the register at the given byte offset from a `u64`.
    ///
    /// Returns `None` when `offset` does not address a writable declared
    /// register or `value` does not fit the bus width.
    fn write_register(&mut self, offset: usize, value: u64) -> Option<()>;
}

// ─── Wire types ──────────────────────────────────────────────────────────────

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

/// Register values are returned both as a JSON number and as a hex string:
/// JavaScript numbers lose precision above 2⁵³, so clients that may meet
/// 64-bit registers should parse `hex` (e.g. with `BigInt`).
#[derive(Serialize)]
struct ReadResp {
    value: u64,
    hex: String,
}

impl From<u64> for ReadResp {
    fn from(value: u64) -> Self {
        Self {
            value,
            hex: format!("{value:#x}"),
        }
    }
}

/// A `u64` that also accepts string forms (`"0x1F"`, `"42"`), again because
/// JSON numbers cannot carry the full 64-bit range through JavaScript.
#[derive(Deserialize)]
#[serde(untagged)]
enum WireValue {
    Int(u64),
    Str(String),
}

impl WireValue {
    fn to_u64(&self) -> Option<u64> {
        match self {
            WireValue::Int(v) => Some(*v),
            WireValue::Str(s) => {
                let s = s.trim();
                match s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
                    Some(hex) => u64::from_str_radix(hex, 16).ok(),
                    None => s.parse().ok(),
                }
            }
        }
    }
}

#[derive(Deserialize)]
struct WriteReq {
    offset: usize,
    value: WireValue,
}

// ─── Auth ────────────────────────────────────────────────────────────────────

/// Compares two strings in constant time.
///
/// Use this inside the [`WebUi::with_auth`] callback when checking passwords
/// or other secret material: a naive `==` exits at the first differing byte
/// and lets an attacker recover the secret byte by byte from response times.
///
/// Lengths are compared with a constant-time integer comparison and contents
/// up to `min(a.len(), b.len())`, so a length mismatch is indistinguishable
/// from a content mismatch. (Overall timing still correlates with the
/// *shorter* length — irrelevant when one side is your fixed secret.)
///
/// ```
/// # use ddevmem::web::ct_eq;
/// assert!(ct_eq("hunter2", "hunter2"));
/// assert!(!ct_eq("hunter2", "hunter3"));
/// assert!(!ct_eq("admin", "administrator"));
/// ```
///
/// Combine several checks with the bitwise `&` operator — `&&` would
/// short-circuit and re-introduce the timing leak:
///
/// ```ignore
/// ct_eq(user, "admin") & ct_eq(pass, "hunter2")
/// ```
pub fn ct_eq(a: &str, b: &str) -> bool {
    use subtle::{Choice, ConstantTimeEq};
    let a = a.as_bytes();
    let b = b.as_bytes();
    let len_eq: Choice = (a.len() as u64).ct_eq(&(b.len() as u64));
    let min = a.len().min(b.len());
    let content_eq: Choice = a[..min].ct_eq(&b[..min]);
    (len_eq & content_eq).into()
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

async fn auth_middleware(
    State(state): State<WebUiState>,
    req: Request<Body>,
    next: Next,
) -> Response {
    if let Some(check) = &state.auth {
        let allowed = match extract_basic_credentials(&req) {
            Some((user, pass)) => check(user, pass).await,
            None => false,
        };
        if !allowed {
            return unauthorized_response();
        }
    }
    next.run(req).await
}

// ─── Builder ─────────────────────────────────────────────────────────────────

type DynMap = Arc<Mutex<dyn RegisterMapInfo + Send>>;

#[derive(Clone)]
struct MapHandle {
    slug: String,
    /// Cached at `add()` time so `GET /api/maps` never locks.
    name: String,
    regs: DynMap,
}

#[derive(Clone)]
struct WebUiState {
    maps: Vec<MapHandle>,
    auth: Option<AuthFn>,
    title: Option<String>,
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
/// # use ddevmem::web::{WebUi, ct_eq};
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
///         .with_auth(|u, p| async move { ct_eq(&u, "admin") & ct_eq(&p, "secret") })
///         .build(),
/// );
/// # }
/// ```
#[derive(Default)]
pub struct WebUi {
    maps: Vec<MapHandle>,
    auth: Option<AuthFn>,
    title: Option<String>,
}

impl WebUi {
    /// Creates an empty builder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a map under the given URL slug (e.g. `"spi"`, `"gpio"`).
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
        // The server is not running yet, so the lock is uncontended; fall
        // back to the slug if the caller managed to hold the lock anyway.
        let name = regs
            .try_lock()
            .map(|guard| guard.map_name().to_owned())
            .unwrap_or_else(|_| slug.to_owned());
        self.maps.push(MapHandle {
            slug: slug.to_owned(),
            name,
            regs: regs as DynMap,
        });
        self
    }

    /// Requires HTTP Basic authentication on every endpoint.
    ///
    /// `check` receives `(username, password)` and must resolve to `true` to
    /// allow the request. It is async, so it may perform I/O — query a
    /// database, call an auth service, etc. For static credentials compare
    /// with [`ct_eq`].
    pub fn with_auth<F, Fut>(mut self, check: F) -> Self
    where
        F: Fn(String, String) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = bool> + Send + 'static,
    {
        self.auth = Some(Arc::new(move |user, pass| {
            Box::pin(check(user, pass)) as AuthFuture
        }));
        self
    }

    /// Overrides the title shown in the browser tab and the page header.
    ///
    /// When unset, the UI falls back to its built-in default
    /// (`"ddevmem — Register Maps"`).
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Consumes the builder and produces an [`axum::Router`] serving:
    ///
    /// | Method | Path                | Body                  | Response |
    /// |--------|---------------------|-----------------------|----------|
    /// | GET    | `/`                 | —                     | HTML single-page app |
    /// | GET    | `/api/maps`         | —                     | `{ title?, maps: [{ slug, name }] }` |
    /// | GET    | `/api/{slug}/info`  | —                     | `{ name, bus_width, base_address, registers }` |
    /// | POST   | `/api/{slug}/read`  | `{ offset }`          | `{ value, hex }` |
    /// | POST   | `/api/{slug}/write` | `{ offset, value }`   | `200 OK` |
    ///
    /// `value` in a write request may be a JSON number or a string
    /// (`"0x1F"` / `"42"`); the string form carries the full 64-bit range.
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

// ─── Handlers ────────────────────────────────────────────────────────────────

async fn index_page() -> Html<&'static str> {
    // Minified at build time by build.rs; the source lives in src/web_ui.html.
    Html(include_str!(concat!(env!("OUT_DIR"), "/web_ui.min.html")))
}

#[derive(Serialize)]
struct MapEntry {
    slug: String,
    name: String,
}

#[derive(Serialize)]
struct MapList {
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    maps: Vec<MapEntry>,
}

async fn api_list(State(state): State<WebUiState>) -> Json<MapList> {
    let maps = state
        .maps
        .iter()
        .map(|m| MapEntry {
            slug: m.slug.clone(),
            name: m.name.clone(),
        })
        .collect();
    Json(MapList {
        title: state.title.clone(),
        maps,
    })
}

fn find_map<'a>(maps: &'a [MapHandle], slug: &str) -> Result<&'a DynMap, StatusCode> {
    maps.iter()
        .find(|m| m.slug == slug)
        .map(|m| &m.regs)
        .ok_or(StatusCode::NOT_FOUND)
}

async fn api_info(
    State(state): State<WebUiState>,
    Path(slug): Path<String>,
) -> Result<Json<RegisterMapDescription>, StatusCode> {
    let regs = find_map(&state.maps, &slug)?.lock().await;
    Ok(Json(RegisterMapDescription {
        name: regs.map_name(),
        bus_width: regs.bus_width(),
        base_address: regs.base_address(),
        registers: regs.registers(),
    }))
}

async fn api_read(
    State(state): State<WebUiState>,
    Path(slug): Path<String>,
    Json(req): Json<ReadReq>,
) -> Result<Json<ReadResp>, StatusCode> {
    let regs = find_map(&state.maps, &slug)?.lock().await;
    regs.read_register(req.offset)
        .map(|value| Json(ReadResp::from(value)))
        .ok_or(StatusCode::BAD_REQUEST)
}

async fn api_write(
    State(state): State<WebUiState>,
    Path(slug): Path<String>,
    Json(req): Json<WriteReq>,
) -> Result<StatusCode, StatusCode> {
    let value = req.value.to_u64().ok_or(StatusCode::BAD_REQUEST)?;
    let mut regs = find_map(&state.maps, &slug)?.lock().await;
    regs.write_register(req.offset, value)
        .map(|()| StatusCode::OK)
        .ok_or(StatusCode::BAD_REQUEST)
}
