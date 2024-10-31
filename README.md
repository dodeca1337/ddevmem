# ddevmem

[![Latest Version]][crates.io] [![Documentation]][docs.rs] ![Downloads] ![License]

Rust library for accessing the physical address space using /dev/mem similar to [busybox devmem](https://www.busybox.net/downloads/BusyBox.html#devmem)

## Installation

Add `ddevmem` to your `Cargo.toml`:

```toml
[dependencies]
ddevmem = "0.2.4"
```

## Example

```rust
use ddevmem::{register_map, DevMem};

register_map! {
    pub unsafe map MyRegisterMap {
        0x00 => rw reg0: u32,
        0x04 => ro reg1: u32,
        0x08 => wo reg2: u32
    }
}

let devmem = unsafe { DevMem::new(0xD0DE_0000, None).unwrap() };
let mut reg_map = unsafe { MyRegisterMap::new(std::sync::Arc::new(devmem)).unwrap() };
let (reg0_address, reg0_offset) = (reg_map.reg0_address(), reg_map.reg0_offset());
let reg1_value = *reg_map.reg1();
*reg_map.mut_reg2() = reg1_value;
```

## License

Ddevmem is distributed under the terms of the [MIT license](https://opensource.org/licenses/MIT). See terms and conditions [here](./LICENSE-MIT).


[crates.io]: https://crates.io/crates/ddevmem
[latest version]: https://img.shields.io/crates/v/ddevmem.svg
[docs.rs]: https://docs.rs/ddevmem
[documentation]: https://docs.rs/libc/badge.svg
[downloads]: https://img.shields.io/crates/d/ddevmem
[license]: https://img.shields.io/crates/l/ddevmem.svg