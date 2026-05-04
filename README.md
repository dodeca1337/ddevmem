# ddevmem

[![Latest Version]][crates.io] [![Documentation]][docs.rs] ![Downloads] ![License]

Safe and ergonomic Rust library for accessing physical memory via `/dev/mem`,
with volatile read/write semantics suitable for memory-mapped I/O (MMIO).

## Features

| Feature        | Default | Description                                                                    |
| -------------- | ------- | ------------------------------------------------------------------------------ |
| `device`       | ✓       | Real `/dev/mem` backend via `memmap2`.                                         |
| `emulator`     |         | Heap-backed `Vec<u8>` for testing without hardware.                            |
| `register-map` | ✓       | Declarative `register_map!` macro with optional bitfields and typed accessors. |
| `web`          |         | Web UI for viewing/editing registers via `axum` (optional auth).               |

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

With the web UI:

```toml
[dependencies]
ddevmem = { version = "0.4.0", features = ["web"] }
tokio = { version = "1", features = ["full"] }
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

// Bulk operations
let mut buf = [0u32; 4];
devmem.read_slice(0x10, &mut buf);
devmem.write_slice(0x10, &[1, 2, 3, 4]);
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

### Typed bitfields

Bitfields can carry an `as <type>` suffix to change the getter/setter types.
Three forms are supported: `as bool`, `as <integer>`, and `as enum`.

```rust,no_run
use std::sync::Arc;
use ddevmem::{register_map, DevMem};

register_map! {
    /// Timer controller with typed bitfields.
    pub unsafe map TimerRegs (u32) {
        0x00 =>
            /// Control register.
            rw cr: u32 {
                /// Enable flag.
                enable: 0 as bool,
                /// Prescaler (0–15).
                psc: 2..=5 as u8,
                /// Operating mode.
                mode: 6..=7 as enum TimerMode {
                    Stopped  = 0,
                    OneShot  = 1,
                    FreeRun  = 2,
                    External = 3,
                },
            }
    }
}

let devmem = unsafe { DevMem::new(0x4000_0000, None).unwrap() };
let mut timer = unsafe { TimerRegs::new(Arc::new(devmem)).unwrap() };

timer.set_cr_enable(true);            // bool
timer.set_cr_psc(7);                  // u8
timer.set_cr_mode(TimerMode::FreeRun); // enum

assert_eq!(timer.cr_enable(), true);
assert_eq!(timer.cr_psc(), 7u8);
assert_eq!(timer.cr_mode(), TimerMode::FreeRun);
```

### Documented register map

Doc comments (`/// ...`) can be placed on the struct, on individual registers
(after `=>`), and on individual bitfields. Comments are forwarded to generated
Rust doc and displayed in the web UI when the `web` feature is enabled.

```rust,no_run
use std::sync::Arc;
use ddevmem::{register_map, DevMem};

register_map! {
    /// SPI controller registers.
    pub unsafe map SpiRegs (u32) {
        0x00 =>
            /// SPI control register.
            rw cr: u32 {
                /// Chip select — active-low output selector.
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
            /// SPI data register — write to transmit, read to receive.
            rw dr: u32,
        0x0C =>
            /// Baud rate divisor (actual rate = PCLK / (2 * (div + 1))).
            rw brr: u32 {
                /// Divisor value (0..=255).
                div: 0..=7
            }
    }
}

let devmem = unsafe { DevMem::new(0x4002_0000, None).unwrap() };
let mut spi = unsafe { SpiRegs::new(Arc::new(devmem)).unwrap() };

// Wait until TX FIFO is empty, then send a byte
while spi.sr_txe() == 0 {}
spi.set_dr(0x42);

// Configure: CPOL=1, CPHA=0, chip-select 2, enable
spi.set_cr_cpol(1);
spi.set_cr_cpha(0);
spi.set_cr_cs(2);
spi.set_cr_enable(1);
```

### `register_map!` syntax reference

```text
register_map! {
    /// Optional struct-level doc comment.
    $vis unsafe map $Name ($bus_width) {
        $offset =>
            /// Optional register doc comment.
            $kind $name: $type {
                /// Optional bitfield doc comment.
                field: bits,
                ...
            },
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
field_name: lo..hi          // exclusive upper bound (Rust convention)
```

A bitfield can carry an `as <type>` suffix to produce typed getters/setters:

```text
field: bit        as bool              // getter → bool, setter accepts bool
field: lo..=hi    as u8                // getter → u8,   setter accepts u8 (any int type)
field: lo..=hi    as enum Name {       // getter → Name, setter accepts Name
    Variant = value,                   //   #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    ...,                               //   with from_raw() / to_raw() methods
}
```

Bits not covered by any field declaration are left untouched during
read-modify-write — there is no need to declare reserved gaps.

**Register arrays.** A register declared as `[T; N]` becomes a contiguous
run of `N` identical registers at `offset, offset + size_of::<bus>(), …`.
Accessors take an extra `idx: usize` parameter, and any bitfields on the
array entry get the same treatment:

```text
0x10 =>
    rw fifo: [u32; 8],          // -> fifo(i), set_fifo(i, v), modify_fifo(i, f), fifo_len()
0x40 =>
    rw chan: [u32; 4] {         // -> chan(i), set_chan(i, v), chan_len()
        enable: 0    as bool,   // -> chan_enable(i), set_chan_enable(i, b)
        prio:   1..=3 as u8     // -> chan_prio(i),   set_chan_prio(i, n)
    }
```

A complete example using the array API:

```rust,no_run
use std::sync::Arc;
use ddevmem::{register_map, DevMem};

register_map! {
    pub unsafe map Dma (u32) {
        0x10 => rw fifo: [u32; 8],
        0x40 => rw chan: [u32; 4] {
            enable: 0    as bool,
            prio:   1..=3 as u8
        }
    }
}

let devmem = unsafe { DevMem::new(0x4002_0000, None).unwrap() };
let mut dma = unsafe { Dma::new(Arc::new(devmem)).unwrap() };

// Whole-register access by index.
for i in 0..dma.fifo_len() {
    dma.set_fifo(i, (i as u32) * 0x1111_1111);
}
let head: u32 = dma.fifo(0);

// Bitfield access on each array element.
for i in 0..dma.chan_len() {
    dma.set_chan_enable(i, true);
    dma.set_chan_prio(i, i as u8);
}
assert!(dma.chan_enable(0));
assert_eq!(dma.chan_prio(2), 2u8);
```

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

When a type suffix is present the return / argument type changes accordingly:

| Suffix         | Getter returns | Setter accepts |
| -------------- | -------------- | -------------- |
| *(none)*       | register type  | register type  |
| `as bool`      | `bool`         | `bool`         |
| `as u8` (etc.) | `u8`           | `u8`           |
| `as enum Name` | `Name`         | `Name`         |

### Web UI (`web` feature)

The `web` feature adds a browser-based interface for viewing and editing
registers at runtime. It is powered by `axum` and requires `tokio`.

When `web` is enabled, `register_map!` auto-implements the
`RegisterMapInfo` trait, which exposes register metadata (names, offsets,
access types, bitfield descriptions, doc strings) and raw read/write access.

```rust,no_run
use std::sync::Arc;
use tokio::sync::Mutex;
use ddevmem::{register_map, DevMem};

register_map! {
    /// PWM controller.
    pub unsafe map PwmRegs (u32) {
        0x00 =>
            /// PWM control register.
            rw cr: u32 {
                /// Channel enable (one bit per channel).
                ch_en: 0..=3,
                /// Prescaler (0 = /1, 1 = /2, … 7 = /128).
                psc:   4..=6
            },
        0x04 =>
            /// PWM period register (in timer ticks).
            rw period: u32,
        0x08 =>
            /// PWM duty cycle register.
            rw duty: u32,
        0x0C =>
            /// PWM status (read-only).
            ro sr: u32 {
                /// Currently running.
                running: 0
            }
    }
}

#[tokio::main]
async fn main() {
    let devmem = unsafe { DevMem::new(0x4001_0000, None).unwrap() };
    let regs = unsafe { PwmRegs::new(Arc::new(devmem)).unwrap() };
    let regs = Arc::new(Mutex::new(regs));

    let app = ddevmem::web::WebUi::new()
        .add("pwm", regs)
        .build();

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    println!("Register map UI at http://localhost:3000");
    axum::serve(listener, app).await.unwrap();
}
```

**With HTTP Basic authentication:**

> **Security note.** HTTP Basic transmits credentials `base64`-encoded, **not
> encrypted** — always run the server behind TLS (e.g. `nginx`, `caddy`,
> `axum-server` + `rustls`) for anything beyond a trusted local network.
> Compare secrets in **constant time** with [`ct_eq`](https://docs.rs/ddevmem/latest/ddevmem/web/fn.ct_eq.html)
> instead of `==` to avoid leaking the password through response timing,
> and use bitwise `&` (not `&&`) so both comparisons run unconditionally.

`with_auth` takes an **async** callback `Fn(String, String) -> Future<Output = bool>`,
so the closure body must be an `async move { ... }` block. This lets the
check perform I/O (e.g. database lookup) without blocking the runtime.

```rust,no_run
use std::sync::Arc;
use tokio::sync::Mutex;
use ddevmem::{register_map, DevMem};
use ddevmem::web::{ct_eq, WebUi};

register_map! {
    pub unsafe map R (u32) { 0x00 => rw x: u32 }
}

async fn build_apps(regs: Arc<Mutex<R>>) {
    // Static credentials (constant-time comparison).
    let _app = WebUi::new()
        .add("r", regs.clone())
        .with_auth(|user, pass| async move {
            ct_eq(&user, "admin") & ct_eq(&pass, "hunter2")
        })
        .build();

    // Or validate against an external source (sync or async — both work
    // inside the `async move` block).
    let _app = WebUi::new()
        .add("r", regs)
        .with_auth(|user, pass| async move {
            my_auth_db::check(&user, &pass).await
        })
        .build();
}

mod my_auth_db {
    pub async fn check(_u: &str, _p: &str) -> bool { true }
}
```

The web UI provides:

- Live register values with auto-refresh
- Per-register and per-bitfield read/write controls
- Documentation strings from `/// ...` comments
- JSON API for integration with external tools
- **Nestable router** — mount the web UI at any prefix on a larger server

The returned `Router` has no root path baked in.
Use `axum::Router::nest()` to mount it wherever you need:

```rust,no_run
use std::sync::Arc;
use tokio::sync::Mutex;
use ddevmem::{register_map, DevMem};
use ddevmem::web::WebUi;

register_map! {
    pub unsafe map R (u32) { 0x00 => rw x: u32 }
}

async fn run(regs: Arc<Mutex<R>>) {
    // Mount at a custom prefix:
    let app = axum::Router::new().nest(
        "/registers/axi",
        WebUi::new().add("axi", regs).build(),
    );
    // ... axum::serve(listener, app).await.unwrap();
    let _ = app;
}
```

**API endpoints** (relative to mount point):

| Method | Path                | Body                           | Response                                              |
| ------ | ------------------- | ------------------------------ | ----------------------------------------------------- |
| GET    | `/`                 | —                              | HTML single-page app                                  |
| GET    | `/api/maps`         | —                              | `{ title?: string, maps: [{ slug, name }, ...] }`     |
| GET    | `/api/{slug}/info`  | —                              | `{ name, bus_width, base_address, registers: [...] }` |
| POST   | `/api/{slug}/read`  | `{ "offset": 0 }`              | `{ "value": 12345 }`                                  |
| POST   | `/api/{slug}/write` | `{ "offset": 0, "value": 42 }` | `200 OK`                                              |

**Custom page title:**

The heading shown in the browser tab and the UI-Shell header defaults to
`ddevmem — Register Maps` (or the map's own name in single-map mode).
Override it with [`WebUi::with_title`](https://docs.rs/ddevmem/latest/ddevmem/web/struct.WebUi.html#method.with_title):

```rust,no_run
use std::sync::Arc;
use tokio::sync::Mutex;
use ddevmem::{register_map, DevMem};
use ddevmem::web::WebUi;

register_map! {
    pub unsafe map R (u32) { 0x00 => rw x: u32 }
}

async fn run(regs: Arc<Mutex<R>>) {
    let _app = WebUi::new()
        .with_title("Acme SoC — Hardware Registers")
        .add("r", regs)
        .build();
}
```

**Hosting multiple register maps on one page:**

The same `WebUi` builder accepts several `.add(slug, regs)` calls.
All maps are displayed together on a single page.

```rust,no_run
use std::sync::Arc;
use tokio::sync::Mutex;
use ddevmem::{register_map, DevMem};
use ddevmem::web::WebUi;

register_map! {
    pub unsafe map Spi (u32) { 0x00 => rw cr: u32 }
}
register_map! {
    pub unsafe map Gpio (u32) { 0x00 => rw data: u32 }
}

async fn run(spi: Arc<Mutex<Spi>>, gpio: Arc<Mutex<Gpio>>) {
    let app = axum::Router::new().nest(
        "/hw",
        WebUi::new()
            .add("spi", spi)
            .add("gpio", gpio)
            .build(),
    );

    // With auth:
    // let r = WebUi::new()
    //     .add("spi", spi_regs)
    //     .add("gpio", gpio_regs)
    //     .with_auth(|u, p| async move { u == "admin" && p == "secret" })
    //     .build();

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
```

### Using the emulator for testing

The `emulator` feature replaces `/dev/mem` with a zero-initialized heap buffer,
allowing you to test register map logic without hardware:

```rust,no_run
// Cargo.toml:
// ddevmem = { version = "0.4.0", default-features = false, features = ["emulator", "register-map"] }

use std::sync::Arc;
use ddevmem::{register_map, DevMem};

register_map! {
    pub unsafe map TestRegs (u32) {
        0x00 => rw data: u32,
        0x04 => rw ctrl: u32 {
            run: 0,
            irq_en: 1
        }
    }
}

// DevMem backed by Vec<u8> — no /dev/mem needed
let devmem = unsafe { DevMem::new(0x0, Some(256)).unwrap() };
let mut regs = unsafe { TestRegs::new(Arc::new(devmem)).unwrap() };

regs.set_data(0xCAFE);
assert_eq!(regs.data(), 0xCAFE);

regs.set_ctrl_run(1);
assert_eq!(regs.ctrl_run(), 1);
assert_eq!(regs.ctrl_irq_en(), 0); // other bits untouched
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
| No doc comment support              | `/// …` on registers & bitfields   |
| No typed bitfield support           | `as bool` / `as u8` / `as enum`    |
| No register-array support           | `rw fifo: [u32; 8]` (indexed API)  |
| No web UI                           | `web` feature with `axum` server   |

## Examples

The crate ships several runnable examples under [`examples/`](./examples).
Each one enables the `emulator` feature, so they work without `/dev/mem`.

| File                | Topic                                                              |
| ------------------- | ------------------------------------------------------------------ |
| `default_bus.rs`    | Minimal register map with `rw` / `ro` / `wo` access.               |
| `bitfield.rs`       | Plain numeric bitfields, doc comments.                             |
| `typed_bitfield.rs` | Typed bitfields: `as bool`, `as u8`, `as enum`.                    |
| `array_regs.rs`     | Register arrays (`[T; N]`) with per-element bitfields.             |
| `web_server.rs`     | Single map served via the `web` feature.                           |
| `web_auth.rs`       | Web UI behind HTTP Basic auth (constant-time `ct_eq`).             |
| `web_same_map.rs`   | Two instances of the same map at different base addresses.         |
| `web_showcase.rs`   | Full-feature showcase: 4 peripherals, every bitfield kind, arrays. |

Run any of them with:

```sh
cargo run --example <name>
```

## License

ddevmem is distributed under the terms of the [MIT license](https://opensource.org/licenses/MIT).
See [LICENSE-MIT](./LICENSE-MIT) for details.

[crates.io]: https://crates.io/crates/ddevmem
[latest version]: https://img.shields.io/crates/v/ddevmem.svg
[docs.rs]: https://docs.rs/ddevmem
[documentation]: https://docs.rs/ddevmem/badge.svg
[downloads]: https://img.shields.io/crates/d/ddevmem
[license]: https://img.shields.io/crates/l/ddevmem.svg