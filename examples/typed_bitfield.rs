//! Example: typed bitfields — `as bool`, `as u8`, `as enum`, an enum shared
//! between fields, and a type of your own through `FieldValue`.
//!
//! Run with:
//!   cargo run --example typed_bitfield

use std::sync::Arc;

use ddevmem::{register_map, DevMem, FieldValue};

/// Input filter length in samples, stored as its base-2 logarithm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FilterLen(pub u32);

impl FieldValue for FilterLen {
    fn from_bits(bits: u64) -> Self {
        FilterLen(1 << bits)
    }

    fn to_bits(self) -> u64 {
        u64::from(self.0.trailing_zeros())
    }
}

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
            rw cnt: u32,

        /// Which edge a capture channel triggers on.
        enum Edge {
            Rising  = 0,
            Falling = 1,
            Both    = 2,
        }

        0x0C =>
            /// Capture configuration.
            rw ccr: u32 {
                /// Channel 1 trigger edge.
                ch1_edge: 0..=1 as Edge,
                /// Channel 2 trigger edge.
                ch2_edge: 2..=3 as Edge,
                /// Input filter length.
                filter: 4..=6 as FilterLen,
            }
    }
}

fn main() {
    let devmem = unsafe { DevMem::new(0x0, Some(256)).unwrap() };
    let mut timer = unsafe { TimerRegs::new(Arc::new(devmem)).unwrap() };

    // Bool bitfields
    timer.set_cr_enable(true);
    assert!(timer.cr_enable());
    println!("enable = {}", timer.cr_enable());

    timer.set_cr_one_pulse(false);
    assert!(!timer.cr_one_pulse());
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

    // One enum shared by two fields
    timer.set_ccr_ch1_edge(Edge::Falling);
    timer.set_ccr_ch2_edge(Edge::Both);
    assert_eq!(timer.ccr_ch1_edge(), Edge::Falling);
    assert_eq!(timer.ccr_ch2_edge(), Edge::Both);
    println!(
        "\nch1 = {}, ch2 = {}",
        timer.ccr_ch1_edge(),
        timer.ccr_ch2_edge()
    );

    // A type of your own, converted through `FieldValue`
    timer.set_ccr_filter(FilterLen(16));
    assert_eq!(timer.ccr_filter(), FilterLen(16));
    println!("filter = {:?}", timer.ccr_filter());

    // ch1(1) | ch2(2)<<2 | filter(log2 16 = 4)<<4 = 1 + 8 + 64 = 73
    println!("CCR = 0x{:08X}", timer.ccr());
    assert_eq!(timer.ccr(), 0x49);

    println!("\nAll assertions passed!");
}
