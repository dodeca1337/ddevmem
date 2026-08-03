//! HTTP-level tests for the web UI (feature `web`, emulator backend).
//!
//! Run with `cargo test --features web`.
#![cfg(feature = "web")]

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use ddevmem::web::{ct_eq, WebUi};
use ddevmem::{register_map, DevMem};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tokio::sync::Mutex;
use tower::ServiceExt;

register_map! {
    /// Timer block.
    pub unsafe map Timer (u32) {
        0x00 =>
            /// Control register.
            rw cr: u32 {
                /// Enable.
                enable: 0 as bool,
                /// Mode.
                mode: 1..=2 as enum Mode {
                    Off = 0,
                    Slow = 1,
                    Fast = 2,
                }
            },
        0x04 => ro sr: u32,
        0x08 => wo cmd: u32,
        0x10 => rw fifo: [u32; 2],
        // 0x18 is deliberately left unmapped — `respects_declared_offsets_and_access`
        // uses it to check that the fifo array does not extend past its length.
        0x20 =>
            /// Mixed-access: a w1c flag next to rw configuration.
            rw isr: u32 {
                w1c pending: 0 as bool,
                ro rev: 8..=15 as u8,
                mask: 16..=19 as u8
            }
    }
}

fn test_router() -> Router {
    let mem = Arc::new(unsafe { DevMem::new(0x4000_0000, Some(256)).unwrap() });
    let regs = unsafe { Timer::new(mem).unwrap() };
    WebUi::new().add("timer", Arc::new(Mutex::new(regs))).build()
}

async fn send(router: &Router, request: Request<Body>) -> (StatusCode, Value) {
    let response = router.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, body)
}

async fn get(router: &Router, path: &str) -> (StatusCode, Value) {
    send(router, Request::get(path).body(Body::empty()).unwrap()).await
}

async fn post(router: &Router, path: &str, body: Value) -> (StatusCode, Value) {
    let request = Request::post(path)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    send(router, request).await
}

#[tokio::test]
async fn serves_the_ui_page() {
    let router = test_router();
    let response = router
        .oneshot(Request::get("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let html = response.into_body().collect().await.unwrap().to_bytes();
    assert!(std::str::from_utf8(&html).unwrap().contains("ddevmem"));
}

#[tokio::test]
async fn lists_maps() {
    let (status, body) = get(&test_router(), "/api/maps").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["maps"], json!([{ "slug": "timer", "name": "Timer" }]));
    assert!(body.get("title").is_none());
}

#[tokio::test]
async fn custom_title_is_reported() {
    let mem = Arc::new(unsafe { DevMem::new(0, Some(256)).unwrap() });
    let regs = unsafe { Timer::new(mem).unwrap() };
    let router = WebUi::new()
        .with_title("Bench #3")
        .add("t", Arc::new(Mutex::new(regs)))
        .build();

    let (_, body) = get(&router, "/api/maps").await;
    assert_eq!(body["title"], "Bench #3");
}

#[tokio::test]
async fn info_describes_registers_and_expands_arrays() {
    let (status, body) = get(&test_router(), "/api/timer/info").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["name"], "Timer");
    assert_eq!(body["bus_width"], 4);
    assert_eq!(body["base_address"], 0x4000_0000u64);

    let registers = body["registers"].as_array().unwrap();
    let names: Vec<_> = registers.iter().map(|r| r["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["cr", "sr", "cmd", "fifo[0]", "fifo[1]", "isr"]);

    let cr = &registers[0];
    assert_eq!(cr["access"], "rw");
    assert_eq!(cr["width"], 32);
    assert_eq!(cr["doc"], "Control register.");

    let bitfields = cr["bitfields"].as_array().unwrap();
    assert_eq!(bitfields[0]["name"], "enable");
    assert_eq!(bitfields[0]["field_type"], "bool");
    assert_eq!(
        bitfields[0]["variants"],
        json!([{ "name": "false", "value": 0 }, { "name": "true", "value": 1 }])
    );
    assert_eq!(bitfields[1]["field_type"], "Mode");
    assert_eq!(bitfields[1]["lo"], 1);
    assert_eq!(bitfields[1]["hi"], 2);
    assert_eq!(bitfields[1]["variants"].as_array().unwrap().len(), 3);

    assert_eq!(registers[4]["offset"], 0x14);

    // A field's own access is reported so the UI can offer Clear instead of
    // Set, and hide the write control of a read-only field.
    let isr = &registers[5];
    let access: Vec<_> = isr["bitfields"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| (b["name"].as_str().unwrap(), b["access"].as_str().unwrap()))
        .collect();
    assert_eq!(
        access,
        [("pending", "w1c"), ("rev", "ro"), ("mask", "rw")]
    );
    // Fields without a modifier report the register's own kind.
    assert_eq!(registers[0]["bitfields"][0]["access"], "rw");
}

#[tokio::test]
async fn write_then_read_roundtrip() {
    let router = test_router();

    let (status, _) = post(&router, "/api/timer/write", json!({ "offset": 0, "value": 5 })).await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = post(&router, "/api/timer/read", json!({ "offset": 0 })).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["value"], 5);
    assert_eq!(body["hex"], "0x5");
}

#[tokio::test]
async fn write_accepts_hex_and_decimal_strings() {
    let router = test_router();

    let (status, _) =
        post(&router, "/api/timer/write", json!({ "offset": 0, "value": "0x2A" })).await;
    assert_eq!(status, StatusCode::OK);
    let (_, body) = post(&router, "/api/timer/read", json!({ "offset": 0 })).await;
    assert_eq!(body["value"], 42);

    let (status, _) =
        post(&router, "/api/timer/write", json!({ "offset": 0, "value": "7" })).await;
    assert_eq!(status, StatusCode::OK);
    let (_, body) = post(&router, "/api/timer/read", json!({ "offset": 0 })).await;
    assert_eq!(body["value"], 7);
}

#[tokio::test]
async fn rejects_bad_values() {
    let router = test_router();
    for value in [json!("zzz"), json!("0x"), json!(-1), json!("0x1_0000_0000")] {
        let (status, _) =
            post(&router, "/api/timer/write", json!({ "offset": 0, "value": value })).await;
        assert_ne!(status, StatusCode::OK, "value {value} must be rejected");
    }
    // Larger than the u32 bus.
    let (status, _) =
        post(&router, "/api/timer/write", json!({ "offset": 0, "value": "0x100000000" })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn respects_declared_offsets_and_access() {
    let router = test_router();

    // Undeclared offset.
    let (status, _) = post(&router, "/api/timer/read", json!({ "offset": 0x0C })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // Misaligned offset inside a register.
    let (status, _) = post(&router, "/api/timer/read", json!({ "offset": 2 })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // Reading a write-only register.
    let (status, _) = post(&router, "/api/timer/read", json!({ "offset": 0x08 })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // Writing a read-only register.
    let (status, _) =
        post(&router, "/api/timer/write", json!({ "offset": 0x04, "value": 1 })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Array elements are addressable...
    let (status, _) = post(&router, "/api/timer/read", json!({ "offset": 0x14 })).await;
    assert_eq!(status, StatusCode::OK);
    // ...but the run ends after the declared count.
    let (status, _) = post(&router, "/api/timer/read", json!({ "offset": 0x18 })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn unknown_slug_is_not_found() {
    let (status, _) = get(&test_router(), "/api/nope/info").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn basic_auth_guards_every_endpoint() {
    let mem = Arc::new(unsafe { DevMem::new(0, Some(256)).unwrap() });
    let regs = unsafe { Timer::new(mem).unwrap() };
    let router = WebUi::new()
        .add("timer", Arc::new(Mutex::new(regs)))
        .with_auth(|user, pass| async move { ct_eq(&user, "admin") & ct_eq(&pass, "secret") })
        .build();

    // No credentials.
    let response = router
        .clone()
        .oneshot(Request::get("/api/maps").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(response.headers().contains_key("WWW-Authenticate"));

    // Wrong credentials ("admin:wrong").
    let response = router
        .clone()
        .oneshot(
            Request::get("/api/maps")
                .header("Authorization", "Basic YWRtaW46d3Jvbmc=")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    // Correct credentials ("admin:secret").
    let response = router
        .clone()
        .oneshot(
            Request::get("/api/maps")
                .header("Authorization", "Basic YWRtaW46c2VjcmV0")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[test]
fn constant_time_comparison() {
    assert!(ct_eq("hunter2", "hunter2"));
    assert!(!ct_eq("hunter2", "hunter3"));
    assert!(!ct_eq("admin", "administrator"));
    assert!(!ct_eq("", "x"));
    assert!(ct_eq("", ""));
}
