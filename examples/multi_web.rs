//! Example: multiple register maps on a single web page.
//!
//! Run with:
//!   cargo run --example multi_web --no-default-features --features "emulator,web"
//!
//! Then open http://localhost:3000/hw/ to see all maps on one page.

use std::sync::Arc;

use ddevmem::{register_map, DevMem};
use tokio::sync::Mutex;

register_map! {
    /// SPI controller.
    pub unsafe map SpiRegs (u32) {
        0x00 =>
            /// SPI control register.
            rw cr: u32 {
                /// Chip select (0–7).
                cs:     0..=2,
                /// Clock polarity.
                cpol:   3,
                /// Clock phase.
                cpha:   4,
                /// Transfer enable.
                enable: 5
            },
        0x04 =>
            /// SPI status register.
            ro sr: u32 {
                /// TX FIFO empty.
                txe:  0,
                /// RX FIFO not empty.
                rxne: 1,
                /// Busy flag.
                busy: 7
            },
        0x08 =>
            /// SPI data register.
            rw dr: u32
    }
}

register_map! {
    /// GPIO controller.
    pub unsafe map GpioRegs (u32) {
        0x00 =>
            /// GPIO data register.
            rw data: u32,
        0x04 =>
            /// GPIO direction (0 = input, 1 = output per bit).
            rw dir: u32,
        0x08 =>
            /// GPIO interrupt enable.
            rw ier: u32,
        0x0C =>
            /// GPIO interrupt status (write-1-to-clear).
            rw isr: u32
    }
}

register_map! {
    /// PWM controller.
    pub unsafe map PwmRegs (u32) {
        0x00 =>
            /// PWM control register.
            rw cr: u32 {
                /// Channel enable bits.
                ch_en: 0..=3,
                /// Prescaler.
                psc:   4..=6
            },
        0x04 =>
            /// PWM period (ticks).
            rw period: u32,
        0x08 =>
            /// PWM duty cycle.
            rw duty: u32
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    // Create three independent DevMem regions (emulator mode).
    let spi_mem = unsafe { DevMem::new(0x0, Some(256)).unwrap() };
    let gpio_mem = unsafe { DevMem::new(0x0, Some(256)).unwrap() };
    let pwm_mem = unsafe { DevMem::new(0x0, Some(256)).unwrap() };

    let mut spi = unsafe { SpiRegs::new(Arc::new(spi_mem)).unwrap() };
    let gpio = unsafe { GpioRegs::new(Arc::new(gpio_mem)).unwrap() };
    let mut pwm = unsafe { PwmRegs::new(Arc::new(pwm_mem)).unwrap() };

    // Pre-populate some values.
    spi.set_cr_cs(2);
    spi.set_cr_enable(1);
    pwm.set_period(1000);
    pwm.set_duty(250);

    // The multi_router() builder produces a Router with no fixed root path,
    // so it can be nested under any prefix on a larger server.
    let regs_router = ddevmem::web::multi_router()
        .add("spi", Arc::new(Mutex::new(spi)))
        .add("gpio", Arc::new(Mutex::new(gpio)))
        .add("pwm", Arc::new(Mutex::new(pwm)))
        .build();

    let app = axum::Router::new().nest("/hw", regs_router);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    println!("Multi-map web UI at http://localhost:3000/hw/");
    axum::serve(listener, app).await.unwrap();
}
