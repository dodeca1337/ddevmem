//! Example: typed bitfields — `as bool`, `as u8`, and `as enum`.
//!
//! Run with:
//!   cargo run --example typed_bitfield --no-default-features --features "emulator,register-map"

use std::sync::Arc;

use ddevmem::{register_map, DevMem};

register_map! {
    /// Timer controller with typed bitfields.
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
                },
            },
        0x04 =>
            /// Timer status register.
            ro sr: u32 {
                /// Counter active flag.
                active: 0 as bool,
                /// Overflow flag.
                overflow: 1 as bool,
            },
        0x08 =>
            /// Counter value.
            rw cnt: u32
    }
}

fn main() {
    let devmem = unsafe { DevMem::new(0x0, Some(256)).unwrap() };
    let mut timer = unsafe { TimerRegs::new(Arc::new(devmem)).unwrap() };

    // Bool bitfields
    timer.set_cr_enable(true);
    assert_eq!(timer.cr_enable(), true);
    println!("enable = {}", timer.cr_enable());

    timer.set_cr_one_pulse(false);
    assert_eq!(timer.cr_one_pulse(), false);
    println!("one_pulse = {}", timer.cr_one_pulse());

    // Cast bitfield (u8)
    timer.set_cr_psc(7);
    assert_eq!(timer.cr_psc(), 7u8);
    println!("psc = {}", timer.cr_psc());

    // Enum bitfield
    timer.set_cr_mode(TimerMode::FreeRun);
    assert_eq!(timer.cr_mode(), TimerMode::FreeRun);
    println!("mode = {:?}", timer.cr_mode());

    timer.set_cr_mode(TimerMode::External);
    assert_eq!(timer.cr_mode(), TimerMode::External);
    println!("mode = {:?}", timer.cr_mode());

    // Verify raw register value
    // enable(1) | one_pulse(0) | psc(7)<<2 | mode(3)<<6 = 1 + 0 + 28 + 192 = 221
    println!("\nCR = 0x{:08X}", timer.cr());
    assert_eq!(timer.cr(), 0xDD);

    // enum from_raw with unknown value defaults to first variant
    timer.set_cr(0);
    assert_eq!(timer.cr_mode(), TimerMode::Stopped);
    println!("mode after clear = {:?}", timer.cr_mode());

    println!("\nAll assertions passed!");
}
