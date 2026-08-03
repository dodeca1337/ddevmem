# ddevmem

[![Latest Version]][crates.io] [![Documentation]][docs.rs] ![Downloads] ![License]

**Talk to memory-mapped hardware from ordinary Linux userspace — no kernel
module, no `volatile` boilerplate.**

You describe a peripheral's registers once, the way the datasheet lists them,
and `ddevmem` turns that description into a typed API over `/dev/mem`: volatile
reads and writes the compiler will not reorder, merge, or optimize away;
bitfield accessors that touch only their own bits; and — if you want one — a
browser UI to poke at the registers live while the board is running.

It is aimed at the everyday embedded-Linux situation: custom AXI-Lite IP in
FPGA fabric, an SoC peripheral the vendor never wrote a driver for, a quick
bring-up script that has to twiddle a few control bits.

```rust
use std::sync::Arc;
use ddevmem::{register_map, DevMem};

register_map! {
    /// UART controller in the FPGA fabric.
    pub unsafe map Uart (u32) {
        0x00 =>
            /// Control register.
            rw cr: u32 {
                /// Transmitter enable.
                tx_en:  0 as bool,
                /// Word length in bits.
                len:    4..=7 as u8,
                /// Parity mode.
                parity: 8..=9 as enum Parity { None = 0, Even = 1, Odd = 2 },
            },
        0x04 =>
            /// Status register.
            ro sr: u32 {
                /// Transmit FIFO empty.
                tx_empty: 0 as bool
            },
        0x08 =>
            /// Transmit data register.
            wo txd: u32
    }
}

let devmem = unsafe { DevMem::new(0x43C0_0000, None) }.unwrap();
let mut uart = unsafe { Uart::new(Arc::new(devmem)) }.unwrap();

uart.set_cr_tx_en(true);           // read-modify-write of bit 0 only
uart.set_cr_len(8);                // bits 7:4, typed as u8
uart.set_cr_parity(Parity::Even);  // bits 9:8, typed as an enum

while !uart.sr_tx_empty() {}       // volatile poll — never hoisted out
uart.set_txd(b'!' as u32);
```

## Highlights

- **Register maps that read like a datasheet** — offsets, access kinds,
  bitfields, and register arrays in a single declarative block.
- **Typed bitfields** — `as bool`, `as u8`, or `as enum` with generated
  `from_raw()` / `to_raw()` conversions, so mode values stop being magic
  numbers.
- **Correct by construction** — every access is a volatile load/store at the
  declared bus width; bitfield writes preserve neighbouring bits; bounds are
  checked once in the constructor instead of on every access.
- **Mistakes caught at compile time** — misaligned offsets, bit ranges that
  don't fit the register, enum values too large for their field, colliding
  method names: each is an error pointing at the offending token, not a
  surprise at 3 a.m. on the bench.
- **Live web UI** (optional) — one self-contained page served by `axum`,
  showing every register and bitfield with the documentation from your `///`
  comments, plus a JSON API for scripting ([screenshots](#web-ui)).
- **Runs without hardware** — the `emulator` backend swaps `/dev/mem` for a
  page-aligned heap buffer, so register logic can be unit-tested on a laptop.

## Contents

- [Installation](#installation)
- [Requirements and caveats](#requirements-and-caveats)
- [Raw memory access with `DevMem`](#raw-memory-access-with-devmem)
- [Register maps](#register-maps)
  - [Syntax at a glance](#syntax-at-a-glance)
  - [The map header](#the-map-header)
  - [Register entries](#register-entries)
  - [Bitfields](#bitfields)
  - [Typed bitfields](#typed-bitfields)
  - [Register arrays](#register-arrays)
  - [Documentation comments](#documentation-comments)
  - [Generated API reference](#generated-api-reference)
  - [Compile-time checks](#compile-time-checks)
- [Web UI](#web-ui)
- [Testing without hardware](#testing-without-hardware)
- [Safety](#safety)
- [Migration from 0.4](#migration-from-04)
- [Examples](#examples)
- [License](#license)

## Installation

```toml
[dependencies]
ddevmem = "0.5.0"
```

With the web UI:

```toml
[dependencies]
ddevmem = { version = "0.5.0", features = ["web"] }
tokio = { version = "1", features = ["full"] }
```

For unit tests and desktop development, without touching real memory:

```toml
[dependencies]
ddevmem = { version = "0.5.0", default-features = false, features = ["emulator", "register-map"] }
```

### Feature flags

| Feature        | Default | Description                                                                       |
| -------------- | ------- | --------------------------------------------------------------------------------- |
| `device`       | ✓       | Real `/dev/mem` backend via `memmap2`.                                            |
| `register-map` | ✓       | The `register_map!` macro (bitfields, typed accessors, arrays).                   |
| `emulator`     |         | Page-aligned heap buffer instead of `/dev/mem`, for testing without hardware.     |
| `web`          |         | Browser UI and JSON API for register maps, served by `axum` (optional HTTP auth). |

When both `device` and `emulator` are enabled, **`emulator` wins**. That is
deliberate: it lets your own crate pull `ddevmem` in as a dev-dependency with
`features = ["emulator"]` so tests and examples run on a workstation, while the
real build still targets `/dev/mem` — no `default-features = false` dance.

## Requirements and caveats

Mapping physical memory is a privileged, sharp-edged operation. Before the
first `DevMem::new` call succeeds, check that:

- **The process can open `/dev/mem`** — that means root, or `CAP_SYS_RAWIO`.
- **The kernel allows the mapping.** Kernels built with `CONFIG_STRICT_DEVMEM`
  (most distro kernels) restrict which physical ranges `/dev/mem` will hand
  out. Device/MMIO regions are typically still reachable; system RAM is not.
  `CONFIG_IO_STRICT_DEVMEM` tightens this further, and refuses regions claimed
  by a kernel driver — if a driver already owns your peripheral, unbind it
  first.
- **The base address is page-aligned.** It is passed to `mmap` as a file
  offset, which the kernel requires to be a multiple of the page size. For a
  peripheral that does not start on a page boundary, map the page it lives in
  and put the remainder into your register offsets:

  ```rust
  // Peripheral at 0x4000_1800 → map the page at 0x4000_1000, offsets += 0x800.
  register_map! {
      pub unsafe map Regs (u32) {
          0x800 => rw cr: u32,
          0x804 => ro sr: u32
      }
  }
  ```

- **Nothing else is driving the same peripheral.** `/dev/mem` gives you an
  unsynchronized view of the hardware; a kernel driver poking the same
  registers concurrently will produce exactly the races you would expect.

## Raw memory access with `DevMem`

`DevMem` is the low-level layer: a mapped physical range with volatile
accessors. Use it directly for one-off pokes, or let a register map wrap it.

```rust
use ddevmem::DevMem;

let devmem = unsafe { DevMem::new(0x4000_0000, Some(0x1000)) }.unwrap();

// Volatile read / write. `None` means the access is out of bounds or the
// offset is not aligned for the value type.
let value: u32 = devmem.read(0x00).unwrap();
devmem.write(0x04, 0xDEAD_BEEFu32).unwrap();

// Volatile read-modify-write (not atomic).
devmem.modify::<u32>(0x00, |v| v | (1 << 8)).unwrap();

// Bulk transfers — one volatile access per element, never a memcpy.
let mut buf = [0u32; 4];
devmem.read_slice(0x10, &mut buf).unwrap();
devmem.write_slice(0x10, &[1, 2, 3, 4]).unwrap();

// Geometry.
assert_eq!(devmem.address(), 0x4000_0000);
assert_eq!(devmem.len(), 0x1000);
```

| Method                        | Description                                                              |
| ----------------------------- | ------------------------------------------------------------------------ |
| `new(address, size)`          | Maps `size` bytes (default: one page) at a **page-aligned** address.     |
| `read::<T>(offset)`           | Volatile read; `None` if out of bounds or misaligned for `T`.            |
| `write(offset, value)`        | Volatile write; same failure conditions.                                 |
| `modify::<T>(offset, f)`      | Volatile read → `f` → volatile write. **Not** atomic.                    |
| `read_slice` / `write_slice`  | Element-wise volatile transfer of a `&mut [T]` / `&[T]`.                 |
| `read_unchecked` / `write_unchecked` | `unsafe`, no bounds or alignment check — used by generated code.  |
| `address()` / `len()` / `is_empty()` | Geometry of the mapping.                                          |
| `as_ptr()`                    | Raw `*mut u8` to the first mapped byte, for hand-written access.         |

`T` must be a plain integer type (anything implementing `bytemuck`'s
`AnyBitPattern` for reads and `NoUninit` for writes).

Errors from `new` are `Error::Open` (could not open `/dev/mem` — usually
permissions) and `Error::Mmap` (the mapping itself failed — usually a
misaligned address or a kernel restriction). Both wrap the underlying
`std::io::Error` and convert back into one via `From`.

## Register maps

### Syntax at a glance

```text
register_map! {
    ATTR*  VIS  unsafe map  NAME  ( "(" BUS ")" )?  {
        ENTRY  ,  ENTRY  ,  …  ,?
    }
}

ENTRY   := OFFSET "=>" ATTR* ACCESS NAME ":" TYPE ( "{" FIELD "," … ,? "}" )?
ACCESS  := "rw" | "ro" | "wo"
TYPE    := INT | "[" INT ";" LEN "]"

FIELD   := ATTR* NAME ":" BITS ( "as" KIND )?
BITS    := POS                     // single bit
         | POS "..=" POS           // inclusive range
         | POS ".." POS            // exclusive upper bound
KIND    := "bool" | INT | "enum" NAME "{" VARIANT "," … ,? "}"
VARIANT := ATTR* NAME "=" CONST_EXPR

INT     := "u8" | "u16" | "u32" | "u64" | "usize"
POS     := integer literal | "(" CONST_EXPR ")"
ATTR    := /// doc comment, or any outer #[attribute]
```

`OFFSET`, `LEN`, and the `CONST_EXPR` forms accept any constant expression —
literals, `const` items, arithmetic on them. Commas between entries and
between bitfields are **required**; a trailing one is optional.

### The map header

```rust
register_map! {
    /// Doc comment for the generated struct.
    pub unsafe map Spi (u32) { 0x00 => rw cr: u32 }
    //  ^^^^^^ ^^^ ^^^  ^^^
    //  │      │   │    └── bus width (optional, defaults to `usize`)
    //  │      │   └─────── struct name
    //  │      └─────────── literal keyword
    //  └────────────────── acknowledges that `new()` will be unsafe
}
```

The **bus width** is the type used for every actual load and store. Set it to
what the interconnect transports — `u32` for AXI-Lite, for instance — and a
`u8` register still gets accessed with a single 32-bit transaction, as the
hardware expects. Values are zero-extended on write and truncated on read.

Omitting `(BUS)` defaults to `usize`, the native pointer width. Prefer stating
it explicitly: a map that is correct on a 64-bit host silently changes access
width when cross-compiled to a 32-bit target.

The map's visibility (`pub`, `pub(crate)`, …) is applied to the struct, its
accessors, and any generated enums.

### Register entries

```text
0x04 => rw ctrl: u32 { … }
^^^^    ^^ ^^^^  ^^^   ^^^
│       │  │     │     └── optional bitfield block
│       │  │     └──────── register type
│       │  └────────────── register name (drives every method name)
│       └───────────────── access kind
└───────────────────────── byte offset from the mapped base
```

| Element    | Rules                                                                                                                        |
| ---------- | ---------------------------------------------------------------------------------------------------------------------------- |
| **Offset** | Byte offset from the base address. Must be a multiple of the bus width.                                                      |
| **Access** | `rw` read-write, `ro` read-only (no setter generated), `wo` write-only (no getter generated).                                |
| **Name**   | Snake-case; becomes `name()`, `set_name()`, `name_offset()`, and the prefix of every bitfield method.                        |
| **Type**   | `u8` … `u64` or `usize`, at most as wide as the bus. `[T; N]` declares an array — see [Register arrays](#register-arrays).   |

Choosing the access kind is not cosmetic: on real hardware a read can have side
effects (popping a FIFO, clearing a latched flag), so `ro`/`wo` remove the
operations that would be wrong to perform. Two entries may share one offset
when the hardware aliases read and write behind the same address — the classic
UART data register:

```rust
register_map! {
    pub unsafe map Uart (u32) {
        0x00 => ro rx: u32,   // reading pops the RX FIFO
        0x00 => wo tx: u32    // writing pushes the TX FIFO
    }
}
```

### Bitfields

A register may carry a block of named bit ranges:

```rust
const BASE: u32 = 8;

register_map! {
    pub unsafe map Regs (u32) {
        0x00 => rw cr: u32 {
            enable: 0,        // single bit
            mode:   1..=3,    // inclusive range: bits 3, 2, 1
            level:  4..7,     // exclusive upper bound: bits 6, 5, 4
            speed:  (BASE)..=(BASE + 1)   // constant expressions, parenthesized
        }
    }
}
```

Both range forms exist so the declaration can follow whichever convention the
datasheet uses; `..=` matches the usual "bits 7:4" notation and is the one to
reach for by default.

Setters on `rw` registers perform a **read-modify-write and touch only their
own bits** — bits belonging to other fields, and bits you never declared at
all, are preserved. There is no need to declare reserved gaps.

On a `wo` register a read-modify-write is impossible (reading is not allowed,
and on hardware often meaningless), so a bitfield setter writes its field with
**all other bits zero**. That matches how self-clearing command registers
behave: `set_cmd_reset(true)` issues "reset, nothing else".

### Typed bitfields

Adding `as <kind>` changes the getter's return type and the setter's argument
type, so values arrive already interpreted:

| Suffix         | Getter returns    | Setter accepts   | Notes                                        |
| -------------- | ----------------- | ---------------- | -------------------------------------------- |
| *(none)*       | the register type | register type    | Raw value, shifted down to bit 0.            |
| `as bool`      | `bool`            | `bool`           | Single-bit fields only.                      |
| `as u8` (etc.) | `u8`              | `u8`             | Any unsigned type wide enough for the field. |
| `as enum Name` | `Name`            | `Name`           | Generates the enum; see below.               |

```rust
use std::sync::Arc;
use ddevmem::{register_map, DevMem};

register_map! {
    pub unsafe map Timer (u32) {
        0x00 => rw cr: u32 {
            /// Counter enable.
            enable: 0 as bool,
            /// Clock prescaler (0–15).
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

let devmem = unsafe { DevMem::new(0x4000_0000, None) }.unwrap();
let mut timer = unsafe { Timer::new(Arc::new(devmem)) }.unwrap();

timer.set_cr_enable(true);
timer.set_cr_psc(7);
timer.set_cr_mode(TimerMode::FreeRun);

assert_eq!(timer.cr_enable(), true);
assert_eq!(timer.cr_psc(), 7u8);
assert_eq!(timer.cr_mode(), TimerMode::FreeRun);
```

An `as enum` field generates a real Rust enum next to the map struct:

- derives `Debug`, `Clone`, `Copy`, `PartialEq`, `Eq`, and implements
  `Display` (same text as `Debug`);
- `Name::from_raw(raw)` converts a raw field value — values matching no
  variant fall back to the **first declared variant**, because hardware can
  always hand you a reserved encoding;
- `Name::to_raw()` converts back;
- variant values must fit the field's width, and duplicates are rejected at
  compile time.

### Register arrays

Declaring a register as `[T; N]` describes `N` identical registers laid out one
bus word apart, starting at the given offset:

```rust
use std::sync::Arc;
use ddevmem::{register_map, DevMem};

register_map! {
    pub unsafe map Dma (u32) {
        0x10 =>
            /// 8-entry data FIFO at 0x10, 0x14, … 0x2C.
            rw fifo: [u32; 8],
        0x40 =>
            /// Four channel-control registers, each with its own bitfields.
            rw chan: [u32; 4] {
                enable: 0     as bool,
                prio:   1..=3 as u8
            }
    }
}

let devmem = unsafe { DevMem::new(0x4000_0000, None) }.unwrap();
let mut dma = unsafe { Dma::new(Arc::new(devmem)) }.unwrap();

for i in 0..dma.fifo_len() {          // fifo_len() == 8
    dma.set_fifo(i, i as u32);        // accessors take a leading index
}
assert_eq!(dma.fifo(3), 3);
assert_eq!(dma.fifo_offset(3), 0x1C);

for i in 0..4 {
    dma.set_chan_enable(i, true);     // bitfields are indexed too
    dma.set_chan_prio(i, i as u8);
}
```

Indices are bounds-checked at runtime and panic with the register's name if out
of range. The array's full extent counts toward the region size that `new()`
requires.

### Documentation comments

`///` comments may be attached to the map, to individual registers (after the
`=>`), to bitfields, and to enum variants. Each one is forwarded to the
generated item, so it shows up in `cargo doc` and on hover in your editor — and
in the web UI, which is what turns that page into a browsable datasheet.

The generated struct's rustdoc also gets an **automatic summary table** of
every register with its offset, access kind, type, and first doc line.

Alongside the comments you write, the macro appends a generated line describing
what each accessor does, for example: *"Writes bits 7:4 of `cr` via
read-modify-write; the other bits are preserved."*

### Generated API reference

For a map named `Regs` with a register `cr` (type `u32`) and a bitfield `en`:

| Item                        | Signature                                         | Generated for          |
| --------------------------- | ------------------------------------------------- | ---------------------- |
| `Regs::new(devmem)`         | `unsafe fn(Arc<DevMem>) -> Option<Self>`          | always                 |
| `cr_offset()`               | `fn(&self) -> usize`                              | always                 |
| `cr_address()`              | `fn(&self) -> usize`                              | always                 |
| `cr()`                      | `fn(&self) -> u32`                                | `rw`, `ro`             |
| `set_cr(value)`             | `fn(&mut self, u32)`                              | `rw`, `wo`             |
| `modify_cr(f)`              | `fn(&mut self, impl FnOnce(u32) -> u32)`          | `rw`                   |
| `cr_en()`                   | `fn(&self) -> u32`                                | `rw`, `ro`             |
| `set_cr_en(value)`          | `fn(&mut self, u32)`                              | `rw`, `wo`             |

For an array register `fifo: [u32; N]` every accessor gains a leading
`idx: usize` parameter (`fifo(idx)`, `set_fifo(idx, value)`,
`fifo_offset(idx)`, `set_fifo_en(idx, value)`, …) and one extra method appears:

| Item          | Signature            | Description                    |
| ------------- | -------------------- | ------------------------------ |
| `fifo_len()`  | `fn(&self) -> usize` | Number of elements, i.e. `N`.  |

`new()` returns `None` when the mapped region is shorter than the declared
registers need; after that every access is known to be in bounds, so the
generated accessors carry no per-access checks. The struct is `Send + Sync`
(it holds only an `Arc<DevMem>`).

The generated code is designed to be readable — `cargo expand` shows one-line
accessors over a small set of private helpers, with masks already folded into
literals:

```rust,ignore
pub fn cr_psc(&self) -> u8 {
    ((self.__read(0x00) >> 2) & 0xF) as u8
}
pub fn set_chan_prio(&mut self, idx: usize, value: u8) {
    self.__update(self.chan_offset(idx), 0x7 << 1, ((value as u32) & 0x7) << 1)
}
```

### Compile-time checks

Most declaration mistakes are rejected while the macro expands, with the error
pointing at the token at fault:

| Mistake                                                | Reported as                                                             |
| ------------------------------------------------------ | ----------------------------------------------------------------------- |
| `0x02 => rw a: u32` on a `u32` bus                     | offset `0x2` is not aligned to the 4-byte bus width                     |
| `rw b: u64` on a `u32` bus                             | register type is wider than the `u32` bus (8 > 4 bytes)                 |
| `f: 7..=4`                                             | bitfield `f`: high bit 4 is below low bit 7                             |
| `g: 40` in a `u32` register                            | bitfield `g`: bit 40 does not exist in the 32-bit register type         |
| `h: 0..=2 as bool`                                     | `as bool` requires a single-bit field, but `h` spans 3 bits             |
| `i: 0..=9 as u8`                                       | cast type `u8` is narrower than the 10-bit field `i`                    |
| `e: 3..=4 as enum E { X = 9 }`                         | variant value `0x9` does not fit in the 2-bit field `e`                 |
| Two registers named `a`                                | this declaration generates a method named `a`, which an earlier one also generates |
| `rw d: [u32; 0]`                                       | register array length must be at least 1                                |
| Missing comma between entries                          | expected `,`                                                            |

Checks that cannot be resolved at expansion time — a `usize` register, an
offset built from a `const` from another crate — are emitted as `const`
assertions instead, so they still fail the build rather than the board.

## Web UI

Enabling the `web` feature makes `register_map!` additionally implement
`RegisterMapInfo`, exposing the map's metadata. `WebUi` turns any number of
maps into an `axum` router serving a single self-contained page — no CDN, no
build step.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="https://raw.githubusercontent.com/dodeca1337/ddevmem/master/docs/ui-dark.png">
  <img alt="The ddevmem web UI: a sidebar listing four register maps with the UART map expanded, and register cards showing values, bitfields and documentation." src="https://raw.githubusercontent.com/dodeca1337/ddevmem/master/docs/ui-light.png">
</picture>

*The `web_showcase` example: four peripherals on three bus widths. Every map
gets a collapsible sidebar group; the theme follows the browser and can be
toggled.*

```rust
use std::sync::Arc;
use tokio::sync::Mutex;
use ddevmem::{register_map, DevMem};
use ddevmem::web::WebUi;

register_map! {
    /// PWM controller.
    pub unsafe map Pwm (u32) {
        0x00 =>
            /// Control register.
            rw cr: u32 {
                /// Per-channel enable bits.
                ch_en: 0..=3,
                /// Prescaler (0 = /1, 1 = /2, … 7 = /128).
                psc: 4..=6
            },
        0x04 => rw period: u32,
        0x08 => rw duty: u32
    }
}

#[tokio::main]
async fn main() {
    let devmem = unsafe { DevMem::new(0x4001_0000, None) }.unwrap();
    let regs = unsafe { Pwm::new(Arc::new(devmem)) }.unwrap();

    let app = WebUi::new()
        .add("pwm", Arc::new(Mutex::new(regs)))
        .build();

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    println!("Register UI at http://localhost:3000");
    axum::serve(listener, app).await.unwrap();
}
```

The page gives you live values with optional 1 s auto-refresh, per-register and
per-bitfield write controls, your `///` docs inline, a text dump of all
registers, and a light/dark theme toggle. Each register becomes a card with its
bitfields broken out — `as bool` and `as enum` fields turn into dropdowns of
their variants, so nobody has to remember that parity 1 means even:

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="https://raw.githubusercontent.com/dodeca1337/ddevmem/master/docs/ui-register-dark.png">
  <img alt="A single register card: the UART control register with its six bitfields, each showing bit positions, decoded value, a set control and its documentation." src="https://raw.githubusercontent.com/dodeca1337/ddevmem/master/docs/ui-register-light.png">
</picture>

Register arrays are expanded element by element, so `fifo: [u32; 8]` is
addressable as `fifo[0]` … `fifo[7]` in the UI just as it is in Rust:

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="https://raw.githubusercontent.com/dodeca1337/ddevmem/master/docs/ui-arrays-dark.png">
  <img alt="The DMA map in the web UI, its sidebar listing fifo[0] through fifo[7] and chan[0] through chan[3] with their individual offsets." src="https://raw.githubusercontent.com/dodeca1337/ddevmem/master/docs/ui-arrays-light.png">
</picture>


**Several maps, one page.** Call `.add()` once per map; each gets a URL slug
(ASCII `[a-zA-Z0-9_-]`) and its own sidebar group. Two instances of the same
map type at different base addresses are fine.

```rust
register_map! { pub unsafe map Spi (u32) { 0x00 => rw cr: u32 } }
register_map! { pub unsafe map Gpio (u32) { 0x00 => rw data: u32 } }

fn build(spi: Arc<Mutex<Spi>>, gpio: Arc<Mutex<Gpio>>) -> axum::Router {
    axum::Router::new().nest(
        "/hw",
        WebUi::new()
            .with_title("Acme SoC — Hardware Registers")
            .add("spi", spi)
            .add("gpio", gpio)
            .build(),
    )
}
```

**Mounting.** The returned `Router` has no root path baked in: serve it
directly, or nest it under a prefix as above (then browse to `/hw` — note that
`axum` does not allow nesting at `"/"`, and that `/hw/` with a trailing slash
does not match).

**Authentication.** `with_auth` adds HTTP Basic auth to every endpoint. The
callback is async, so it can query a database or an auth service; for static
credentials use `ct_eq`, and combine checks with bitwise `&` rather than `&&`
so both comparisons always run:

```rust
use ddevmem::web::{ct_eq, WebUi};

fn build(regs: Arc<Mutex<Pwm>>) -> axum::Router {
    WebUi::new()
        .add("pwm", regs)
        .with_auth(|user, pass| async move {
            ct_eq(&user, "admin") & ct_eq(&pass, "hunter2")
        })
        .build()
}
```

> **Security.** HTTP Basic sends credentials `base64`-encoded, **not
> encrypted**. Treat the UI as a trusted-network tool (lab bench, internal
> VLAN, SSH tunnel); anything exposed belongs behind TLS (`nginx`, `caddy`,
> `axum-server` + `rustls`). Comparing secrets with `==` leaks them through
> response timing — that is what `ct_eq` is for. There is no built-in CSRF
> protection or rate limiting; a reverse proxy enforcing `Origin`/`Referer`
> checks covers both.

### HTTP API

All paths are relative to the mount point, so the UI can be scripted with
`curl` just as easily as clicked:

| Method | Path                | Body                           | Response                                              |
| ------ | ------------------- | ------------------------------ | ----------------------------------------------------- |
| GET    | `/`                 | —                              | The HTML page                                         |
| GET    | `/api/maps`         | —                              | `{ title?: string, maps: [{ slug, name }, …] }`       |
| GET    | `/api/{slug}/info`  | —                              | `{ name, bus_width, base_address, registers: [...] }` |
| POST   | `/api/{slug}/read`  | `{ "offset": 0 }`              | `{ "value": 12345, "hex": "0x3039" }`                 |
| POST   | `/api/{slug}/write` | `{ "offset": 0, "value": 42 }` | `200 OK`                                              |

`value` on write may be a JSON number or a string (`"0x2A"`, `"42"`); the
string form carries the full 64-bit range, which JSON numbers lose above 2⁵³.
Reads return both forms for the same reason.

Requests are validated against the declared map: an offset that is unknown,
misaligned, or belongs to a register that cannot be read (respectively
written) is rejected with `400 Bad Request`, as is a value too wide for the
bus. The UI cannot reach memory your map does not describe.

## Testing without hardware

The `emulator` feature replaces `/dev/mem` with a page-aligned, zero-filled
heap buffer. The API is identical, so register logic can be exercised in
ordinary unit tests.

The usual setup keeps the real backend for the build and switches to the
emulator for tests — listing the crate twice is all it takes, because Cargo
unifies the two feature sets only for targets that use dev-dependencies, and
`emulator` then takes precedence:

```toml
[dependencies]
ddevmem = "0.5.0"

[dev-dependencies]
# `cargo test` / `cargo run --example …` build against the emulator;
# `cargo build` still targets /dev/mem.
ddevmem = { version = "0.5.0", features = ["emulator"] }
```

```rust
use std::sync::Arc;
use ddevmem::{register_map, DevMem};

register_map! {
    pub unsafe map Regs (u32) {
        0x00 => rw data: u32,
        0x04 => rw ctrl: u32 {
            run: 0 as bool,
            irq_en: 1 as bool
        }
    }
}

// No /dev/mem, no root, no board.
let devmem = unsafe { DevMem::new(0x0, Some(256)) }.unwrap();
let mut regs = unsafe { Regs::new(Arc::new(devmem)) }.unwrap();

regs.set_data(0xCAFE);
assert_eq!(regs.data(), 0xCAFE);

regs.set_ctrl_run(true);
assert!(regs.ctrl_run());
assert!(!regs.ctrl_irq_en());   // neighbouring bits untouched
```

The buffer is page-aligned precisely so that alignment behaviour matches a real
mapping — an emulator test that passes will not hit an alignment fault on the
board.

## Safety

Two constructors are `unsafe`, each with a contract the compiler cannot check:

- **`DevMem::new`** maps arbitrary physical memory. The caller must ensure the
  range is one the process may map and that touching it is acceptable —
  reads and writes go to real devices, with real side effects.
- **`Map::new`** does not track claimed regions. The caller must ensure no
  other register map or mapping aliases the same memory with conflicting
  expectations.

Everything generated afterwards is safe to call: offsets were bounds-checked in
the constructor, array indices are checked on use, and bitfield math cannot go
out of range because it was validated at compile time.

`DevMem` is `Send + Sync` but performs **no internal synchronization** —
hardware registers cannot be protected by the type system. Share a map across
tasks with `Arc<Mutex<…>>` (which is exactly what `WebUi` requires).

## Migration from 0.4

`ddevmem` 0.5 is a cleanup release with a handful of **breaking** changes:

| 0.4                                                                                   | 0.5                                                                                                       |
| ------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------- |
| `Error::CantOpenFile` / `Error::CantMmapFile`                                         | `Error::Open` / `Error::Mmap`                                                                             |
| Bitfield setters on `wo` registers did a read-modify-write (reading a write-only register!) | They write the field with all other bits zero                                                        |
| `DevMem::read`/`write`/`modify` accepted misaligned offsets (undefined behaviour)      | Misaligned offsets return `None`; `read_unchecked` / `write_unchecked` added for generated code           |
| The web API accepted any offset and would write to `ro` registers                      | Offsets and access kinds are validated against the declared map                                           |
| Register types were unchecked                                                          | Must be `u8`/`u16`/`u32`/`u64`/`usize`; bit ranges, enum values, and name collisions are compile errors    |
| Missing commas between registers were silently accepted                                | Commas are required (a trailing one is still optional)                                                    |
| `/api/{slug}/read` returned `{ value }`                                                | Returns `{ value, hex }`; writes also accept string values, covering the full 64-bit range                |

The `register_map!` syntax itself is unchanged — existing maps compile as they
are, provided they were already well-formed.

## Examples

Every example runs against the emulator, so none of them need `/dev/mem` or
root:

| File                | Topic                                                                     |
| ------------------- | ------------------------------------------------------------------------- |
| `default_bus.rs`    | Minimal map: `rw` / `ro` / `wo`, offsets and addresses.                   |
| `bitfield.rs`       | Plain numeric bitfields and doc comments.                                 |
| `typed_bitfield.rs` | `as bool`, `as u8`, `as enum`, and `from_raw` fallback behaviour.         |
| `array_regs.rs`     | Register arrays (`[T; N]`) with per-element bitfields.                    |
| `web_server.rs`     | One map served on `http://localhost:3000`.                                |
| `web_auth.rs`       | Web UI behind HTTP Basic auth with constant-time comparison.              |
| `web_same_map.rs`   | Two instances of one map type at different base addresses, on `/hw`.      |
| `web_showcase.rs`   | Four peripherals on three bus widths, every bitfield kind, arrays.        |

```sh
cargo run --example typed_bitfield
cargo run --example web_showcase --features web   # then open http://localhost:8800/hw
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
