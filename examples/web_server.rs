//! Example: web UI for viewing and editing registers (emulator backend).
//!
//! Run with:
//!   cargo run --example web_server --no-default-features --features "emulator,web"
//!
//! Then open http://localhost:3000 in your browser.

use std::sync::Arc;

use ddevmem::{register_map, DevMem};
use tokio::sync::Mutex;

register_map! {
    /// PWM controller.
    pub unsafe map PwmRegs (u32) {
        0x00 =>
            /// PWM control register.
            rw cr: u32 {
                /// Channel enable bits (one per channel, 0–3).
                ch_en: 0..=3,
                /// Prescaler value (0 = /1, 1 = /2, … 7 = /128).
                psc:   4..=6
            },
        0x04 =>
            /// PWM period register (timer ticks).
            rw period: u32,
        0x08 =>
            /// PWM duty cycle register.
            rw duty: u32,
        0x0C =>
            /// PWM status register (read-only).
            ro sr: u32 {
                /// Currently running.
                running: 0
            }
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let devmem = unsafe { DevMem::new(0x0, Some(256)).unwrap() };
    let mut regs = unsafe { PwmRegs::new(Arc::new(devmem)).unwrap() };

    // Pre-populate some values so the UI shows something interesting.
    regs.set_cr_ch_en(0b0101);
    regs.set_cr_psc(3);
    regs.set_period(1000);
    regs.set_duty(250);

    let regs = Arc::new(Mutex::new(regs));

    // The router has no fixed root — nest it at any path.
    // Use .nest("/", ...) to serve from the root, or any other prefix.
    let app = axum::Router::new().nest("/", ddevmem::web::WebUi::new().add("pwm", regs).build());

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    println!("Register map web UI at http://localhost:3000/");
    axum::serve(listener, app).await.unwrap();
}
