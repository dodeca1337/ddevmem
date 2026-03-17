//! Example: two identical register map structs with different base addresses,
//! plus typed bitfields (bool, u8, enum) with a web UI.
//!
//! Run with:
//!   cargo run --example multi_same_map --no-default-features --features "emulator,web"
//!
//! Then open http://localhost:3000/hw/ and verify:
//!   - Two "TimerRegs" sections appear with different base addresses.
//!   - Enum fields show dropdown selectors.
//!   - Bool fields show true/false dropdowns.

use std::sync::Arc;

use ddevmem::{register_map, DevMem};
use tokio::sync::Mutex;

register_map! {
    /// Timer controller.
    pub unsafe map TimerRegs (u32) {
        0x00 =>
            /// Timer control register.
            rw cr: u32 {
                /// Timer enable flag.
                enable: 0 as bool,
                /// One-pulse mode.
                one_pulse: 1 as bool,
                /// Clock prescaler (0–15).
                psc: 2..=5 as u8,
                /// Operating mode.
                mode: 6..=7 as enum TimerMode {
                    Stopped  = 0,
                    OneShot  = 1,
                    FreeRun  = 2,
                    External = 3,
                }
            },
        0x04 =>
            /// Timer status register.
            ro sr: u32 {
                /// Counter active flag.
                active: 0 as bool,
                /// Overflow flag.
                overflow: 1 as bool
            },
        0x08 =>
            /// Counter value.
            rw cnt: u32
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    // Two timer peripherals at different physical addresses.
    let tim1_mem = unsafe { DevMem::new(0x4000_0000, Some(256)).unwrap() };
    let tim2_mem = unsafe { DevMem::new(0x4000_1000, Some(256)).unwrap() };

    let mut tim1 = unsafe { TimerRegs::new(Arc::new(tim1_mem)).unwrap() };
    let mut tim2 = unsafe { TimerRegs::new(Arc::new(tim2_mem)).unwrap() };

    // Give them different initial values.
    tim1.set_cr_enable(true);
    tim1.set_cr_mode(TimerMode::FreeRun);
    tim1.set_cnt(1000);

    tim2.set_cr_enable(false);
    tim2.set_cr_mode(TimerMode::External);
    tim2.set_cnt(42);

    let app = axum::Router::new().nest(
        "/hw",
        ddevmem::web::multi_router()
            .add("timer1", Arc::new(Mutex::new(tim1)))
            .add("timer2", Arc::new(Mutex::new(tim2)))
            .build(),
    );

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    println!("Multi-map web UI at http://localhost:3000/hw/");
    println!("  timer1 @ 0x4000_0000, timer2 @ 0x4000_1000");
    axum::serve(listener, app).await.unwrap();
}
