//! Optional web UI for viewing and editing register maps.
//!
//! Enable the `web` feature to pull in [`axum`] and get a ready-made HTTP
//! interface for any type that implements [`RegisterMapInfo`] (automatically
//! derived by [`register_map!`]).
//!
//! Typed bitfields (`as bool`, `as enum`) are rendered as dropdown selectors
//! in the web UI instead of plain numeric inputs.
//!
//! The returned [`Router`] has **no root path baked in**, so it can be nested
//! freely under any prefix with [`Router::nest`].
//!
//! # Single map
//!
//! ```rust,no_run
//! use std::sync::Arc;
//! use tokio::sync::Mutex;
//! use ddevmem::{register_map, DevMem};
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
//! let regs = Arc::new(Mutex::new(regs));
//!
//! // Mount at any prefix:
//! let app = axum::Router::new()
//!     .nest("/registers/axi", ddevmem::web::router(regs));
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
//! # register_map! { pub unsafe map Spi (u32) { 0x00 => rw cr: u32 } }
//! # register_map! { pub unsafe map Gpio (u32) { 0x00 => rw data: u32 } }
//! # async fn run() {
//! # let d1 = unsafe { DevMem::new(0x0, Some(256)).unwrap() };
//! # let d2 = unsafe { DevMem::new(0x0, Some(256)).unwrap() };
//! # let spi = unsafe { Spi::new(Arc::new(d1)).unwrap() };
//! # let gpio = unsafe { Gpio::new(Arc::new(d2)).unwrap() };
//! let app = axum::Router::new().nest(
//!     "/hw",
//!     ddevmem::web::multi_router()
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
use std::sync::Arc;
use tokio::sync::Mutex;

// ─── Metadata types ──────────────────────────────────────────────────────────

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
    pub name: &'static str,
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

/// Full register map description returned by the API.
#[derive(Debug, Clone, Serialize)]
pub struct RegisterMapDescription {
    /// Name of the register map struct.
    pub name: &'static str,
    /// Bus width in bytes.
    pub bus_width: usize,
    /// Physical base address.
    pub base_address: usize,
    /// All registers.
    pub registers: Vec<RegisterInfo>,
}

// ─── Traits ──────────────────────────────────────────────────────────────────

/// Trait automatically implemented by [`register_map!`] when the `web` feature
/// is enabled. Provides register metadata and raw read/write access for the
/// web UI.
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

/// Extension trait that provides the axum [`Router`].
pub trait RegisterMapWeb: RegisterMapInfo + Send + 'static {}
impl<T: RegisterMapInfo + Send + 'static> RegisterMapWeb for T {}

// ─── Auth ────────────────────────────────────────────────────────────────────

/// A boxed authentication function: `(username, password) -> bool`.
type AuthFn = Arc<dyn Fn(&str, &str) -> bool + Send + Sync>;

fn check_basic_auth(auth: &AuthFn, req: &Request<Body>) -> bool {
    req.headers()
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Basic "))
        .and_then(|b64| {
            use base64::Engine;
            base64::engine::general_purpose::STANDARD.decode(b64).ok()
        })
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .map(|decoded| {
            if let Some((user, pass)) = decoded.split_once(':') {
                auth(user, pass)
            } else {
                false
            }
        })
        .unwrap_or(false)
}

fn unauthorized_response() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [("WWW-Authenticate", "Basic realm=\"ddevmem register map\"")],
        "Unauthorized",
    )
        .into_response()
}

// ─── Single-map router ──────────────────────────────────────────────────────

struct AppState<T: RegisterMapInfo + Send + 'static> {
    regs: Arc<Mutex<T>>,
    auth: Option<AuthFn>,
}

impl<T: RegisterMapInfo + Send + 'static> Clone for AppState<T> {
    fn clone(&self) -> Self {
        Self {
            regs: self.regs.clone(),
            auth: self.auth.clone(),
        }
    }
}

/// Create an axum [`Router`] serving the register-map web UI **without**
/// authentication.
///
/// The router has no root path — nest it wherever you like:
///
/// ```rust,ignore
/// let app = axum::Router::new()
///     .nest("/registers/pwm", ddevmem::web::router(regs));
/// ```
pub fn router<T: RegisterMapInfo + Send + 'static>(regs: Arc<Mutex<T>>) -> Router {
    build_single_router(AppState { regs, auth: None })
}

/// Create an axum [`Router`] serving the register-map web UI **with** HTTP
/// Basic authentication.
///
/// `check` receives `(username, password)` and must return `true` to allow
/// access.
pub fn router_with_auth<T, F>(regs: Arc<Mutex<T>>, check: F) -> Router
where
    T: RegisterMapInfo + Send + 'static,
    F: Fn(&str, &str) -> bool + Send + Sync + 'static,
{
    build_single_router(AppState {
        regs,
        auth: Some(Arc::new(check)),
    })
}

fn build_single_router<T: RegisterMapInfo + Send + 'static>(state: AppState<T>) -> Router {
    let api = Router::new()
        .route("/info", get(api_info::<T>))
        .route("/read", post(api_read::<T>))
        .route("/write", post(api_write::<T>));

    Router::new()
        .route("/", get(index_page))
        .nest("/api", api)
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth_middleware::<T>,
        ))
        .with_state(state)
}

async fn auth_middleware<T: RegisterMapInfo + Send + 'static>(
    State(state): State<AppState<T>>,
    req: Request<Body>,
    next: Next,
) -> Response {
    if let Some(ref check) = state.auth {
        if !check_basic_auth(check, &req) {
            return unauthorized_response();
        }
    }
    next.run(req).await
}

// ─── Single-map API handlers ────────────────────────────────────────────────

async fn api_info<T: RegisterMapInfo + Send + 'static>(
    State(state): State<AppState<T>>,
) -> Json<RegisterMapDescription> {
    let regs = state.regs.lock().await;
    Json(RegisterMapDescription {
        name: regs.map_name(),
        bus_width: regs.bus_width(),
        base_address: regs.base_address(),
        registers: regs.registers(),
    })
}

#[derive(Deserialize)]
struct ReadReq {
    offset: usize,
}

#[derive(Serialize)]
struct ReadResp {
    value: u64,
}

async fn api_read<T: RegisterMapInfo + Send + 'static>(
    State(state): State<AppState<T>>,
    Json(req): Json<ReadReq>,
) -> Result<Json<ReadResp>, StatusCode> {
    let regs = state.regs.lock().await;
    regs.read_register(req.offset)
        .map(|value| Json(ReadResp { value }))
        .ok_or(StatusCode::BAD_REQUEST)
}

#[derive(Deserialize)]
struct WriteReq {
    offset: usize,
    value: u64,
}

async fn api_write<T: RegisterMapInfo + Send + 'static>(
    State(state): State<AppState<T>>,
    Json(req): Json<WriteReq>,
) -> StatusCode {
    let mut regs = state.regs.lock().await;
    match regs.write_register(req.offset, req.value) {
        Some(()) => StatusCode::OK,
        None => StatusCode::BAD_REQUEST,
    }
}

async fn index_page() -> Html<&'static str> {
    Html(include_str!("web_ui.html"))
}

// ─── Multi-map support ──────────────────────────────────────────────────────

type DynMap = Arc<Mutex<dyn RegisterMapInfo + Send>>;

struct MultiMapState {
    maps: Vec<(String, DynMap)>,
    auth: Option<AuthFn>,
}

impl Clone for MultiMapState {
    fn clone(&self) -> Self {
        Self {
            maps: self.maps.clone(),
            auth: self.auth.clone(),
        }
    }
}

/// Builder for hosting multiple register maps on a single page.
///
/// All maps are displayed together on one page. The API uses per-map slugs
/// in the URL path (`/api/{slug}/info`, `/api/{slug}/read`, etc.).
///
/// The resulting router has no root path and can be nested freely:
///
/// ```rust,no_run
/// # use std::sync::Arc;
/// # use tokio::sync::Mutex;
/// # use ddevmem::{register_map, DevMem};
/// # register_map! { pub unsafe map Spi (u32) { 0x00 => rw cr: u32 } }
/// # register_map! { pub unsafe map Gpio (u32) { 0x00 => rw data: u32 } }
/// # async fn run() {
/// # let d1 = unsafe { DevMem::new(0x0, Some(256)).unwrap() };
/// # let d2 = unsafe { DevMem::new(0x0, Some(256)).unwrap() };
/// # let spi = unsafe { Spi::new(Arc::new(d1)).unwrap() };
/// # let gpio = unsafe { Gpio::new(Arc::new(d2)).unwrap() };
/// let regs_router = ddevmem::web::multi_router()
///     .add("spi", Arc::new(Mutex::new(spi)))
///     .add("gpio", Arc::new(Mutex::new(gpio)))
///     .build();
///
/// // Mount under a custom prefix:
/// let app = axum::Router::new()
///     .nest("/hw/regs", regs_router);
///
/// let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
/// axum::serve(listener, app).await.unwrap();
/// # }
/// ```
pub struct MultiMapBuilder {
    maps: Vec<(String, DynMap)>,
    auth: Option<AuthFn>,
}

/// Create a [`MultiMapBuilder`] for hosting several register maps **without**
/// authentication.
pub fn multi_router() -> MultiMapBuilder {
    MultiMapBuilder {
        maps: Vec::new(),
        auth: None,
    }
}

/// Create a [`MultiMapBuilder`] for hosting several register maps **with**
/// HTTP Basic authentication.
pub fn multi_router_with_auth<F>(check: F) -> MultiMapBuilder
where
    F: Fn(&str, &str) -> bool + Send + Sync + 'static,
{
    MultiMapBuilder {
        maps: Vec::new(),
        auth: Some(Arc::new(check)),
    }
}

impl MultiMapBuilder {
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

    /// Consume the builder and produce an axum [`Router`].
    ///
    /// The router serves:
    /// - `GET /` — single HTML page showing all maps
    /// - `GET /api/maps` — JSON list of `{ slug, name }`
    /// - `GET /api/{slug}/info` — register metadata
    /// - `POST /api/{slug}/read` — read a register
    /// - `POST /api/{slug}/write` — write a register
    pub fn build(self) -> Router {
        let state = MultiMapState {
            maps: self.maps,
            auth: self.auth,
        };

        let api = Router::new()
            .route("/maps", get(multi_api_list))
            .route("/{slug}/info", get(multi_api_info))
            .route("/{slug}/read", post(multi_api_read))
            .route("/{slug}/write", post(multi_api_write));

        Router::new()
            .route("/", get(multi_index_page))
            .nest("/api", api)
            .layer(middleware::from_fn_with_state(
                state.clone(),
                multi_auth_middleware,
            ))
            .with_state(state)
    }
}

// ─── Multi-map auth middleware ───────────────────────────────────────────────

async fn multi_auth_middleware(
    State(state): State<MultiMapState>,
    req: Request<Body>,
    next: Next,
) -> Response {
    if let Some(ref check) = state.auth {
        if !check_basic_auth(check, &req) {
            return unauthorized_response();
        }
    }
    next.run(req).await
}

// ─── Multi-map API handlers ─────────────────────────────────────────────────

#[derive(Serialize)]
struct MapEntry {
    slug: String,
    name: String,
}

async fn multi_api_list(State(state): State<MultiMapState>) -> Json<Vec<MapEntry>> {
    let mut entries = Vec::with_capacity(state.maps.len());
    for (slug, regs) in &state.maps {
        let regs = regs.lock().await;
        entries.push(MapEntry {
            slug: slug.clone(),
            name: regs.map_name().to_owned(),
        });
    }
    Json(entries)
}

fn find_map<'a>(maps: &'a [(String, DynMap)], slug: &str) -> Result<&'a DynMap, StatusCode> {
    maps.iter()
        .find(|(s, _)| *s == slug)
        .map(|(_, regs)| regs)
        .ok_or(StatusCode::NOT_FOUND)
}

async fn multi_api_info(
    State(state): State<MultiMapState>,
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

async fn multi_api_read(
    State(state): State<MultiMapState>,
    axum::extract::Path(slug): axum::extract::Path<String>,
    Json(req): Json<ReadReq>,
) -> Result<Json<ReadResp>, StatusCode> {
    let regs = find_map(&state.maps, &slug)?;
    let regs = regs.lock().await;
    regs.read_register(req.offset)
        .map(|value| Json(ReadResp { value }))
        .ok_or(StatusCode::BAD_REQUEST)
}

async fn multi_api_write(
    State(state): State<MultiMapState>,
    axum::extract::Path(slug): axum::extract::Path<String>,
    Json(req): Json<WriteReq>,
) -> Result<StatusCode, StatusCode> {
    let regs = find_map(&state.maps, &slug)?;
    let mut regs = regs.lock().await;
    regs.write_register(req.offset, req.value)
        .map(|()| StatusCode::OK)
        .ok_or(StatusCode::BAD_REQUEST)
}

// ─── Multi-map HTML page ────────────────────────────────────────────────────

async fn multi_index_page() -> Html<&'static str> {
    Html(include_str!("web_ui.html"))
}
