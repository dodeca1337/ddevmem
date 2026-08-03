//! Behavioral tests for the `register_map!` macro (emulator backend).

use std::sync::Arc;

use ddevmem::{register_map, DevMem};

fn devmem(size: usize) -> Arc<DevMem> {
    Arc::new(unsafe { DevMem::new(0x4000_0000, Some(size)).unwrap() })
}

// ─── Full-register access ────────────────────────────────────────────────────

register_map! {
    /// Basic block.
    pub unsafe map Basic (u32) {
        0x00 => rw data: u32,
        0x04 => ro status: u32,
        0x08 => wo command: u32
    }
}

#[test]
fn read_write_roundtrip() {
    let mem = devmem(256);
    let mut regs = unsafe { Basic::new(mem.clone()).unwrap() };

    regs.set_data(0xDEAD_BEEF);
    assert_eq!(regs.data(), 0xDEAD_BEEF);
    assert_eq!(mem.read::<u32>(0x00), Some(0xDEAD_BEEF));

    regs.modify_data(|v| v ^ 0xFFFF_0000);
    assert_eq!(regs.data(), 0x2152_BEEF);
}

#[test]
fn write_only_register_writes_through() {
    let mem = devmem(256);
    let mut regs = unsafe { Basic::new(mem.clone()).unwrap() };

    regs.set_command(0x1234_5678);
    assert_eq!(mem.read::<u32>(0x08), Some(0x1234_5678));
}

#[test]
fn read_only_register_reads_underlying_memory() {
    let mem = devmem(256);
    let regs = unsafe { Basic::new(mem.clone()).unwrap() };

    mem.write::<u32>(0x04, 0xC0FF_EE00).unwrap();
    assert_eq!(regs.status(), 0xC0FF_EE00);
}

#[test]
fn offsets_and_addresses() {
    let mem = devmem(256);
    let regs = unsafe { Basic::new(mem).unwrap() };

    assert_eq!(regs.data_offset(), 0x00);
    assert_eq!(regs.status_offset(), 0x04);
    assert_eq!(regs.command_offset(), 0x08);
    assert_eq!(regs.status_address(), 0x4000_0004);
}

#[test]
fn new_checks_region_length() {
    // The map spans 0x0C bytes.
    assert!(unsafe { Basic::new(devmem(0x0B)) }.is_none());
    assert!(unsafe { Basic::new(devmem(0x0C)) }.is_some());
}

// ─── Registers narrower than the bus ─────────────────────────────────────────

register_map! {
    pub unsafe map Narrow (u32) {
        0x00 => rw byte: u8
    }
}

#[test]
fn narrow_register_uses_full_bus_access() {
    let mem = devmem(256);
    let mut regs = unsafe { Narrow::new(mem.clone()).unwrap() };

    // Pre-set high bits; a bus-wide write of the zero-extended value must
    // clear them (the register is accessed as one u32 word).
    mem.write::<u32>(0x00, 0xFFFF_FF00).unwrap();
    regs.set_byte(0xAB);
    assert_eq!(mem.read::<u32>(0x00), Some(0xAB));

    // Reads truncate the bus word to the register type.
    mem.write::<u32>(0x00, 0x1234_56CD).unwrap();
    assert_eq!(regs.byte(), 0xCD);
}

// ─── Bitfields ───────────────────────────────────────────────────────────────

register_map! {
    pub unsafe map Fields (u32) {
        0x00 => rw cr: u32 {
            enable: 0,
            mode: 1..=3,
            // Exclusive upper bound: bits 4..=6.
            level: 4..7,
            flag: 8 as bool,
            count: 12..=15 as u8,
            speed: 16..=17 as enum Speed {
                Slow = 0,
                Normal = 1,
                Fast = 2,
                Turbo = 3,
            }
        },
        0x04 => ro sr: u32 {
            ready: 0 as bool,
            error: 1 as bool
        },
        0x08 => wo cmd: u32 {
            start: 0 as bool,
            channel: 4..=6 as u8
        }
    }
}

#[test]
fn bitfield_set_preserves_neighbors() {
    let mem = devmem(256);
    let mut regs = unsafe { Fields::new(mem).unwrap() };

    regs.set_cr(0xFFFF_FFFF);
    regs.set_cr_mode(0b010);
    assert_eq!(regs.cr_mode(), 0b010);
    // Only bits 3:1 changed.
    assert_eq!(regs.cr(), 0xFFFF_FFF5);
}

#[test]
fn bitfield_setter_masks_oversized_values() {
    let mem = devmem(256);
    let mut regs = unsafe { Fields::new(mem).unwrap() };

    // 0xFF does not fit into 3 bits; only the low bits may land.
    regs.set_cr_mode(0xFF);
    assert_eq!(regs.cr_mode(), 0b111);
    assert_eq!(regs.cr(), 0b1110);
}

#[test]
fn exclusive_range_covers_expected_bits() {
    let mem = devmem(256);
    let mut regs = unsafe { Fields::new(mem).unwrap() };

    regs.set_cr_level(0b111);
    assert_eq!(regs.cr(), 0b111 << 4);
    assert_eq!(regs.cr_level(), 0b111);
}

#[test]
fn typed_bitfields_roundtrip() {
    let mem = devmem(256);
    let mut regs = unsafe { Fields::new(mem).unwrap() };

    regs.set_cr_flag(true);
    assert!(regs.cr_flag());
    regs.set_cr_flag(false);
    assert!(!regs.cr_flag());

    regs.set_cr_count(0xA);
    assert_eq!(regs.cr_count(), 0xAu8);

    regs.set_cr_speed(Speed::Fast);
    assert_eq!(regs.cr_speed(), Speed::Fast);
    assert_eq!(regs.cr() >> 16 & 0b11, 2);
}

#[test]
fn enum_from_raw_falls_back_to_first_variant() {
    assert_eq!(Speed::from_raw(2), Speed::Fast);
    // No variant with value 0xFF exists.
    assert_eq!(Speed::from_raw(0xFF), Speed::Slow);
    assert_eq!(Speed::Turbo.to_raw(), 3);
    assert_eq!(Speed::Normal.to_string(), "Normal");
}

#[test]
fn read_only_bitfields() {
    let mem = devmem(256);
    let regs = unsafe { Fields::new(mem.clone()).unwrap() };

    mem.write::<u32>(0x04, 0b10).unwrap();
    assert!(!regs.sr_ready());
    assert!(regs.sr_error());
}

#[test]
fn write_only_bitfield_writes_field_and_zeroes_rest() {
    let mem = devmem(256);
    let mut regs = unsafe { Fields::new(mem.clone()).unwrap() };

    // Pre-fill the register; a wo bitfield setter cannot read-modify-write,
    // so it must overwrite the whole word with only the field set.
    mem.write::<u32>(0x08, 0xFFFF_FFFF).unwrap();
    regs.set_cmd_channel(0b101);
    assert_eq!(mem.read::<u32>(0x08), Some(0b101 << 4));

    regs.set_cmd_start(true);
    assert_eq!(mem.read::<u32>(0x08), Some(1));
}

// ─── Per-field access ────────────────────────────────────────────────────────

register_map! {
    /// The pattern the field-level access kinds exist for: RW configuration
    /// sharing a register with write-1-to-clear interrupt flags.
    pub unsafe map Isr (u32) {
        0x00 => rw cr: u32 {
            enable: 0 as bool,
            mode: 1..=2 as u8,
            ro  locked: 3 as bool,
            w1c overrun: 8 as bool,
            w1c error: 9 as bool,
            wo  trigger: 16 as bool
        },
        0x04 => wo ack: u32 {
            w1c done: 0 as bool,
            channel: 4..=6 as u8
        }
    }
}

#[test]
fn writing_a_field_does_not_acknowledge_pending_w1c_flags() {
    let mem = devmem(256);
    let mut regs = unsafe { Isr::new(mem.clone()).unwrap() };

    // Hardware raised both flags while we were configuring.
    mem.write::<u32>(0x00, 0b11 << 8 | 0b101).unwrap();

    regs.set_cr_mode(3);

    // The flags must go back as zero — writing them back as read would
    // acknowledge them — while ordinary bits are preserved.
    let written = mem.read::<u32>(0x00).unwrap();
    assert_eq!(written & (0b11 << 8), 0, "w1c bits must be written as zero");
    assert_eq!(written & 1, 1, "enable preserved");
    assert_eq!(regs.cr_mode(), 3);
}

#[test]
fn wo_field_is_not_re_triggered_by_a_neighbouring_write() {
    let mem = devmem(256);
    let mut regs = unsafe { Isr::new(mem.clone()).unwrap() };

    // The trigger bit still reads as 1 (command in flight).
    mem.write::<u32>(0x00, 1 << 16).unwrap();
    regs.set_cr_enable(true);

    assert_eq!(mem.read::<u32>(0x00).unwrap() & (1 << 16), 0);
}

#[test]
fn clear_acknowledges_only_its_own_flag() {
    let mem = devmem(256);
    let mut regs = unsafe { Isr::new(mem.clone()).unwrap() };

    mem.write::<u32>(0x00, 0b11 << 8 | 0b101).unwrap();
    assert!(regs.cr_overrun());
    assert!(regs.cr_error());

    regs.clear_cr_overrun();

    let written = mem.read::<u32>(0x00).unwrap();
    assert_eq!(written & (1 << 8), 1 << 8, "overrun acknowledged");
    assert_eq!(written & (1 << 9), 0, "error left pending");
    assert_eq!(written & 0b101, 0b101, "configuration preserved");
}

#[test]
fn clear_on_a_write_only_register_zeroes_everything_else() {
    let mem = devmem(256);
    let mut regs = unsafe { Isr::new(mem.clone()).unwrap() };

    mem.write::<u32>(0x04, 0xFFFF_FFFF).unwrap();
    regs.clear_ack_done();
    assert_eq!(mem.read::<u32>(0x04), Some(1));
}

#[test]
fn ro_field_is_readable_and_reflects_memory() {
    let mem = devmem(256);
    let regs = unsafe { Isr::new(mem.clone()).unwrap() };

    mem.write::<u32>(0x00, 1 << 3).unwrap();
    assert!(regs.cr_locked());
}

// ─── Whole-register writer for `wo` registers ────────────────────────────────

register_map! {
    pub unsafe map Cmd (u32) {
        0x00 => wo cmd: u32 {
            tx_reset: 0 as bool,
            rx_reset: 1 as bool,
            channel: 4..=6 as u8,
            mode: 8..=9 as enum CmdMode {
                Idle = 0,
                Run = 1,
                Halt = 2,
            }
        },
        0x04 => wo trig: [u32; 2] {
            fire: 0 as bool
        }
    }
}

#[test]
fn writer_sets_several_fields_in_one_transaction() {
    let mem = devmem(256);
    let mut regs = unsafe { Cmd::new(mem.clone()).unwrap() };

    regs.write_cmd(|w| w.tx_reset(true).channel(5).mode(CmdMode::Run));

    assert_eq!(mem.read::<u32>(0x00), Some(1 | 5 << 4 | 1 << 8));
}

#[test]
fn writer_starts_from_zero_every_time() {
    let mem = devmem(256);
    let mut regs = unsafe { Cmd::new(mem.clone()).unwrap() };

    mem.write::<u32>(0x00, 0xFFFF_FFFF).unwrap();
    regs.write_cmd(|w| w.rx_reset(true));

    assert_eq!(mem.read::<u32>(0x00), Some(0b10));
}

#[test]
fn writer_works_on_array_registers() {
    let mem = devmem(256);
    let mut regs = unsafe { Cmd::new(mem.clone()).unwrap() };

    regs.write_trig(1, |w| w.fire(true));
    assert_eq!(mem.read::<u32>(0x04), Some(0));
    assert_eq!(mem.read::<u32>(0x08), Some(1));
}

// ─── Register arrays ─────────────────────────────────────────────────────────

register_map! {
    pub unsafe map Arrays (u32) {
        0x00 => rw ctrl: u32,
        0x10 => rw fifo: [u32; 8],
        0x40 => rw chan: [u32; 4] {
            enable: 0 as bool,
            prio: 1..=3 as u8
        },
        0x50 => ro samples: [u32; 2]
    }
}

#[test]
fn array_elements_are_one_bus_word_apart() {
    let mem = devmem(256);
    let mut regs = unsafe { Arrays::new(mem.clone()).unwrap() };

    assert_eq!(regs.fifo_len(), 8);
    for i in 0..regs.fifo_len() {
        assert_eq!(regs.fifo_offset(i), 0x10 + i * 4);
        regs.set_fifo(i, 0xA000 + i as u32);
    }
    for i in 0..regs.fifo_len() {
        assert_eq!(mem.read::<u32>(0x10 + i * 4), Some(0xA000 + i as u32));
        assert_eq!(regs.fifo(i), 0xA000 + i as u32);
    }

    regs.modify_fifo(3, |v| v | 0xF);
    assert_eq!(regs.fifo(3), 0xA003 | 0xF);
}

#[test]
fn array_bitfields_are_indexed() {
    let mem = devmem(256);
    let mut regs = unsafe { Arrays::new(mem).unwrap() };

    for i in 0..4 {
        regs.set_chan_enable(i, true);
        regs.set_chan_prio(i, i as u8 + 1);
    }
    for i in 0..4 {
        assert!(regs.chan_enable(i));
        assert_eq!(regs.chan_prio(i), i as u8 + 1);
        assert_eq!(regs.chan(i), 1 | ((i as u32 + 1) << 1));
    }
}

#[test]
#[should_panic(expected = "out of bounds for register array `fifo`")]
fn array_read_out_of_bounds_panics() {
    let regs = unsafe { Arrays::new(devmem(256)).unwrap() };
    let _ = regs.fifo(8);
}

#[test]
#[should_panic(expected = "out of bounds for register array `chan`")]
fn array_bitfield_out_of_bounds_panics() {
    let mut regs = unsafe { Arrays::new(devmem(256)).unwrap() };
    regs.set_chan_enable(4, true);
}

#[test]
fn array_length_counts_toward_required_region() {
    // `samples` ends at 0x50 + 2 * 4 = 0x58.
    assert!(unsafe { Arrays::new(devmem(0x57)) }.is_none());
    assert!(unsafe { Arrays::new(devmem(0x58)) }.is_some());
}

// ─── Const-expression positions and offsets ──────────────────────────────────

const FIELD_LO: u32 = 4;
const FIELD_HI: u32 = 7;
const REG_OFFSET: usize = 0x08;

register_map! {
    pub unsafe map ConstPos (u32) {
        0x00 => rw cr: u32 {
            f: (FIELD_LO)..=(FIELD_HI) as u8,
            g: (30)
        },
        REG_OFFSET => rw extra: u32
    }
}

#[test]
fn const_expression_positions_work() {
    let mem = devmem(256);
    let mut regs = unsafe { ConstPos::new(mem).unwrap() };

    regs.set_cr_f(0xFF); // masked to 4 bits
    assert_eq!(regs.cr_f(), 0xF);
    assert_eq!(regs.cr(), 0xF0);

    regs.set_cr_g(1);
    assert_eq!(regs.cr_g(), 1);
    assert_eq!(regs.cr(), 0xF0 | 1 << 30);

    assert_eq!(regs.extra_offset(), 0x08);
    regs.set_extra(7);
    assert_eq!(regs.extra(), 7);
}

// ─── Default (usize) bus ─────────────────────────────────────────────────────

register_map! {
    pub unsafe map NativeBus {
        0x00 => rw word: usize {
            low: 0..=3
        }
    }
}

#[test]
fn default_bus_is_pointer_wide() {
    let mem = devmem(256);
    let mut regs = unsafe { NativeBus::new(mem.clone()).unwrap() };

    regs.set_word(usize::MAX);
    assert_eq!(regs.word(), usize::MAX);
    assert_eq!(mem.read::<usize>(0x00), Some(usize::MAX));

    regs.set_word_low(0);
    assert_eq!(regs.word(), usize::MAX << 4);
    assert_eq!(regs.word_low(), 0);
}

// ─── Aliased read-only / write-only registers at one offset ──────────────────

register_map! {
    /// Hardware-style aliasing: reading 0x00 pops RX, writing pushes TX.
    pub unsafe map Aliased (u32) {
        0x00 => ro rx: u32,
        0x00 => wo tx: u32
    }
}

#[test]
fn aliased_ro_wo_registers_share_an_offset() {
    let mem = devmem(256);
    let mut regs = unsafe { Aliased::new(mem).unwrap() };

    regs.set_tx(0x55);
    assert_eq!(regs.rx(), 0x55);
}

// ─── Send/Sync of the generated struct ───────────────────────────────────────

#[test]
fn generated_map_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Basic>();
}
