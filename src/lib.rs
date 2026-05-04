//! # ddevmem
//!
//! Safe and ergonomic access to physical memory via `/dev/mem`, with volatile
//! read/write semantics suitable for memory-mapped I/O (MMIO).
//!
//! This crate provides:
//!
//! - [`DevMem`] — memory-mapped access to a physical address range with
//!   volatile read, write, and modify operations.
//! - [`register_map!`] — declarative macro for defining named register maps
//!   with optional bus-width enforcement, bitfield accessors, and typed
//!   bitfields (`as bool` / `as u8` / `as enum`) (requires the
//!   `register-map` feature).
//!
//! ## Feature flags
//!
//! | Feature          | Default | Description |
//! |------------------|---------|-------------|
//! | `device`         | yes     | Real `/dev/mem` backend via `memmap2`. |
//! | `emulator`       | no      | In-memory `Vec<u8>` backend for testing without hardware. |
//! | `register-map`   | yes     | [`register_map!`] macro with bitfields and typed accessors. |
//! | `web`            | no      | Web UI for viewing/editing registers via [`axum`]. |
//!
//! Enable exactly one of `device` or `emulator`. Both enabled simultaneously is
//! a compile error.
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
//! // Read a full register
//! let status = regs.status();
//!
//! // Read a single-bit bitfield
//! let enabled = regs.control_enable();
//!
//! // Write a multi-bit bitfield (read-modify-write)
//! regs.set_control_mode(0b101);
//!
//! // Write a full register
//! regs.set_command(0xFF);
//!
//! // Read-modify-write
//! regs.modify_control(|v| v | 1);
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
/// Each entry specifies an offset, access kind (`rw` / `ro` / `wo`), a name,
/// and a type. An optional bus-width type can be given in parentheses after
/// the map name to enforce that every register access goes through the
/// bus-native width (e.g. `u32` for AXI-Lite); register types narrower than
/// the bus are truncated / zero-extended automatically.
///
/// When no bus width is specified the default is `usize` (the native pointer
/// width). On a 32-bit target this equals `u32`; on 64-bit it equals `u64`.
/// All register offsets must be aligned to the bus width.
///
/// Registers may carry bitfield blocks. Within a bitfield block, each field
/// is either a single bit (`field: bit`) or a range (`field: lo..=hi` or
/// `field: lo..hi`). `..=` is an inclusive range; `..` is exclusive on the
/// upper bound (consistent with Rust syntax). Bits not covered by any field
/// are left untouched during read-modify-write — there is no need to declare
/// reserved gaps.
///
/// ## Typed bitfields
///
/// A bitfield can carry an `as <type>` suffix to change the getter/setter
/// types:
///
/// - `field: bit as bool` — getter returns `bool`, setter accepts `bool`.
/// - `field: lo..=hi as u8` — getter returns `u8`, setter accepts `u8`
///   (any integer type is supported).
/// - `field: lo..=hi as enum Name { Variant = value, ... }` — generates a
///   `#[derive(Debug, Clone, Copy, PartialEq, Eq)]` enum with `from_raw()`
///   and `to_raw()` methods. Unknown raw values map to the first variant.
///
/// ## Register arrays
///
/// A register declared as `[T; N]` represents `N` consecutive identical
/// registers at `offset, offset + size_of::<bus>(), …`. Generated accessors
/// take an extra `idx: usize` parameter, and a `name_len()` method returns
/// `N`. Bitfields declared on an array entry are also indexed.
///
/// ```rust,ignore
/// register_map! {
///     pub unsafe map Dma (u32) {
///         0x10 => rw fifo: [u32; 8],            // fifo(i), set_fifo(i, v)
///         0x40 => rw chan: [u32; 4] {           // chan(i), set_chan(i, v)
///             enable: 0    as bool,             // chan_enable(i), set_chan_enable(i, b)
///             prio:   1..=3 as u8               // chan_prio(i),   set_chan_prio(i, n)
///         }
///     }
/// }
/// ```
///
/// # Generated API
///
/// For a register named `ctrl` the following methods are generated:
///
/// | Kind | Method | Signature |
/// |------|--------|-----------|
/// | all  | `ctrl_offset()` | `fn(&self) -> usize` |
/// | all  | `ctrl_address()` | `fn(&self) -> usize` |
/// | `rw` / `ro` | `ctrl()` | `fn(&self) -> T` |
/// | `rw` / `wo` | `set_ctrl(value)` | `fn(&mut self, T)` |
/// | `rw` | `modify_ctrl(f)` | `fn(&mut self, FnOnce(T) -> T)` |
///
/// For a bitfield `enable` on register `ctrl`:
///
/// | Kind | Method | Signature |
/// |------|--------|-----------|
/// | `rw` / `ro` | `ctrl_enable()` | `fn(&self) -> T` |
/// | `rw` / `wo` | `set_ctrl_enable(value)` | `fn(&mut self, T)` |
///
/// When a type suffix is present, `T` becomes the specified type (`bool`,
/// `u8`, or the generated enum).
/// # Safety
///
/// The macro-generated `new()` is `unsafe` because [`DevMem`] does not track
/// which regions are claimed by register maps. The caller must ensure no
/// overlapping maps alias the same memory.
///
/// # Documentation
///
/// Doc comments (`/// ...`) can be placed on the register map struct itself,
/// on individual registers (after `=>`), and on individual bitfields.
/// They are forwarded to the generated methods and, when the `web` feature
/// is enabled, displayed in the web UI.
///
/// ```rust,no_run
/// # use ddevmem::register_map;
/// register_map! {
///     /// My peripheral.
///     pub unsafe map MyRegs (u32) {
///         0x00 =>
///             /// Control register
///             rw control: u32 {
///                 /// Enable the peripheral
///                 enable: 0,
///                 /// Operating mode (0-7)
///                 mode: 1..=3
///             },
///         0x04 =>
///             /// Status register (read-only)
///             ro status: u32
///     }
/// }
/// ```
///
/// # Examples
///
/// With explicit bus width (recommended for FPGA / AXI-Lite):
///
/// ```rust,no_run
/// # use ddevmem::register_map;
/// register_map! {
///     pub unsafe map Axi (u32) {
///         0x00 => rw control: u32 {
///             enable:    0,
///             mode:      1..=3,
///             threshold: 4..=7
///         },
///         0x04 => ro status: u32 {
///             ready: 0,
///             error: 1
///         },
///         0x08 => wo command: u32
///     }
/// }
/// ```
///
/// Without bus width (defaults to `usize`):
///
/// ```rust,ignore
/// # use ddevmem::register_map;
/// register_map! {
///     pub unsafe map Plain {
///         0x00 => rw data:   u32,
///         0x04 => ro status: u32,
///         0x08 => wo cmd:    u32
///     }
/// }
/// ```
///
/// With typed bitfields:
///
/// ```rust,no_run
/// # use ddevmem::register_map;
/// register_map! {
///     pub unsafe map Timer (u32) {
///         0x00 => rw cr: u32 {
///             enable: 0 as bool,
///             psc:    2..=5 as u8,
///             mode:   6..=7 as enum TimerMode {
///                 Stopped  = 0,
///                 OneShot  = 1,
///                 FreeRun  = 2,
///                 External = 3,
///             },
///         }
///     }
/// }
/// ```
#[cfg(feature = "register-map")]
pub use ddevmem_macros::register_map;
