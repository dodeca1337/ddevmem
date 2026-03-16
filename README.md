# ddevmem

[![Latest Version]][crates.io] [![Documentation]][docs.rs] ![Downloads] ![License]

Safe and ergonomic Rust library for accessing physical memory via `/dev/mem`,
with volatile read/write semantics suitable for memory-mapped I/O (MMIO).

## Features

| Feature        | Default | Description                                                |
| -------------- | ------- | ---------------------------------------------------------- |
| `device`       | ✓       | Real `/dev/mem` backend via `memmap2`.                     |
| `emulator`     |         | Heap-backed `Vec<u8>` for testing without hardware.        |
| `reg`          |         | Typed `Reg<T>` / `SliceReg<T>` register handles.           |
| `register-map` | ✓       | Declarative `register_map!` macro with optional bitfields. |

> **Note:** enable exactly one of `device` or `emulator`. Enabling both is a compile error.

## Installation

```toml
[dependencies]
ddevmem = "0.4.0"
```

Or with specific features:

```toml
[dependencies]
ddevmem = { version = "0.4.0", default-features = false, features = ["emulator", "register-map"] }
```

## Quick start

### Raw `DevMem` access

```rust,no_run
use ddevmem::DevMem;

let devmem = unsafe { DevMem::new(0x4000_0000, Some(0x1000)).unwrap() };

// Volatile read
let value: u32 = devmem.read(0x00).unwrap();

// Volatile write
devmem.write(0x04, 0xDEAD_BEEFu32).unwrap();

// Read-modify-write
devmem.modify::<u32>(0x00, |v| v | (1 << 8)).unwrap();
```

### Register map with bitfields

```rust,no_run
use std::sync::Arc;
use ddevmem::{register_map, DevMem};

register_map! {
    pub unsafe map Regs (u32) {
        0x00 => rw control: u32 {
            enable:    0,
            mode:      1..=3,
            threshold: 4..=7
        },
        0x04 => ro status: u32 {
            ready: 0,
            error: 1
        },
        0x08 => wo command: u32
    }
}

let devmem = unsafe { DevMem::new(0x4000_0000, None).unwrap() };
let mut regs = unsafe { Regs::new(Arc::new(devmem)).unwrap() };

// Full-register access
let status = regs.status();
regs.set_command(0xFF);
regs.modify_control(|v| v | 1);

// Bitfield access
let enabled: u32 = regs.control_enable();  // single-bit → value from bit 0
let mode: u32    = regs.control_mode();     // bits 1..=3

regs.set_control_mode(0b101);              // read-modify-write only the mode bits
```

### `register_map!` syntax reference

```text
register_map! {
    $vis unsafe map $Name ($bus_width) {
        $offset => $kind $name: $type { …bitfields… },
        ...
    }
}
```

| Element        | Description                                                  |
| -------------- | ------------------------------------------------------------ |
| `$vis`         | Visibility (`pub`, `pub(crate)`, etc.).                      |
| `$Name`        | Name of the generated struct.                                |
| `($bus_width)` | Optional bus type (e.g. `u32`). All accesses use this width. |
| `$offset`      | Byte offset of the register (`0x00`, `0x04`, …).             |
| `$kind`        | `rw` (read-write), `ro` (read-only), or `wo` (write-only).   |
| `$name`        | Register name — drives the generated method names.           |
| `$type`        | Register type (`u8`, `u16`, `u32`, `u64`).                   |

**Bitfield syntax:**

```text
field_name: bit             // single bit
field_name: lo..=hi         // inclusive range (recommended)
field_name: lo..hi          // also inclusive (same as ..=)
```

Bits not covered by any field declaration are left untouched during
read-modify-write — there is no need to declare reserved gaps.

**Generated methods per register:**

| Kind        | Method            | Description                         |
| ----------- | ----------------- | ----------------------------------- |
| all         | `name_offset()`   | Byte offset within DevMem.          |
| all         | `name_address()`  | Physical address (`base + offset`). |
| `rw` / `ro` | `name()`          | Volatile read.                      |
| `rw` / `wo` | `set_name(value)` | Volatile write.                     |
| `rw`        | `modify_name(f)`  | Volatile read-modify-write.         |

**Generated methods per bitfield:**

| Kind        | Method                 | Description                            |
| ----------- | ---------------------- | -------------------------------------- |
| `rw` / `ro` | `reg_field()`          | Extract field bits.                    |
| `rw` / `wo` | `set_reg_field(value)` | Read-modify-write only the field bits. |

### Typed registers (`reg` feature)

```rust,no_run
use std::sync::Arc;
use ddevmem::DevMem;
use ddevmem::reg::{Reg, ReadOnlyReg, SliceReg};

let devmem = Arc::new(unsafe { DevMem::new(0x4000_0000, Some(0x100)).unwrap() });

// Read-write register at offset 0x00
let mut ctrl = unsafe { Reg::<u32>::new(devmem.clone(), 0x00).unwrap() };
ctrl.write(0x01);
let val = ctrl.read();
ctrl.modify(|v| v | (1 << 4));

// Read-only register
let status = unsafe { ReadOnlyReg::<u32>::new(devmem.clone(), 0x04).unwrap() };
let s = status.read();
// status.write(0); // compile error — WRITE = false

// Array of 8 registers starting at offset 0x10
let mut buf = unsafe { SliceReg::<u32>::new(devmem, 0x10, 8).unwrap() };
buf.write_at(0, 0xAA);
let first = buf.read_at(0);
```

## Migration from 0.3

`ddevmem` 0.4 is a **breaking** release. Key changes:

| 0.3                                 | 0.4                                |
| ----------------------------------- | ---------------------------------- |
| `*reg.get()` / `*reg.get_mut() = v` | `reg.read()` / `reg.write(v)`      |
| `reg.get_mut()` dereference         | `reg.modify(\|v\| …)`              |
| `black_box`-based access            | `read_volatile` / `write_volatile` |
| No bitfield support                 | `register_map!` with bitfields     |
| No bus-width control                | `register_map!(… (u32) { … })`     |

## License

ddevmem is distributed under the terms of the [MIT license](https://opensource.org/licenses/MIT).
See [LICENSE-MIT](./LICENSE-MIT) for details.

[crates.io]: https://crates.io/crates/ddevmem
[latest version]: https://img.shields.io/crates/v/ddevmem.svg
[docs.rs]: https://docs.rs/ddevmem
[documentation]: https://docs.rs/ddevmem/badge.svg
[downloads]: https://img.shields.io/crates/d/ddevmem
[license]: https://img.shields.io/crates/l/ddevmem.svg