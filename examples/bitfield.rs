//! Example: register map with bitfields and doc comments (emulator backend).
//!
//! Run with:
//!   cargo run --example bitfield --no-default-features --features "emulator,register-map"

use std::sync::Arc;

use ddevmem::{register_map, DevMem};

register_map! {
    /// SPI controller registers.
    pub unsafe map SpiRegs (u32) {
        0x00 =>
            /// SPI control register.
            rw cr: u32 {
                /// Chip select — active-low output selector (0–7).
                cs:     0..=2,
                /// Clock polarity (CPOL).
                cpol:   3,
                /// Clock phase (CPHA).
                cpha:   4,
                /// Transfer enable.
                enable: 5
            },
        0x04 =>
            /// SPI status register.
            ro sr: u32 {
                /// Transmit FIFO empty.
                txe:  0,
                /// Receive FIFO not empty.
                rxne: 1,
                /// Busy flag — transfer in progress.
                busy: 7
            },
        0x08 =>
            /// SPI data register — write to TX, read from RX.
            rw dr: u32,
        0x0C =>
            /// Baud rate register.
            rw brr: u32 {
                /// Baud rate divisor (0–255).
                div: 0..=7
            }
    }
}

fn main() {
    // With the emulator feature, DevMem is backed by Vec<u8>.
    let devmem = unsafe { DevMem::new(0x0, Some(256)).unwrap() };
    let mut spi = unsafe { SpiRegs::new(Arc::new(devmem)).unwrap() };

    // Configure: chip-select 2, CPOL=1, CPHA=0, enable
    spi.set_cr_cs(2);
    spi.set_cr_cpol(1);
    spi.set_cr_cpha(0);
    spi.set_cr_enable(1);

    println!("CR = 0x{:08X}", spi.cr());
    println!("  cs     = {}", spi.cr_cs());
    println!("  cpol   = {}", spi.cr_cpol());
    println!("  cpha   = {}", spi.cr_cpha());
    println!("  enable = {}", spi.cr_enable());

    assert_eq!(spi.cr_cs(), 2);
    assert_eq!(spi.cr_cpol(), 1);
    assert_eq!(spi.cr_cpha(), 0);
    assert_eq!(spi.cr_enable(), 1);

    // Write data register
    spi.set_dr(0xAB);
    println!("\nDR = 0x{:08X}", spi.dr());
    assert_eq!(spi.dr(), 0xAB);

    // Set baud rate divisor
    spi.set_brr_div(7);
    println!("BRR div = {}", spi.brr_div());
    assert_eq!(spi.brr_div(), 7);

    // Read-modify-write: flip enable bit
    spi.modify_cr(|v| v ^ (1 << 5));
    println!("\nAfter toggle enable: CR = 0x{:08X}", spi.cr());
    assert_eq!(spi.cr_enable(), 0);

    println!("\nAll assertions passed!");
}
