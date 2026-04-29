//! Example: web UI with HTTP Basic authentication.
//!
//! Run with:
//!   cargo run --example web_auth --no-default-features --features "emulator,web"
//!
//! Then open http://localhost:3000 — the browser will prompt for credentials.
//! Use admin / secret.

use std::sync::Arc;

use ddevmem::{register_map, DevMem};
use tokio::sync::Mutex;

register_map! {
    /// GPIO controller.
    pub unsafe map GpioRegs (u32) {
        0x00 =>
            /// GPIO data register (directly controls pin state).
            rw data: u32,
        0x04 =>
            /// GPIO direction register (0 = input, 1 = output per bit).
            rw dir: u32,
        0x08 =>
            /// GPIO interrupt enable (one bit per pin).
            rw ier: u32,
        0x0C =>
            /// GPIO interrupt status (write-1-to-clear).
            rw isr: u32
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let devmem = unsafe { DevMem::new(0x0, Some(256)).unwrap() };
    let regs = unsafe { GpioRegs::new(Arc::new(devmem)).unwrap() };
    let regs = Arc::new(Mutex::new(regs));

    // Authenticate with static credentials. Use `ct_eq` (constant-time
    // comparison) instead of `==` so response timing does not leak the
    // password, and bitwise `&` instead of `&&` to evaluate both checks
    // unconditionally.
    let app = axum::Router::new().nest(
        "/",
        ddevmem::web::WebUi::new()
            .add("gpio", regs)
            .with_auth(|user, pass| async move {
                // `with_auth` is async, so a real implementation could query a
                // database or remote auth service here.
                ddevmem::web::ct_eq(&user, "admin") & ddevmem::web::ct_eq(&pass, "secret")
            })
            .build(),
    );

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    println!("Register map web UI (with auth) at http://localhost:3000");
    println!("Credentials: admin / secret");
    axum::serve(listener, app).await.unwrap();
}
