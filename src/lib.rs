//! # ddevmem
//!
//! Safe and ergonomic access to physical memory via `/dev/mem`, with volatile
//! read/write semantics suitable for memory-mapped I/O (MMIO).
//!
//! This crate provides:
//!
//! - [`DevMem`] — a memory-mapped view of a physical address range with
//!   volatile read, write, and modify operations (checked and unchecked).
//! - [`register_map!`] — a declarative macro for defining named register maps
//!   with bus-width enforcement, bitfield accessors, typed bitfields
//!   (`as bool` / `as u8` / `as enum`), and register arrays (requires the
//!   `register-map` feature).
//! - [`web`] — an optional [`axum`]-based web UI for viewing and editing
//!   registers at runtime (requires the `web` feature).
//!
//! ## Feature flags
//!
//! | Feature        | Default | Description |
//! |----------------|---------|-------------|
//! | `device`       | yes     | Real `/dev/mem` backend via `memmap2`. |
//! | `emulator`     | no      | Page-aligned heap buffer for testing without hardware. |
//! | `register-map` | yes     | The [`register_map!`] macro. |
//! | `web`          | no      | Web UI for register maps. |
//!
//! When both `device` and `emulator` are enabled, `emulator` takes
//! precedence. This lets tests and examples opt into emulation through
//! dev-dependencies without touching the default features.
//!
//! ## Quick start
//!
//! ```rust,no_run
//! use std::sync::Arc;
//! use ddevmem::{register_map, DevMem};
//!
//! register_map! {
//!     pub unsafe map Regs (u32) {
//!         0x00 => rw control: u32 {
//!             enable: 0,
//!             mode:   1..=3
//!         },
//!         0x04 => ro status:  u32,
//!         0x08 => wo command: u32
//!     }
//! }
//!
//! let devmem = unsafe { DevMem::new(0x4000_0000, None).unwrap() };
//! let mut regs = unsafe { Regs::new(Arc::new(devmem)).unwrap() };
//!
//! let status = regs.status();          // full-register read
//! let enabled = regs.control_enable(); // bitfield read
//! regs.set_control_mode(0b101);        // bitfield read-modify-write
//! regs.set_command(0xFF);              // full-register write
//! regs.modify_control(|v| v | 1);      // full-register read-modify-write
//! ```

#[cfg(any(feature = "device", feature = "emulator"))]
mod devmem;

#[cfg(any(feature = "device", feature = "emulator"))]
#[doc(inline)]
pub use devmem::{DevMem, Error};

#[cfg(feature = "web")]
pub mod web;

/// Declares a named register map backed by a [`DevMem`] instance.
///
/// ```rust,no_run
/// # use ddevmem::register_map;
/// register_map! {
///     /// SPI controller.
///     pub unsafe map SpiRegs (u32) {
///         0x00 =>
///             /// Control register.
///             rw cr: u32 {
///                 /// Chip select (0–7).
///                 cs:     0..=2,
///                 /// Clock polarity.
///                 cpol:   3,
///                 /// Transfer enable.
///                 enable: 5 as bool
///             },
///         0x04 =>
///             /// Status register.
///             ro sr: u32,
///         0x08 => wo cmd: u32
///     }
/// }
/// ```
///
/// Each entry is `offset => access name: type`, optionally followed by a
/// `{ ... }` block of bitfields:
///
/// - `offset` — byte offset of the register. Must be aligned to the bus
///   width; literal offsets are checked during macro expansion, expressions
///   by `const` assertions.
/// - `access` — `rw` (read-write), `ro` (read-only), or `wo` (write-only).
/// - `type` — `u8`, `u16`, `u32`, `u64`, or `usize`; must not be wider than
///   the bus. An array form `[type; N]` declares a register array (below).
///
/// The optional parenthesized type after the map name is the **bus width**:
/// every access is performed with a volatile load/store of exactly this type
/// (e.g. `u32` for AXI-Lite, which routes 32-bit transactions). Register
/// types narrower than the bus are zero-extended on write and truncated on
/// read. When omitted, the bus defaults to `usize` — the native pointer
/// width.
///
/// # Bitfields
///
/// Within a bitfield block, each field is a single bit (`field: 3`) or a bit
/// range — `field: 4..=7` (inclusive) or `field: 4..8` (exclusive upper
/// bound). Bits not covered by any field are preserved on write: bitfield
/// setters on `rw` registers do a volatile read-modify-write. There is no
/// need to declare reserved gaps.
///
/// On **`wo` registers** a read-modify-write is impossible, so a bitfield
/// setter writes the field value with **all other bits zero** — the usual
/// semantics of self-clearing command registers.
///
/// Bit positions may be integer literals or parenthesized const expressions
/// (`field: (BASE)..=(BASE + 3)`).
///
/// ## Typed bitfields
///
/// A field may carry an `as <type>` suffix that changes the getter/setter
/// types:
///
/// - `field: 0 as bool` — `bool` getter/setter (single-bit fields only);
/// - `field: 4..=7 as u8` — any unsigned integer wide enough for the field;
/// - `field: 6..=7 as enum Mode { A = 0, B = 1 }` — generates
///   `#[derive(Debug, Clone, Copy, PartialEq, Eq)] enum Mode` with
///   `from_raw()` / `to_raw()`; raw values not matching any variant map to
///   the first declared variant.
///
/// # Register arrays
///
/// A register declared as `[T; N]` is a run of `N` identical registers laid
/// out one bus word apart. Its accessors take an `idx: usize` parameter
/// (panicking on out-of-range indices), a `{name}_len()` method returns `N`,
/// and bitfields declared on the entry are indexed the same way:
///
/// ```rust,no_run
/// # use ddevmem::register_map;
/// register_map! {
///     pub unsafe map Dma (u32) {
///         0x10 => rw fifo: [u32; 8],            // fifo(i), set_fifo(i, v)
///         0x40 => rw chan: [u32; 4] {           // chan(i), set_chan(i, v)
///             enable: 0     as bool,            // chan_enable(i), set_chan_enable(i, b)
///             prio:   1..=3 as u8               // chan_prio(i),   set_chan_prio(i, n)
///         }
///     }
/// }
/// ```
///
/// # Generated API
///
/// For a register named `cr` (types shown for a `u32` register):
///
/// | Access      | Method            | Signature                          |
/// |-------------|-------------------|------------------------------------|
/// | all         | `cr_offset()`     | `fn(&self) -> usize`               |
/// | all         | `cr_address()`    | `fn(&self) -> usize`               |
/// | `rw` / `ro` | `cr()`            | `fn(&self) -> u32`                 |
/// | `rw` / `wo` | `set_cr(value)`   | `fn(&mut self, u32)`               |
/// | `rw`        | `modify_cr(f)`    | `fn(&mut self, impl FnOnce(u32) -> u32)` |
///
/// For a bitfield `enable` on `cr`, `cr_enable()` and `set_cr_enable(value)`
/// are generated analogously; with an `as` suffix the value type becomes the
/// specified one. Array accessors take a leading `idx: usize` parameter.
///
/// The struct's rustdoc includes a generated register summary table, and all
/// `/// ...` comments (on the map, registers, and bitfields) are forwarded to
/// the generated items — and shown in the web UI when the `web` feature is
/// enabled.
///
/// # Compile-time validation
///
/// Misaligned offsets, bit ranges that exceed the register type, `as bool`
/// on multi-bit fields, enum values that don't fit their field, casts
/// narrower than the field, and name collisions between generated methods
/// are reported as compile errors pointing at the offending token. Checks
/// that depend on the target (`usize` widths, non-literal expressions) are
/// enforced by generated `const` assertions.
///
/// # Safety
///
/// The generated `new()` is `unsafe`: [`DevMem`] does not track which
/// regions are claimed, so the caller must ensure no overlapping maps alias
/// the same memory. `new()` returns `None` when the region is too short for
/// the declared registers; all subsequent accesses are then in bounds and
/// need no per-access checks.
#[cfg(feature = "register-map")]
pub use ddevmem_macros::register_map;
