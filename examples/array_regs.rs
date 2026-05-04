//! Example: register arrays — declare a contiguous run of identical
//! registers as `[T; N]` and access them with an index.
//!
//! Run with:
//!   cargo run --example array_regs

use std::sync::Arc;

use ddevmem::{register_map, DevMem};

register_map! {
    /// Block with control register, an array of 8 FIFO data slots,
    /// and an array of 4 channel-mask registers carrying typed bitfields.
    pub unsafe map DmaRegs (u32) {
        0x00 =>
            /// Global enable.
            rw ctrl: u32 {
                /// Master enable.
                enable: 0,
                /// Number of active channels (0–7).
                nch: 1..=3
            },

        // 8 word-wide FIFO slots at 0x10, 0x14, 0x18, ...
        0x10 =>
            /// 8-entry data FIFO. `fifo(i)` reads slot `i`,
            /// `set_fifo(i, v)` writes it.
            rw fifo: [u32; 8],

        // 4 channel-mask registers, each with `enable` + `prio` bitfields.
        0x40 =>
            /// Per-channel masks (4 channels). Bitfields are indexed too:
            /// `chan_enable(i)`, `set_chan_prio(i, v)`, …
            rw chan: [u32; 4] {
                /// Channel enable.
                enable: 0,
                /// Channel priority.
                prio: 1..=3
            }
    }
}

fn main() {
    let devmem = unsafe { DevMem::new(0x0, Some(256)).unwrap() };
    let mut dma = unsafe { DmaRegs::new(Arc::new(devmem)).unwrap() };

    // Scalar register still works the usual way.
    dma.set_ctrl_enable(1);
    dma.set_ctrl_nch(4);
    assert_eq!(dma.ctrl_nch(), 4);

    // Array register: indexed accessors.
    assert_eq!(dma.fifo_len(), 8);
    for i in 0..dma.fifo_len() {
        dma.set_fifo(i, 0xA000 + i as u32);
    }
    for i in 0..dma.fifo_len() {
        assert_eq!(dma.fifo(i), 0xA000 + i as u32);
        println!(
            "fifo[{i}] = 0x{:08X}  @ offset 0x{:02X}",
            dma.fifo(i),
            dma.fifo_offset(i)
        );
    }

    // Read-modify-write a single slot.
    dma.modify_fifo(3, |v| v ^ 0xFF);
    assert_eq!(dma.fifo(3), (0xA000 + 3) ^ 0xFF);

    // Array + bitfields: per-element bitfield setters/getters.
    for i in 0..4 {
        dma.set_chan_enable(i, 1);
        dma.set_chan_prio(i, (i as u32) + 1);
    }
    for i in 0..4 {
        assert_eq!(dma.chan_enable(i), 1);
        assert_eq!(dma.chan_prio(i), (i as u32) + 1);
        println!(
            "chan[{i}] raw = 0x{:08X}  enable={}  prio={}",
            dma.chan(i),
            dma.chan_enable(i),
            dma.chan_prio(i),
        );
    }

    println!("\nAll array assertions passed!");
}
