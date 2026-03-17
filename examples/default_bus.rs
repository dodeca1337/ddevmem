//! Example: basic register map with explicit 32-bit bus width.
//!
//! Run with:
//!   cargo run --example default_bus --no-default-features --features "emulator,register-map"

use std::sync::Arc;

use ddevmem::{register_map, DevMem};

register_map! {
    pub unsafe map Regs (u32) {
        0x00 => rw data:   u32,
        0x04 => ro status: u32,
        0x08 => wo command: u32
    }
}

fn main() {
    let devmem = unsafe { DevMem::new(0x0, Some(256)).unwrap() };
    let mut regs = unsafe { Regs::new(Arc::new(devmem)).unwrap() };

    // Write and read back
    regs.set_data(0xDEAD_BEEF);
    println!("data    = 0x{:08X}", regs.data());
    assert_eq!(regs.data(), 0xDEAD_BEEF);

    // Status is read-only — the emulator starts at zero
    println!("status  = 0x{:08X}", regs.status());
    assert_eq!(regs.status(), 0);

    // Command is write-only (no getter)
    regs.set_command(0xFF);

    // Offsets and addresses
    println!("data    offset  = 0x{:X}", regs.data_offset());
    println!("status  offset  = 0x{:X}", regs.status_offset());
    println!("command offset  = 0x{:X}", regs.command_offset());

    println!("\nAll assertions passed!");
}
