//! Example: multiple register maps on a single web page covering the full
//! range of register-map features supported by `ddevmem`.
//!
//! Demonstrated:
//!   - Three different bus widths: `u32` (UART), `u16` (ADC), `u8` (I²C).
//!   - All three access kinds: `rw`, `ro`, and `wo` (write-only / command).
//!   - Plain numeric bitfields (e.g. counters, addresses).
//!   - Typed bitfields: `as bool`, `as u8`, and `as enum`.
//!   - Read-only status registers with bool flags.
//!   - Write-only command registers and write-1-to-clear interrupt registers.
//!
//! Run with:
//!   cargo run --example multi_web --no-default-features --features "emulator,web"
//!
//! Then open http://localhost:8800/hw to see all three peripherals.

use std::sync::Arc;

use ddevmem::{register_map, DevMem};
use tokio::sync::Mutex;

// ---------------------------------------------------------------------------
// UART: 32-bit bus. Showcases bool / u8 / enum bitfields, write-only command
// register, and read-only status register with flag bits.
// ---------------------------------------------------------------------------
register_map! {
    /// UART controller.
    pub unsafe map UartRegs (u32) {
        0x00 =>
            /// UART control register.
            rw cr: u32 {
                /// Transmitter enable.
                tx_en: 0 as bool,
                /// Receiver enable.
                rx_en: 1 as bool,
                /// Number of stop bits.
                stop:  2..=3 as enum StopBits {
                    One        = 0,
                    OnePointFive = 1,
                    Two        = 2,
                },
                /// Parity mode.
                parity: 4..=5 as enum Parity {
                    None = 0,
                    Even = 1,
                    Odd  = 2,
                    Mark = 3,
                },
                /// Word length (5 to 9 bits).
                word_len: 6..=9 as u8,
                /// Hardware flow control (RTS/CTS).
                flow_ctl: 10 as bool
            },
        0x04 =>
            /// Baud rate divisor (system clock / divisor = baud rate).
            rw brd: u32,
        0x08 =>
            /// UART status register (read-only flags).
            ro sr: u32 {
                /// TX FIFO empty.
                tx_empty: 0 as bool,
                /// TX FIFO full.
                tx_full:  1 as bool,
                /// RX FIFO empty.
                rx_empty: 2 as bool,
                /// RX FIFO full.
                rx_full:  3 as bool,
                /// Parity error detected.
                parity_err: 4 as bool,
                /// Frame error detected.
                frame_err:  5 as bool,
                /// Number of bytes currently in RX FIFO (0–15).
                rx_count: 8..=11 as u8
            },
        0x0C =>
            /// Interrupt status (write 1 to clear the corresponding bit).
            rw isr: u32 {
                /// TX FIFO empty interrupt pending.
                tx_empty: 0 as bool,
                /// RX byte received interrupt pending.
                rx_byte: 1 as bool,
                /// Parity error interrupt pending.
                parity:  2 as bool,
                /// Frame error interrupt pending.
                frame:   3 as bool
            },
        0x10 =>
            /// Command register (write-only). Writing triggers an action; the
            /// hardware self-clears immediately.
            wo cmd: u32 {
                /// Reset transmitter (1 = reset).
                tx_reset: 0 as bool,
                /// Reset receiver (1 = reset).
                rx_reset: 1 as bool,
                /// Send break condition (1 = send).
                send_break: 2 as bool,
                /// Software-triggered abort.
                abort:    3 as bool
            },
        0x14 =>
            /// Transmit data register (write-only).
            wo txd: u32,
        0x18 =>
            /// Received data register (read-only).
            ro rxd: u32
    }
}

// ---------------------------------------------------------------------------
// ADC: 16-bit bus. Mix of rw configuration, ro samples, and a write-only
// trigger register.
// ---------------------------------------------------------------------------
register_map! {
    /// 16-bit ADC peripheral.
    pub unsafe map AdcRegs (u16) {
        0x00 =>
            /// ADC control register.
            rw cr: u16 {
                /// ADC enable.
                enable: 0 as bool,
                /// Continuous conversion (versus single-shot).
                continuous: 1 as bool,
                /// Channel selection (0–7).
                channel: 2..=4 as u8,
                /// Resolution.
                resolution: 5..=6 as enum AdcResolution {
                    Bits8  = 0,
                    Bits10 = 1,
                    Bits12 = 2,
                    Bits14 = 3,
                },
                /// Trigger source.
                trigger: 8..=9 as enum AdcTrigger {
                    Software = 0,
                    Timer1   = 1,
                    Timer2   = 2,
                    External = 3,
                }
            },
        0x02 =>
            /// Last conversion result (read-only).
            ro data: u16,
        0x04 =>
            /// Conversion threshold for the analog watchdog.
            rw threshold: u16,
        0x06 =>
            /// Status register.
            ro sr: u16 {
                /// Conversion in progress.
                busy: 0 as bool,
                /// New data ready.
                eoc:  1 as bool,
                /// Watchdog tripped.
                awd:  2 as bool
            },
        0x08 =>
            /// Trigger a software conversion (write any non-zero value).
            wo start: u16
    }
}

// ---------------------------------------------------------------------------
// I²C: 8-bit bus. Shows narrow registers, 7-bit address fields, and a
// write-only command register.
// ---------------------------------------------------------------------------
register_map! {
    /// I²C controller (8-bit register bus).
    pub unsafe map I2cRegs (u8) {
        0x00 =>
            /// I²C control register.
            rw cr: u8 {
                /// Peripheral enable.
                enable: 0 as bool,
                /// Master mode (vs. slave).
                master: 1 as bool,
                /// Clock speed.
                speed: 2..=3 as enum I2cSpeed {
                    Standard100k = 0,
                    Fast400k     = 1,
                    FastPlus1M   = 2,
                    HighSpeed3M  = 3,
                },
                /// Acknowledge enable.
                ack:    4 as bool
            },
        0x01 =>
            /// Own slave address (7-bit).
            rw oar: u8 {
                /// Slave address.
                addr: 0..=6 as u8
            },
        0x02 =>
            /// Status register.
            ro sr: u8 {
                /// Bus busy.
                busy: 0 as bool,
                /// Master mode active.
                msl:  1 as bool,
                /// Address matched (slave mode).
                addr: 2 as bool,
                /// Byte transfer finished.
                btf:  3 as bool,
                /// ACK failure.
                af:   4 as bool,
                /// Arbitration lost.
                arlo: 5 as bool
            },
        0x03 =>
            /// Data register.
            rw dr: u8,
        0x04 =>
            /// Command register (write-only).
            wo cmd: u8 {
                /// Generate START condition.
                start: 0 as bool,
                /// Generate STOP condition.
                stop:  1 as bool,
                /// Reset the peripheral.
                reset: 7 as bool
            }
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    // Allocate emulated memory regions for each peripheral.
    let uart_mem = unsafe { DevMem::new(0x43D8_0000, Some(256)).unwrap() };
    let adc_mem = unsafe { DevMem::new(0x83C1_0000, Some(256)).unwrap() };
    let i2c_mem = unsafe { DevMem::new(0x83B4_0000, Some(256)).unwrap() };

    let mut uart = unsafe { UartRegs::new(Arc::new(uart_mem)).unwrap() };
    let mut adc = unsafe { AdcRegs::new(Arc::new(adc_mem)).unwrap() };
    let mut i2c = unsafe { I2cRegs::new(Arc::new(i2c_mem)).unwrap() };

    // Pre-populate values so the UI shows interesting defaults.
    uart.set_cr_tx_en(true);
    uart.set_cr_rx_en(true);
    uart.set_cr_word_len(8);
    uart.set_cr_parity(Parity::Even);
    uart.set_cr_stop(StopBits::One);
    uart.set_brd(115_200);

    adc.set_cr_enable(true);
    adc.set_cr_continuous(true);
    adc.set_cr_channel(3);
    adc.set_cr_resolution(AdcResolution::Bits12);
    adc.set_cr_trigger(AdcTrigger::Timer1);
    adc.set_threshold(2048);

    i2c.set_cr_enable(true);
    i2c.set_cr_master(true);
    i2c.set_cr_speed(I2cSpeed::Fast400k);
    i2c.set_cr_ack(true);
    i2c.set_oar_addr(0x42);

    let regs_router = ddevmem::web::WebUi::new()
        .add("uart", Arc::new(Mutex::new(uart)))
        .add("adc", Arc::new(Mutex::new(adc)))
        .add("i2c", Arc::new(Mutex::new(i2c)))
        .build();

    let app = axum::Router::new().nest("/hw", regs_router);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:8800").await.unwrap();
    println!("Multi-map web UI at http://localhost:8800/hw");
    println!("  uart (u32 bus)  — typed bitfields, wo command + txd, ro rxd");
    println!("  adc  (u16 bus)  — enum trigger/resolution, ro data, wo start");
    println!("  i2c  (u8 bus)   — narrow bus, 7-bit address field, wo cmd");
    axum::serve(listener, app).await.unwrap();
}
