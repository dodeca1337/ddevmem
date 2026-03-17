//! # ddevmem
//!
//! Safe and ergonomic access to physical memory via `/dev/mem`, with volatile
//! read/write semantics suitable for memory-mapped I/O (MMIO).
//!
//! This crate provides:
//!
//! - [`DevMem`] — memory-mapped access to a physical address range with
//!   volatile read, write, and modify operations.
//! - [`Reg`](reg::Reg) / [`SliceReg`](reg::SliceReg) — typed register handles
//!   with compile-time read/write control (requires the `reg` feature).
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
//! | `reg`            | no      | [`Reg`](reg::Reg) and [`SliceReg`](reg::SliceReg) types. |
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

#[cfg(all(feature = "reg", any(feature = "device", feature = "emulator")))]
pub mod reg;

#[cfg(feature = "web")]
pub mod web;

#[cfg(feature = "register-map")]
#[doc(hidden)]
pub use concat_idents::concat_idents as __concat_idents;

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
#[macro_export]
macro_rules! register_map {
    // With bus width
    ($(#[$struct_meta:meta])* $vis: vis unsafe map $name: ident ($bus: ty) { $($tt:tt)+ }) => {
        $(#[$struct_meta])*
        $vis struct $name {
            devmem: std::sync::Arc<$crate::DevMem>,
        }

        // Generate any enums from `as enum` bitfields at module scope.
        $crate::__register_map_extract_enums!($vis ($bus) $($tt)+);

        impl $name {
            /// Creates a new register map wrapping the given [`DevMem`].
            ///
            /// Returns `None` if any declared register offset falls outside the
            /// mapped region.
            ///
            /// # Safety
            ///
            /// The caller must ensure no other map or register aliases the same
            /// memory range. [`DevMem`] does not track claimed regions.
            #[inline(always)]
            pub unsafe fn new(devmem: std::sync::Arc<$crate::DevMem>) -> Option<Self> {
                $crate::__register_map_check!(($bus) devmem $($tt)+);
                Some(Self { devmem })
            }

            $crate::__register_map_methods!($vis ($bus) $($tt)+);
        }

        unsafe impl Sync for $name {}
        unsafe impl Send for $name {}

        $crate::__register_map_web_impl!($name ($bus) $($tt)+);
    };

    // Without bus width — defaults to usize (native word width)
    ($(#[$struct_meta:meta])* $vis: vis unsafe map $name: ident { $($tt:tt)+ }) => {
        $crate::register_map!($(#[$struct_meta])* $vis unsafe map $name (usize) { $($tt)+ });
    };
}

/// Internal macro: generate `RegisterMapInfo` when `web` feature is active.
/// When `web` is not enabled this expands to nothing.
#[cfg(all(feature = "register-map", feature = "web"))]
#[macro_export]
#[doc(hidden)]
macro_rules! __register_map_web_impl {
    ($name:ident ($bus:ty) $($tt:tt)+) => {
        impl $crate::web::RegisterMapInfo for $name {
            fn map_name(&self) -> &'static str {
                stringify!($name)
            }

            fn bus_width(&self) -> usize {
                std::mem::size_of::<$bus>()
            }

            fn base_address(&self) -> usize {
                self.devmem.address()
            }

            fn registers(&self) -> Vec<$crate::web::RegisterInfo> {
                let mut regs = Vec::new();
                $crate::__register_map_collect_info!(regs ($bus) $($tt)+);
                regs
            }

            fn read_register(&self, offset: usize) -> Option<u64> {
                self.devmem.read::<$bus>(offset).map(|v| v as u64)
            }

            fn write_register(&mut self, offset: usize, value: u64) -> Option<()> {
                self.devmem.write::<$bus>(offset, value as $bus)
            }
        }
    };
}

#[cfg(all(feature = "register-map", not(feature = "web")))]
#[macro_export]
#[doc(hidden)]
macro_rules! __register_map_web_impl {
    ($name:ident ($bus:ty) $($tt:tt)+) => {};
}

/// Internal macro: collect register metadata for the web UI.
#[cfg(feature = "web")]
#[macro_export]
#[doc(hidden)]
macro_rules! __register_map_collect_info {
    // Entry with bitfields, more entries follow
    ($regs:ident ($bus:ty) $offset:expr => $(#[$meta:meta])* $kind:ident $name:ident : $ty:ty { $($fields:tt)* } , $($rest:tt)+) => {
        {
            let mut bitfields = Vec::new();
            $crate::__register_collect_bitfields!(bitfields $($fields)*);
            let doc = $crate::__extract_doc_str!($(#[$meta])*);
            $regs.push($crate::web::RegisterInfo {
                name: stringify!($name),
                doc,
                offset: $offset,
                access: stringify!($kind),
                width: std::mem::size_of::<$ty>() * 8,
                bitfields,
            });
        }
        $crate::__register_map_collect_info!($regs ($bus) $($rest)+);
    };
    // Entry with bitfields, last entry
    ($regs:ident ($bus:ty) $offset:expr => $(#[$meta:meta])* $kind:ident $name:ident : $ty:ty { $($fields:tt)* }) => {
        {
            let mut bitfields = Vec::new();
            $crate::__register_collect_bitfields!(bitfields $($fields)*);
            let doc = $crate::__extract_doc_str!($(#[$meta])*);
            $regs.push($crate::web::RegisterInfo {
                name: stringify!($name),
                doc,
                offset: $offset,
                access: stringify!($kind),
                width: std::mem::size_of::<$ty>() * 8,
                bitfields,
            });
        }
    };
    // Entry without bitfields, more entries follow
    ($regs:ident ($bus:ty) $offset:expr => $(#[$meta:meta])* $kind:ident $name:ident : $ty:ty , $($rest:tt)+) => {
        {
            let doc = $crate::__extract_doc_str!($(#[$meta])*);
            $regs.push($crate::web::RegisterInfo {
                name: stringify!($name),
                doc,
                offset: $offset,
                access: stringify!($kind),
                width: std::mem::size_of::<$ty>() * 8,
                bitfields: Vec::new(),
            });
        }
        $crate::__register_map_collect_info!($regs ($bus) $($rest)+);
    };
    // Entry without bitfields, last entry
    ($regs:ident ($bus:ty) $offset:expr => $(#[$meta:meta])* $kind:ident $name:ident : $ty:ty) => {
        {
            let doc = $crate::__extract_doc_str!($(#[$meta])*);
            $regs.push($crate::web::RegisterInfo {
                name: stringify!($name),
                doc,
                offset: $offset,
                access: stringify!($kind),
                width: std::mem::size_of::<$ty>() * 8,
                bitfields: Vec::new(),
            });
        }
    };
}

/// Internal macro: collect bitfield info for the web UI.
#[cfg(feature = "web")]
#[macro_export]
#[doc(hidden)]
macro_rules! __register_collect_bitfields {
    // empty
    ($bf:ident) => {};

    // ── as bool ──────────────────────────────────────────────────────────

    // Inclusive range as bool, more follow
    ($bf:ident $(#[$fmeta:meta])* $field:ident : $lo:tt ..= $hi:tt as bool , $($rest:tt)*) => {
        $crate::__push_bitfield_info!($bf $(#[$fmeta])* ; $field, $lo, $hi, "bool",
            vec![$crate::web::VariantInfo { name: "false", value: 0 },
                 $crate::web::VariantInfo { name: "true",  value: 1 }]);
        $crate::__register_collect_bitfields!($bf $($rest)*);
    };
    // Inclusive range as bool, last
    ($bf:ident $(#[$fmeta:meta])* $field:ident : $lo:tt ..= $hi:tt as bool) => {
        $crate::__push_bitfield_info!($bf $(#[$fmeta])* ; $field, $lo, $hi, "bool",
            vec![$crate::web::VariantInfo { name: "false", value: 0 },
                 $crate::web::VariantInfo { name: "true",  value: 1 }]);
    };

    // Exclusive range as bool, more follow
    ($bf:ident $(#[$fmeta:meta])* $field:ident : $lo:tt .. $hi:tt as bool , $($rest:tt)*) => {
        $crate::__push_bitfield_info!($bf $(#[$fmeta])* ; $field, $lo, $hi - 1, "bool",
            vec![$crate::web::VariantInfo { name: "false", value: 0 },
                 $crate::web::VariantInfo { name: "true",  value: 1 }]);
        $crate::__register_collect_bitfields!($bf $($rest)*);
    };
    // Exclusive range as bool, last
    ($bf:ident $(#[$fmeta:meta])* $field:ident : $lo:tt .. $hi:tt as bool) => {
        $crate::__push_bitfield_info!($bf $(#[$fmeta])* ; $field, $lo, $hi - 1, "bool",
            vec![$crate::web::VariantInfo { name: "false", value: 0 },
                 $crate::web::VariantInfo { name: "true",  value: 1 }]);
    };

    // Single bit as bool, more follow
    ($bf:ident $(#[$fmeta:meta])* $field:ident : $bit:tt as bool , $($rest:tt)*) => {
        $crate::__push_bitfield_info!($bf $(#[$fmeta])* ; $field, $bit, $bit, "bool",
            vec![$crate::web::VariantInfo { name: "false", value: 0 },
                 $crate::web::VariantInfo { name: "true",  value: 1 }]);
        $crate::__register_collect_bitfields!($bf $($rest)*);
    };
    // Single bit as bool, last
    ($bf:ident $(#[$fmeta:meta])* $field:ident : $bit:tt as bool) => {
        $crate::__push_bitfield_info!($bf $(#[$fmeta])* ; $field, $bit, $bit, "bool",
            vec![$crate::web::VariantInfo { name: "false", value: 0 },
                 $crate::web::VariantInfo { name: "true",  value: 1 }]);
    };

    // ── as enum ──────────────────────────────────────────────────────────

    // Inclusive range as enum, more follow
    ($bf:ident $(#[$fmeta:meta])* $field:ident : $lo:tt ..= $hi:tt as enum $ename:ident { $($ebody:tt)* } , $($rest:tt)*) => {
        $crate::__push_bitfield_info!($bf $(#[$fmeta])* ; $field, $lo, $hi, stringify!($ename),
            $crate::__enum_variants!($($ebody)*));
        $crate::__register_collect_bitfields!($bf $($rest)*);
    };
    // Inclusive range as enum, last
    ($bf:ident $(#[$fmeta:meta])* $field:ident : $lo:tt ..= $hi:tt as enum $ename:ident { $($ebody:tt)* }) => {
        $crate::__push_bitfield_info!($bf $(#[$fmeta])* ; $field, $lo, $hi, stringify!($ename),
            $crate::__enum_variants!($($ebody)*));
    };

    // Exclusive range as enum, more follow
    ($bf:ident $(#[$fmeta:meta])* $field:ident : $lo:tt .. $hi:tt as enum $ename:ident { $($ebody:tt)* } , $($rest:tt)*) => {
        $crate::__push_bitfield_info!($bf $(#[$fmeta])* ; $field, $lo, $hi - 1, stringify!($ename),
            $crate::__enum_variants!($($ebody)*));
        $crate::__register_collect_bitfields!($bf $($rest)*);
    };
    // Exclusive range as enum, last
    ($bf:ident $(#[$fmeta:meta])* $field:ident : $lo:tt .. $hi:tt as enum $ename:ident { $($ebody:tt)* }) => {
        $crate::__push_bitfield_info!($bf $(#[$fmeta])* ; $field, $lo, $hi - 1, stringify!($ename),
            $crate::__enum_variants!($($ebody)*));
    };

    // Single bit as enum, more follow
    ($bf:ident $(#[$fmeta:meta])* $field:ident : $bit:tt as enum $ename:ident { $($ebody:tt)* } , $($rest:tt)*) => {
        $crate::__push_bitfield_info!($bf $(#[$fmeta])* ; $field, $bit, $bit, stringify!($ename),
            $crate::__enum_variants!($($ebody)*));
        $crate::__register_collect_bitfields!($bf $($rest)*);
    };
    // Single bit as enum, last
    ($bf:ident $(#[$fmeta:meta])* $field:ident : $bit:tt as enum $ename:ident { $($ebody:tt)* }) => {
        $crate::__push_bitfield_info!($bf $(#[$fmeta])* ; $field, $bit, $bit, stringify!($ename),
            $crate::__enum_variants!($($ebody)*));
    };

    // ── as cast type ─────────────────────────────────────────────────────

    // Inclusive range as $cast_ty, more follow
    ($bf:ident $(#[$fmeta:meta])* $field:ident : $lo:tt ..= $hi:tt as $cast_ty:ty , $($rest:tt)*) => {
        $crate::__push_bitfield_info!($bf $(#[$fmeta])* ; $field, $lo, $hi, stringify!($cast_ty), Vec::new());
        $crate::__register_collect_bitfields!($bf $($rest)*);
    };
    // Inclusive range as $cast_ty, last
    ($bf:ident $(#[$fmeta:meta])* $field:ident : $lo:tt ..= $hi:tt as $cast_ty:ty) => {
        $crate::__push_bitfield_info!($bf $(#[$fmeta])* ; $field, $lo, $hi, stringify!($cast_ty), Vec::new());
    };

    // Exclusive range as $cast_ty, more follow
    ($bf:ident $(#[$fmeta:meta])* $field:ident : $lo:tt .. $hi:tt as $cast_ty:ty , $($rest:tt)*) => {
        $crate::__push_bitfield_info!($bf $(#[$fmeta])* ; $field, $lo, $hi - 1, stringify!($cast_ty), Vec::new());
        $crate::__register_collect_bitfields!($bf $($rest)*);
    };
    // Exclusive range as $cast_ty, last
    ($bf:ident $(#[$fmeta:meta])* $field:ident : $lo:tt .. $hi:tt as $cast_ty:ty) => {
        $crate::__push_bitfield_info!($bf $(#[$fmeta])* ; $field, $lo, $hi - 1, stringify!($cast_ty), Vec::new());
    };

    // Single bit as $cast_ty, more follow
    ($bf:ident $(#[$fmeta:meta])* $field:ident : $bit:tt as $cast_ty:ty , $($rest:tt)*) => {
        $crate::__push_bitfield_info!($bf $(#[$fmeta])* ; $field, $bit, $bit, stringify!($cast_ty), Vec::new());
        $crate::__register_collect_bitfields!($bf $($rest)*);
    };
    // Single bit as $cast_ty, last
    ($bf:ident $(#[$fmeta:meta])* $field:ident : $bit:tt as $cast_ty:ty) => {
        $crate::__push_bitfield_info!($bf $(#[$fmeta])* ; $field, $bit, $bit, stringify!($cast_ty), Vec::new());
    };

    // ── untyped (raw) ────────────────────────────────────────────────────

    // Inclusive range, more follow
    ($bf:ident $(#[$fmeta:meta])* $field:ident : $lo:tt ..= $hi:tt , $($rest:tt)*) => {
        $crate::__push_bitfield_info!($bf $(#[$fmeta])* ; $field, $lo, $hi, "raw", Vec::new());
        $crate::__register_collect_bitfields!($bf $($rest)*);
    };
    // Inclusive range, last
    ($bf:ident $(#[$fmeta:meta])* $field:ident : $lo:tt ..= $hi:tt) => {
        $crate::__push_bitfield_info!($bf $(#[$fmeta])* ; $field, $lo, $hi, "raw", Vec::new());
    };

    // Exclusive range, more follow
    ($bf:ident $(#[$fmeta:meta])* $field:ident : $lo:tt .. $hi:tt , $($rest:tt)*) => {
        $crate::__push_bitfield_info!($bf $(#[$fmeta])* ; $field, $lo, $hi - 1, "raw", Vec::new());
        $crate::__register_collect_bitfields!($bf $($rest)*);
    };
    // Exclusive range, last
    ($bf:ident $(#[$fmeta:meta])* $field:ident : $lo:tt .. $hi:tt) => {
        $crate::__push_bitfield_info!($bf $(#[$fmeta])* ; $field, $lo, $hi - 1, "raw", Vec::new());
    };

    // Single bit, more follow
    ($bf:ident $(#[$fmeta:meta])* $field:ident : $bit:tt , $($rest:tt)*) => {
        $crate::__push_bitfield_info!($bf $(#[$fmeta])* ; $field, $bit, $bit, "raw", Vec::new());
        $crate::__register_collect_bitfields!($bf $($rest)*);
    };
    // Single bit, last
    ($bf:ident $(#[$fmeta:meta])* $field:ident : $bit:tt) => {
        $crate::__push_bitfield_info!($bf $(#[$fmeta])* ; $field, $bit, $bit, "raw", Vec::new());
    };
}

/// Helper: push a `BitfieldInfo` with all fields.
#[cfg(feature = "web")]
#[macro_export]
#[doc(hidden)]
macro_rules! __push_bitfield_info {
    ($bf:ident $(#[$fmeta:meta])* ; $field:ident, $lo:expr, $hi:expr, $ft:expr, $variants:expr) => {
        {
            let doc = $crate::__extract_doc_str!($(#[$fmeta])*);
            $bf.push($crate::web::BitfieldInfo {
                name: stringify!($field),
                doc,
                lo: $lo,
                hi: $hi,
                field_type: $ft,
                variants: $variants,
            });
        }
    };
}

/// Helper: produce a `Vec<VariantInfo>` from enum body tokens.
#[cfg(feature = "web")]
#[macro_export]
#[doc(hidden)]
macro_rules! __enum_variants {
    ($($variant:ident = $val:expr),* $(,)?) => {
        vec![
            $($crate::web::VariantInfo {
                name: stringify!($variant),
                value: $val as u64,
            }),*
        ]
    };
}

/// Internal macro: extract doc string from attributes.
/// Concatenates all `#[doc = "..."]` attributes into a single `&'static str`.
/// If no doc attributes, returns `""`.
#[cfg(feature = "web")]
#[macro_export]
#[doc(hidden)]
macro_rules! __extract_doc_str {
    () => { "" };
    (#[doc = $doc:expr] $(#[$rest:meta])*) => {
        concat!($doc, $crate::__extract_doc_str!($(#[$rest])*))
    };
    (#[$other:meta] $(#[$rest:meta])*) => {
        $crate::__extract_doc_str!($(#[$rest])*)
    };
}

/// Internal macro: walk register entries and extract `as enum` definitions
/// at module scope (outside `impl` blocks).
#[cfg(feature = "register-map")]
#[macro_export]
#[doc(hidden)]
macro_rules! __register_map_extract_enums {
    // Entry with bitfields, more entries follow
    ($vis:vis ($bus:ty) $offset:expr => $(#[$meta:meta])* $kind:ident $name:ident : $ty:ty { $($fields:tt)* } , $($rest:tt)+) => {
        $crate::__register_extract_enums!($vis, $ty { $($fields)* });
        $crate::__register_map_extract_enums!($vis ($bus) $($rest)+);
    };
    // Entry with bitfields, last entry
    ($vis:vis ($bus:ty) $offset:expr => $(#[$meta:meta])* $kind:ident $name:ident : $ty:ty { $($fields:tt)* }) => {
        $crate::__register_extract_enums!($vis, $ty { $($fields)* });
    };
    // Entry without bitfields, more entries follow
    ($vis:vis ($bus:ty) $offset:expr => $(#[$meta:meta])* $kind:ident $name:ident : $ty:ty , $($rest:tt)+) => {
        $crate::__register_map_extract_enums!($vis ($bus) $($rest)+);
    };
    // Entry without bitfields, last entry
    ($vis:vis ($bus:ty) $offset:expr => $(#[$meta:meta])* $kind:ident $name:ident : $ty:ty) => {};
}

/// Internal macro: walk bitfield entries within one register and generate
/// `__register_enum!` for every `as enum` field; skip everything else.
#[cfg(feature = "register-map")]
#[macro_export]
#[doc(hidden)]
macro_rules! __register_extract_enums {
    // ── empty ────────────────────────────────────────────────────────────
    ($vis:vis, $ty:ty {}) => {};

    // ── enum fields (generate enum) ──────────────────────────────────────

    // Inclusive range, more follow
    ($vis:vis, $ty:ty {
        $(#[$fmeta:meta])* $field:ident : $lo:tt ..= $hi:tt as enum $ename:ident { $($ebody:tt)* } , $($rest:tt)*
    }) => {
        $crate::__register_enum!($vis $ename : $ty { $($ebody)* });
        $crate::__register_extract_enums!($vis, $ty { $($rest)* });
    };
    // Inclusive range, last
    ($vis:vis, $ty:ty {
        $(#[$fmeta:meta])* $field:ident : $lo:tt ..= $hi:tt as enum $ename:ident { $($ebody:tt)* }
    }) => {
        $crate::__register_enum!($vis $ename : $ty { $($ebody)* });
    };
    // Exclusive range, more follow
    ($vis:vis, $ty:ty {
        $(#[$fmeta:meta])* $field:ident : $lo:tt .. $hi:tt as enum $ename:ident { $($ebody:tt)* } , $($rest:tt)*
    }) => {
        $crate::__register_enum!($vis $ename : $ty { $($ebody)* });
        $crate::__register_extract_enums!($vis, $ty { $($rest)* });
    };
    // Exclusive range, last
    ($vis:vis, $ty:ty {
        $(#[$fmeta:meta])* $field:ident : $lo:tt .. $hi:tt as enum $ename:ident { $($ebody:tt)* }
    }) => {
        $crate::__register_enum!($vis $ename : $ty { $($ebody)* });
    };
    // Single bit, more follow
    ($vis:vis, $ty:ty {
        $(#[$fmeta:meta])* $field:ident : $bit:tt as enum $ename:ident { $($ebody:tt)* } , $($rest:tt)*
    }) => {
        $crate::__register_enum!($vis $ename : $ty { $($ebody)* });
        $crate::__register_extract_enums!($vis, $ty { $($rest)* });
    };
    // Single bit, last
    ($vis:vis, $ty:ty {
        $(#[$fmeta:meta])* $field:ident : $bit:tt as enum $ename:ident { $($ebody:tt)* }
    }) => {
        $crate::__register_enum!($vis $ename : $ty { $($ebody)* });
    };

    // ── as bool — skip ──────────────────────────────────────────────────

    ($vis:vis, $ty:ty { $(#[$fmeta:meta])* $field:ident : $lo:tt ..= $hi:tt as bool , $($rest:tt)* }) => {
        $crate::__register_extract_enums!($vis, $ty { $($rest)* });
    };
    ($vis:vis, $ty:ty { $(#[$fmeta:meta])* $field:ident : $lo:tt ..= $hi:tt as bool }) => {};
    ($vis:vis, $ty:ty { $(#[$fmeta:meta])* $field:ident : $lo:tt .. $hi:tt as bool , $($rest:tt)* }) => {
        $crate::__register_extract_enums!($vis, $ty { $($rest)* });
    };
    ($vis:vis, $ty:ty { $(#[$fmeta:meta])* $field:ident : $lo:tt .. $hi:tt as bool }) => {};
    ($vis:vis, $ty:ty { $(#[$fmeta:meta])* $field:ident : $bit:tt as bool , $($rest:tt)* }) => {
        $crate::__register_extract_enums!($vis, $ty { $($rest)* });
    };
    ($vis:vis, $ty:ty { $(#[$fmeta:meta])* $field:ident : $bit:tt as bool }) => {};

    // ── as $cast_ty — skip ──────────────────────────────────────────────

    ($vis:vis, $ty:ty { $(#[$fmeta:meta])* $field:ident : $lo:tt ..= $hi:tt as $cast_ty:ty , $($rest:tt)* }) => {
        $crate::__register_extract_enums!($vis, $ty { $($rest)* });
    };
    ($vis:vis, $ty:ty { $(#[$fmeta:meta])* $field:ident : $lo:tt ..= $hi:tt as $cast_ty:ty }) => {};
    ($vis:vis, $ty:ty { $(#[$fmeta:meta])* $field:ident : $lo:tt .. $hi:tt as $cast_ty:ty , $($rest:tt)* }) => {
        $crate::__register_extract_enums!($vis, $ty { $($rest)* });
    };
    ($vis:vis, $ty:ty { $(#[$fmeta:meta])* $field:ident : $lo:tt .. $hi:tt as $cast_ty:ty }) => {};
    ($vis:vis, $ty:ty { $(#[$fmeta:meta])* $field:ident : $bit:tt as $cast_ty:ty , $($rest:tt)* }) => {
        $crate::__register_extract_enums!($vis, $ty { $($rest)* });
    };
    ($vis:vis, $ty:ty { $(#[$fmeta:meta])* $field:ident : $bit:tt as $cast_ty:ty }) => {};

    // ── untyped (raw) — skip ────────────────────────────────────────────

    ($vis:vis, $ty:ty { $(#[$fmeta:meta])* $field:ident : $lo:tt ..= $hi:tt , $($rest:tt)* }) => {
        $crate::__register_extract_enums!($vis, $ty { $($rest)* });
    };
    ($vis:vis, $ty:ty { $(#[$fmeta:meta])* $field:ident : $lo:tt ..= $hi:tt }) => {};
    ($vis:vis, $ty:ty { $(#[$fmeta:meta])* $field:ident : $lo:tt .. $hi:tt , $($rest:tt)* }) => {
        $crate::__register_extract_enums!($vis, $ty { $($rest)* });
    };
    ($vis:vis, $ty:ty { $(#[$fmeta:meta])* $field:ident : $lo:tt .. $hi:tt }) => {};
    ($vis:vis, $ty:ty { $(#[$fmeta:meta])* $field:ident : $bit:tt , $($rest:tt)* }) => {
        $crate::__register_extract_enums!($vis, $ty { $($rest)* });
    };
    ($vis:vis, $ty:ty { $(#[$fmeta:meta])* $field:ident : $bit:tt }) => {};
}

/// Internal macro: bounds-check each register offset inside `new()`.
#[cfg(feature = "register-map")]
#[macro_export]
#[doc(hidden)]
macro_rules! __register_map_check {
    // Entry with bitfields — skip bitfield block, continue
    (($bus: ty) $dv:ident $offset:expr => $(#[$meta:meta])* $kind:ident $name:ident : $ty:ty { $($fields:tt)* } , $($rest:tt)+) => {
        $crate::__register_map_check!(@one ($bus) $dv $offset ; $ty);
        $crate::__register_map_check!(($bus) $dv $($rest)+);
    };
    (($bus: ty) $dv:ident $offset:expr => $(#[$meta:meta])* $kind:ident $name:ident : $ty:ty { $($fields:tt)* }) => {
        $crate::__register_map_check!(@one ($bus) $dv $offset ; $ty);
    };
    // Entry without bitfields
    (($bus: ty) $dv:ident $offset:expr => $(#[$meta:meta])* $kind:ident $name:ident : $ty:ty , $($rest:tt)+) => {
        $crate::__register_map_check!(@one ($bus) $dv $offset ; $ty);
        $crate::__register_map_check!(($bus) $dv $($rest)+);
    };
    (($bus: ty) $dv:ident $offset:expr => $(#[$meta:meta])* $kind:ident $name:ident : $ty:ty) => {
        $crate::__register_map_check!(@one ($bus) $dv $offset ; $ty);
    };
    (@one ($bus: ty) $dv:ident $offset:expr ; $ty:ty) => {
        const _: () = assert!(
            std::mem::size_of::<$ty>() <= std::mem::size_of::<$bus>(),
            "register type must not be wider than bus type"
        );
        const _: () = assert!(
            ($offset) % std::mem::align_of::<$bus>() == 0,
            "register offset must be aligned to bus width"
        );
        if ($offset) + std::mem::size_of::<$bus>() > $dv.len() {
            return None;
        }
    };
}

/// Internal macro: TT-muncher that dispatches each register entry.
#[cfg(feature = "register-map")]
#[macro_export]
#[doc(hidden)]
macro_rules! __register_map_methods {
    // Entry with bitfields, more entries follow
    ($vis:vis ($bus:ty) $offset:expr => $(#[$meta:meta])* $kind:ident $name:ident : $ty:ty { $($fields:tt)* } , $($rest:tt)+) => {
        $crate::__register_map_entry!($vis ($bus) [$(#[$meta])*] $offset => $kind $name : $ty { $($fields)* });
        $crate::__register_map_methods!($vis ($bus) $($rest)+);
    };
    // Entry with bitfields, last entry
    ($vis:vis ($bus:ty) $offset:expr => $(#[$meta:meta])* $kind:ident $name:ident : $ty:ty { $($fields:tt)* }) => {
        $crate::__register_map_entry!($vis ($bus) [$(#[$meta])*] $offset => $kind $name : $ty { $($fields)* });
    };
    // Entry without bitfields, more entries follow
    ($vis:vis ($bus:ty) $offset:expr => $(#[$meta:meta])* $kind:ident $name:ident : $ty:ty , $($rest:tt)+) => {
        $crate::__register_map_entry!($vis ($bus) [$(#[$meta])*] $offset => $kind $name : $ty {});
        $crate::__register_map_methods!($vis ($bus) $($rest)+);
    };
    // Entry without bitfields, last entry
    ($vis:vis ($bus:ty) $offset:expr => $(#[$meta:meta])* $kind:ident $name:ident : $ty:ty) => {
        $crate::__register_map_entry!($vis ($bus) [$(#[$meta])*] $offset => $kind $name : $ty {});
    };
}

/// Internal macro: generate methods for a single register entry.
#[cfg(feature = "register-map")]
#[macro_export]
#[doc(hidden)]
macro_rules! __register_map_entry {
    ($vis:vis ($bus:ty) [$(#[$meta:meta])*] $offset:expr => rw $name:ident : $ty:ty { $($fields:tt)* }) => {
        $crate::__register_methods!($vis reg base $offset => $name : $ty);
        $crate::__register_methods!($vis reg($bus) read [$(#[$meta])*] $offset => $name : $ty);
        $crate::__register_methods!($vis reg($bus) write [$(#[$meta])*] $offset => $name : $ty);
        $crate::__register_methods!($vis reg($bus) modify [$(#[$meta])*] $offset => $name : $ty);
        $crate::__register_bitfields!($vis ($bus) $offset => rw $name : $ty { $($fields)* });
    };
    ($vis:vis ($bus:ty) [$(#[$meta:meta])*] $offset:expr => ro $name:ident : $ty:ty { $($fields:tt)* }) => {
        $crate::__register_methods!($vis reg base $offset => $name : $ty);
        $crate::__register_methods!($vis reg($bus) read [$(#[$meta])*] $offset => $name : $ty);
        $crate::__register_bitfields!($vis ($bus) $offset => ro $name : $ty { $($fields)* });
    };
    ($vis:vis ($bus:ty) [$(#[$meta:meta])*] $offset:expr => wo $name:ident : $ty:ty { $($fields:tt)* }) => {
        $crate::__register_methods!($vis reg base $offset => $name : $ty);
        $crate::__register_methods!($vis reg($bus) write [$(#[$meta])*] $offset => $name : $ty);
        $crate::__register_bitfields!($vis ($bus) $offset => wo $name : $ty { $($fields)* });
    };
}

/// Internal macro: TT-muncher that parses bitfield declarations.
#[cfg(feature = "register-map")]
#[macro_export]
#[doc(hidden)]
macro_rules! __register_bitfields {
    // Empty — no bitfields
    ($vis:vis ($bus:ty) $offset:expr => $kind:ident $reg:ident : $ty:ty {}) => {};

    // ── as bool ──────────────────────────────────────────────────────────

    ($vis:vis ($bus:ty) $offset:expr => $kind:ident $reg:ident : $ty:ty {
        $(#[$fmeta:meta])* $field:ident : $lo:tt ..= $hi:tt as bool , $($rest:tt)*
    }) => {
        $crate::__register_one_bitfield!($vis ($bus) [$(#[$fmeta])*] $offset => $kind $reg : $ty, $field, $lo, $hi, @bool);
        $crate::__register_bitfields!($vis ($bus) $offset => $kind $reg : $ty { $($rest)* });
    };
    ($vis:vis ($bus:ty) $offset:expr => $kind:ident $reg:ident : $ty:ty {
        $(#[$fmeta:meta])* $field:ident : $lo:tt ..= $hi:tt as bool
    }) => {
        $crate::__register_one_bitfield!($vis ($bus) [$(#[$fmeta])*] $offset => $kind $reg : $ty, $field, $lo, $hi, @bool);
    };
    ($vis:vis ($bus:ty) $offset:expr => $kind:ident $reg:ident : $ty:ty {
        $(#[$fmeta:meta])* $field:ident : $lo:tt .. $hi:tt as bool , $($rest:tt)*
    }) => {
        $crate::__register_one_bitfield!($vis ($bus) [$(#[$fmeta])*] $offset => $kind $reg : $ty, $field, $lo, ($hi - 1), @bool);
        $crate::__register_bitfields!($vis ($bus) $offset => $kind $reg : $ty { $($rest)* });
    };
    ($vis:vis ($bus:ty) $offset:expr => $kind:ident $reg:ident : $ty:ty {
        $(#[$fmeta:meta])* $field:ident : $lo:tt .. $hi:tt as bool
    }) => {
        $crate::__register_one_bitfield!($vis ($bus) [$(#[$fmeta])*] $offset => $kind $reg : $ty, $field, $lo, ($hi - 1), @bool);
    };
    ($vis:vis ($bus:ty) $offset:expr => $kind:ident $reg:ident : $ty:ty {
        $(#[$fmeta:meta])* $field:ident : $bit:tt as bool , $($rest:tt)*
    }) => {
        $crate::__register_one_bitfield!($vis ($bus) [$(#[$fmeta])*] $offset => $kind $reg : $ty, $field, $bit, $bit, @bool);
        $crate::__register_bitfields!($vis ($bus) $offset => $kind $reg : $ty { $($rest)* });
    };
    ($vis:vis ($bus:ty) $offset:expr => $kind:ident $reg:ident : $ty:ty {
        $(#[$fmeta:meta])* $field:ident : $bit:tt as bool
    }) => {
        $crate::__register_one_bitfield!($vis ($bus) [$(#[$fmeta])*] $offset => $kind $reg : $ty, $field, $bit, $bit, @bool);
    };

    // ── as enum ──────────────────────────────────────────────────────────

    ($vis:vis ($bus:ty) $offset:expr => $kind:ident $reg:ident : $ty:ty {
        $(#[$fmeta:meta])* $field:ident : $lo:tt ..= $hi:tt as enum $ename:ident { $($ebody:tt)* } , $($rest:tt)*
    }) => {
        $crate::__register_one_bitfield!($vis ($bus) [$(#[$fmeta])*] $offset => $kind $reg : $ty, $field, $lo, $hi, @enum $ename);
        $crate::__register_bitfields!($vis ($bus) $offset => $kind $reg : $ty { $($rest)* });
    };
    ($vis:vis ($bus:ty) $offset:expr => $kind:ident $reg:ident : $ty:ty {
        $(#[$fmeta:meta])* $field:ident : $lo:tt ..= $hi:tt as enum $ename:ident { $($ebody:tt)* }
    }) => {
        $crate::__register_one_bitfield!($vis ($bus) [$(#[$fmeta])*] $offset => $kind $reg : $ty, $field, $lo, $hi, @enum $ename);
    };
    ($vis:vis ($bus:ty) $offset:expr => $kind:ident $reg:ident : $ty:ty {
        $(#[$fmeta:meta])* $field:ident : $lo:tt .. $hi:tt as enum $ename:ident { $($ebody:tt)* } , $($rest:tt)*
    }) => {
        $crate::__register_one_bitfield!($vis ($bus) [$(#[$fmeta])*] $offset => $kind $reg : $ty, $field, $lo, ($hi - 1), @enum $ename);
        $crate::__register_bitfields!($vis ($bus) $offset => $kind $reg : $ty { $($rest)* });
    };
    ($vis:vis ($bus:ty) $offset:expr => $kind:ident $reg:ident : $ty:ty {
        $(#[$fmeta:meta])* $field:ident : $lo:tt .. $hi:tt as enum $ename:ident { $($ebody:tt)* }
    }) => {
        $crate::__register_one_bitfield!($vis ($bus) [$(#[$fmeta])*] $offset => $kind $reg : $ty, $field, $lo, ($hi - 1), @enum $ename);
    };
    ($vis:vis ($bus:ty) $offset:expr => $kind:ident $reg:ident : $ty:ty {
        $(#[$fmeta:meta])* $field:ident : $bit:tt as enum $ename:ident { $($ebody:tt)* } , $($rest:tt)*
    }) => {
        $crate::__register_one_bitfield!($vis ($bus) [$(#[$fmeta])*] $offset => $kind $reg : $ty, $field, $bit, $bit, @enum $ename);
        $crate::__register_bitfields!($vis ($bus) $offset => $kind $reg : $ty { $($rest)* });
    };
    ($vis:vis ($bus:ty) $offset:expr => $kind:ident $reg:ident : $ty:ty {
        $(#[$fmeta:meta])* $field:ident : $bit:tt as enum $ename:ident { $($ebody:tt)* }
    }) => {
        $crate::__register_one_bitfield!($vis ($bus) [$(#[$fmeta])*] $offset => $kind $reg : $ty, $field, $bit, $bit, @enum $ename);
    };

    // ── as cast type ─────────────────────────────────────────────────────

    ($vis:vis ($bus:ty) $offset:expr => $kind:ident $reg:ident : $ty:ty {
        $(#[$fmeta:meta])* $field:ident : $lo:tt ..= $hi:tt as $cast_ty:ty , $($rest:tt)*
    }) => {
        $crate::__register_one_bitfield!($vis ($bus) [$(#[$fmeta])*] $offset => $kind $reg : $ty, $field, $lo, $hi, @cast $cast_ty);
        $crate::__register_bitfields!($vis ($bus) $offset => $kind $reg : $ty { $($rest)* });
    };
    ($vis:vis ($bus:ty) $offset:expr => $kind:ident $reg:ident : $ty:ty {
        $(#[$fmeta:meta])* $field:ident : $lo:tt ..= $hi:tt as $cast_ty:ty
    }) => {
        $crate::__register_one_bitfield!($vis ($bus) [$(#[$fmeta])*] $offset => $kind $reg : $ty, $field, $lo, $hi, @cast $cast_ty);
    };
    ($vis:vis ($bus:ty) $offset:expr => $kind:ident $reg:ident : $ty:ty {
        $(#[$fmeta:meta])* $field:ident : $lo:tt .. $hi:tt as $cast_ty:ty , $($rest:tt)*
    }) => {
        $crate::__register_one_bitfield!($vis ($bus) [$(#[$fmeta])*] $offset => $kind $reg : $ty, $field, $lo, ($hi - 1), @cast $cast_ty);
        $crate::__register_bitfields!($vis ($bus) $offset => $kind $reg : $ty { $($rest)* });
    };
    ($vis:vis ($bus:ty) $offset:expr => $kind:ident $reg:ident : $ty:ty {
        $(#[$fmeta:meta])* $field:ident : $lo:tt .. $hi:tt as $cast_ty:ty
    }) => {
        $crate::__register_one_bitfield!($vis ($bus) [$(#[$fmeta])*] $offset => $kind $reg : $ty, $field, $lo, ($hi - 1), @cast $cast_ty);
    };
    ($vis:vis ($bus:ty) $offset:expr => $kind:ident $reg:ident : $ty:ty {
        $(#[$fmeta:meta])* $field:ident : $bit:tt as $cast_ty:ty , $($rest:tt)*
    }) => {
        $crate::__register_one_bitfield!($vis ($bus) [$(#[$fmeta])*] $offset => $kind $reg : $ty, $field, $bit, $bit, @cast $cast_ty);
        $crate::__register_bitfields!($vis ($bus) $offset => $kind $reg : $ty { $($rest)* });
    };
    ($vis:vis ($bus:ty) $offset:expr => $kind:ident $reg:ident : $ty:ty {
        $(#[$fmeta:meta])* $field:ident : $bit:tt as $cast_ty:ty
    }) => {
        $crate::__register_one_bitfield!($vis ($bus) [$(#[$fmeta])*] $offset => $kind $reg : $ty, $field, $bit, $bit, @cast $cast_ty);
    };

    // ── untyped (raw) ────────────────────────────────────────────────────

    // Multi-bit field (lo..=hi), more follow
    ($vis:vis ($bus:ty) $offset:expr => $kind:ident $reg:ident : $ty:ty {
        $(#[$fmeta:meta])* $field:ident : $lo:tt ..= $hi:tt , $($rest:tt)*
    }) => {
        $crate::__register_one_bitfield!($vis ($bus) [$(#[$fmeta])*] $offset => $kind $reg : $ty, $field, $lo, $hi);
        $crate::__register_bitfields!($vis ($bus) $offset => $kind $reg : $ty { $($rest)* });
    };
    // Multi-bit field (lo..=hi), last
    ($vis:vis ($bus:ty) $offset:expr => $kind:ident $reg:ident : $ty:ty {
        $(#[$fmeta:meta])* $field:ident : $lo:tt ..= $hi:tt
    }) => {
        $crate::__register_one_bitfield!($vis ($bus) [$(#[$fmeta])*] $offset => $kind $reg : $ty, $field, $lo, $hi);
    };

    // Multi-bit field (lo..hi exclusive), more follow
    ($vis:vis ($bus:ty) $offset:expr => $kind:ident $reg:ident : $ty:ty {
        $(#[$fmeta:meta])* $field:ident : $lo:tt .. $hi:tt , $($rest:tt)*
    }) => {
        $crate::__register_one_bitfield!($vis ($bus) [$(#[$fmeta])*] $offset => $kind $reg : $ty, $field, $lo, ($hi - 1));
        $crate::__register_bitfields!($vis ($bus) $offset => $kind $reg : $ty { $($rest)* });
    };
    // Multi-bit field (lo..hi exclusive), last
    ($vis:vis ($bus:ty) $offset:expr => $kind:ident $reg:ident : $ty:ty {
        $(#[$fmeta:meta])* $field:ident : $lo:tt .. $hi:tt
    }) => {
        $crate::__register_one_bitfield!($vis ($bus) [$(#[$fmeta])*] $offset => $kind $reg : $ty, $field, $lo, ($hi - 1));
    };

    // Single-bit field, more follow
    ($vis:vis ($bus:ty) $offset:expr => $kind:ident $reg:ident : $ty:ty {
        $(#[$fmeta:meta])* $field:ident : $bit:tt , $($rest:tt)*
    }) => {
        $crate::__register_one_bitfield!($vis ($bus) [$(#[$fmeta])*] $offset => $kind $reg : $ty, $field, $bit, $bit);
        $crate::__register_bitfields!($vis ($bus) $offset => $kind $reg : $ty { $($rest)* });
    };
    // Single-bit field, last
    ($vis:vis ($bus:ty) $offset:expr => $kind:ident $reg:ident : $ty:ty {
        $(#[$fmeta:meta])* $field:ident : $bit:tt
    }) => {
        $crate::__register_one_bitfield!($vis ($bus) [$(#[$fmeta])*] $offset => $kind $reg : $ty, $field, $bit, $bit);
    };
}

/// Internal macro: generate getter / setter for a single bitfield.
#[cfg(feature = "register-map")]
#[macro_export]
#[doc(hidden)]
macro_rules! __register_one_bitfield {
    // ── untyped (raw) dispatch ───────────────────────────────────────────

    ($vis:vis ($bus:ty) [$(#[$fmeta:meta])*] $offset:expr => rw $reg:ident : $ty:ty, $field:ident, $lo:expr, $hi:expr) => {
        $crate::__register_one_bitfield!(@getter $vis ($bus) [$(#[$fmeta])*] $offset => $reg : $ty, $field, $lo, $hi);
        $crate::__register_one_bitfield!(@setter $vis ($bus) [$(#[$fmeta])*] $offset => $reg : $ty, $field, $lo, $hi);
    };
    ($vis:vis ($bus:ty) [$(#[$fmeta:meta])*] $offset:expr => ro $reg:ident : $ty:ty, $field:ident, $lo:expr, $hi:expr) => {
        $crate::__register_one_bitfield!(@getter $vis ($bus) [$(#[$fmeta])*] $offset => $reg : $ty, $field, $lo, $hi);
    };
    ($vis:vis ($bus:ty) [$(#[$fmeta:meta])*] $offset:expr => wo $reg:ident : $ty:ty, $field:ident, $lo:expr, $hi:expr) => {
        $crate::__register_one_bitfield!(@setter $vis ($bus) [$(#[$fmeta])*] $offset => $reg : $ty, $field, $lo, $hi);
    };

    // ── bool dispatch ────────────────────────────────────────────────────

    ($vis:vis ($bus:ty) [$(#[$fmeta:meta])*] $offset:expr => rw $reg:ident : $ty:ty, $field:ident, $lo:expr, $hi:expr, @bool) => {
        $crate::__register_one_bitfield!(@bool_getter $vis ($bus) [$(#[$fmeta])*] $offset => $reg : $ty, $field, $lo, $hi);
        $crate::__register_one_bitfield!(@bool_setter $vis ($bus) [$(#[$fmeta])*] $offset => $reg : $ty, $field, $lo, $hi);
    };
    ($vis:vis ($bus:ty) [$(#[$fmeta:meta])*] $offset:expr => ro $reg:ident : $ty:ty, $field:ident, $lo:expr, $hi:expr, @bool) => {
        $crate::__register_one_bitfield!(@bool_getter $vis ($bus) [$(#[$fmeta])*] $offset => $reg : $ty, $field, $lo, $hi);
    };
    ($vis:vis ($bus:ty) [$(#[$fmeta:meta])*] $offset:expr => wo $reg:ident : $ty:ty, $field:ident, $lo:expr, $hi:expr, @bool) => {
        $crate::__register_one_bitfield!(@bool_setter $vis ($bus) [$(#[$fmeta])*] $offset => $reg : $ty, $field, $lo, $hi);
    };

    // ── cast dispatch ────────────────────────────────────────────────────

    ($vis:vis ($bus:ty) [$(#[$fmeta:meta])*] $offset:expr => rw $reg:ident : $ty:ty, $field:ident, $lo:expr, $hi:expr, @cast $cast_ty:ty) => {
        $crate::__register_one_bitfield!(@cast_getter $vis ($bus) [$(#[$fmeta])*] $offset => $reg : $ty, $field, $lo, $hi, $cast_ty);
        $crate::__register_one_bitfield!(@cast_setter $vis ($bus) [$(#[$fmeta])*] $offset => $reg : $ty, $field, $lo, $hi, $cast_ty);
    };
    ($vis:vis ($bus:ty) [$(#[$fmeta:meta])*] $offset:expr => ro $reg:ident : $ty:ty, $field:ident, $lo:expr, $hi:expr, @cast $cast_ty:ty) => {
        $crate::__register_one_bitfield!(@cast_getter $vis ($bus) [$(#[$fmeta])*] $offset => $reg : $ty, $field, $lo, $hi, $cast_ty);
    };
    ($vis:vis ($bus:ty) [$(#[$fmeta:meta])*] $offset:expr => wo $reg:ident : $ty:ty, $field:ident, $lo:expr, $hi:expr, @cast $cast_ty:ty) => {
        $crate::__register_one_bitfield!(@cast_setter $vis ($bus) [$(#[$fmeta])*] $offset => $reg : $ty, $field, $lo, $hi, $cast_ty);
    };

    // ── enum dispatch ────────────────────────────────────────────────────

    ($vis:vis ($bus:ty) [$(#[$fmeta:meta])*] $offset:expr => rw $reg:ident : $ty:ty, $field:ident, $lo:expr, $hi:expr, @enum $ename:ident) => {
        $crate::__register_one_bitfield!(@enum_getter $vis ($bus) [$(#[$fmeta])*] $offset => $reg : $ty, $field, $lo, $hi, $ename);
        $crate::__register_one_bitfield!(@enum_setter $vis ($bus) [$(#[$fmeta])*] $offset => $reg : $ty, $field, $lo, $hi, $ename);
    };
    ($vis:vis ($bus:ty) [$(#[$fmeta:meta])*] $offset:expr => ro $reg:ident : $ty:ty, $field:ident, $lo:expr, $hi:expr, @enum $ename:ident) => {
        $crate::__register_one_bitfield!(@enum_getter $vis ($bus) [$(#[$fmeta])*] $offset => $reg : $ty, $field, $lo, $hi, $ename);
    };
    ($vis:vis ($bus:ty) [$(#[$fmeta:meta])*] $offset:expr => wo $reg:ident : $ty:ty, $field:ident, $lo:expr, $hi:expr, @enum $ename:ident) => {
        $crate::__register_one_bitfield!(@enum_setter $vis ($bus) [$(#[$fmeta])*] $offset => $reg : $ty, $field, $lo, $hi, $ename);
    };

    // ── raw getter / setter ──────────────────────────────────────────────

    (@getter $vis:vis ($bus:ty) [$(#[$fmeta:meta])*] $offset:expr => $reg:ident : $ty:ty, $field:ident, $lo:expr, $hi:expr) => {
        $crate::__concat_idents!(fn_name = $reg, _, $field, {
            $(#[$fmeta])*
            #[inline(always)]
            $vis fn fn_name(&self) -> $ty {
                let raw = unsafe { std::ptr::read_volatile(self.devmem.as_ptr().add($offset) as *const $bus) } as $ty;
                let width: u32 = ($hi) - ($lo) + 1;
                let mask: $ty = if width >= <$ty>::BITS { <$ty>::MAX } else { (1 << width) - 1 };
                (raw >> ($lo)) & mask
            }
        });
    };

    (@setter $vis:vis ($bus:ty) [$(#[$fmeta:meta])*] $offset:expr => $reg:ident : $ty:ty, $field:ident, $lo:expr, $hi:expr) => {
        $crate::__concat_idents!(fn_name = set_, $reg, _, $field, {
            $(#[$fmeta])*
            #[inline(always)]
            $vis fn fn_name(&mut self, value: $ty) {
                let width: u32 = ($hi) - ($lo) + 1;
                let mask: $ty = if width >= <$ty>::BITS { <$ty>::MAX } else { (1 << width) - 1 };
                unsafe {
                    let ptr = self.devmem.as_ptr().add($offset);
                    let old = std::ptr::read_volatile(ptr as *const $bus) as $ty;
                    let new = (old & !(mask << ($lo))) | ((value & mask) << ($lo));
                    std::ptr::write_volatile(ptr as *mut $bus, new as $bus);
                }
            }
        });
    };

    // ── bool getter / setter ─────────────────────────────────────────────

    (@bool_getter $vis:vis ($bus:ty) [$(#[$fmeta:meta])*] $offset:expr => $reg:ident : $ty:ty, $field:ident, $lo:expr, $hi:expr) => {
        $crate::__concat_idents!(fn_name = $reg, _, $field, {
            $(#[$fmeta])*
            #[inline(always)]
            $vis fn fn_name(&self) -> bool {
                let raw = unsafe { std::ptr::read_volatile(self.devmem.as_ptr().add($offset) as *const $bus) } as $ty;
                let width: u32 = ($hi) - ($lo) + 1;
                let mask: $ty = if width >= <$ty>::BITS { <$ty>::MAX } else { (1 << width) - 1 };
                ((raw >> ($lo)) & mask) != 0
            }
        });
    };

    (@bool_setter $vis:vis ($bus:ty) [$(#[$fmeta:meta])*] $offset:expr => $reg:ident : $ty:ty, $field:ident, $lo:expr, $hi:expr) => {
        $crate::__concat_idents!(fn_name = set_, $reg, _, $field, {
            $(#[$fmeta])*
            #[inline(always)]
            $vis fn fn_name(&mut self, value: bool) {
                let value = value as $ty;
                let width: u32 = ($hi) - ($lo) + 1;
                let mask: $ty = if width >= <$ty>::BITS { <$ty>::MAX } else { (1 << width) - 1 };
                unsafe {
                    let ptr = self.devmem.as_ptr().add($offset);
                    let old = std::ptr::read_volatile(ptr as *const $bus) as $ty;
                    let new = (old & !(mask << ($lo))) | ((value & mask) << ($lo));
                    std::ptr::write_volatile(ptr as *mut $bus, new as $bus);
                }
            }
        });
    };

    // ── cast getter / setter ─────────────────────────────────────────────

    (@cast_getter $vis:vis ($bus:ty) [$(#[$fmeta:meta])*] $offset:expr => $reg:ident : $ty:ty, $field:ident, $lo:expr, $hi:expr, $cast_ty:ty) => {
        $crate::__concat_idents!(fn_name = $reg, _, $field, {
            $(#[$fmeta])*
            #[inline(always)]
            $vis fn fn_name(&self) -> $cast_ty {
                let raw = unsafe { std::ptr::read_volatile(self.devmem.as_ptr().add($offset) as *const $bus) } as $ty;
                let width: u32 = ($hi) - ($lo) + 1;
                let mask: $ty = if width >= <$ty>::BITS { <$ty>::MAX } else { (1 << width) - 1 };
                ((raw >> ($lo)) & mask) as $cast_ty
            }
        });
    };

    (@cast_setter $vis:vis ($bus:ty) [$(#[$fmeta:meta])*] $offset:expr => $reg:ident : $ty:ty, $field:ident, $lo:expr, $hi:expr, $cast_ty:ty) => {
        $crate::__concat_idents!(fn_name = set_, $reg, _, $field, {
            $(#[$fmeta])*
            #[inline(always)]
            $vis fn fn_name(&mut self, value: $cast_ty) {
                let value = value as $ty;
                let width: u32 = ($hi) - ($lo) + 1;
                let mask: $ty = if width >= <$ty>::BITS { <$ty>::MAX } else { (1 << width) - 1 };
                unsafe {
                    let ptr = self.devmem.as_ptr().add($offset);
                    let old = std::ptr::read_volatile(ptr as *const $bus) as $ty;
                    let new = (old & !(mask << ($lo))) | ((value & mask) << ($lo));
                    std::ptr::write_volatile(ptr as *mut $bus, new as $bus);
                }
            }
        });
    };

    // ── enum getter / setter ─────────────────────────────────────────────

    (@enum_getter $vis:vis ($bus:ty) [$(#[$fmeta:meta])*] $offset:expr => $reg:ident : $ty:ty, $field:ident, $lo:expr, $hi:expr, $ename:ident) => {
        $crate::__concat_idents!(fn_name = $reg, _, $field, {
            $(#[$fmeta])*
            #[inline(always)]
            $vis fn fn_name(&self) -> $ename {
                let raw = unsafe { std::ptr::read_volatile(self.devmem.as_ptr().add($offset) as *const $bus) } as $ty;
                let width: u32 = ($hi) - ($lo) + 1;
                let mask: $ty = if width >= <$ty>::BITS { <$ty>::MAX } else { (1 << width) - 1 };
                $ename::from_raw((raw >> ($lo)) & mask)
            }
        });
    };

    (@enum_setter $vis:vis ($bus:ty) [$(#[$fmeta:meta])*] $offset:expr => $reg:ident : $ty:ty, $field:ident, $lo:expr, $hi:expr, $ename:ident) => {
        $crate::__concat_idents!(fn_name = set_, $reg, _, $field, {
            $(#[$fmeta])*
            #[inline(always)]
            $vis fn fn_name(&mut self, value: $ename) {
                let value = value.to_raw();
                let width: u32 = ($hi) - ($lo) + 1;
                let mask: $ty = if width >= <$ty>::BITS { <$ty>::MAX } else { (1 << width) - 1 };
                unsafe {
                    let ptr = self.devmem.as_ptr().add($offset);
                    let old = std::ptr::read_volatile(ptr as *const $bus) as $ty;
                    let new = (old & !(mask << ($lo))) | ((value & mask) << ($lo));
                    std::ptr::write_volatile(ptr as *mut $bus, new as $bus);
                }
            }
        });
    };
}

/// Internal macro: generate an enum from bitfield variant definitions.
///
/// Produces a `#[derive(Debug, Clone, Copy, PartialEq, Eq)]` enum with
/// `from_raw()` / `to_raw()` conversion methods.
#[cfg(feature = "register-map")]
#[macro_export]
#[doc(hidden)]
macro_rules! __register_enum {
    ($vis:vis $ename:ident : $ty:ty { $($variant:ident = $val:expr),* $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        $vis enum $ename {
            $($variant,)*
        }

        impl $ename {
            /// Convert from a raw register value.
            ///
            /// Unknown values map to the first declared variant.
            #[inline]
            #[allow(unreachable_patterns)]
            pub fn from_raw(v: $ty) -> Self {
                match v {
                    $($val => Self::$variant,)*
                    _ => {
                        let _variants = [$(Self::$variant,)*];
                        _variants[0]
                    }
                }
            }

            /// Convert to raw register value.
            #[inline]
            pub fn to_raw(self) -> $ty {
                match self {
                    $(Self::$variant => $val as $ty,)*
                }
            }
        }

        impl std::fmt::Display for $ename {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                std::fmt::Debug::fmt(self, f)
            }
        }
    };
}

#[cfg(feature = "register-map")]
#[macro_export(local_inner_macros)]
#[doc(hidden)]
macro_rules! __register_methods {
    ($vis: vis reg base $offset: expr => $name: ident : $ty: ty) => {
        $crate::__concat_idents!(fn_name = $name, _offset, {
            /// Returns the offset of the register within the DevMem.
            #[inline(always)]
            $vis fn fn_name(&self) -> usize {
                $offset
            }
        });

        $crate::__concat_idents!(fn_name = $name, _address, {
            /// Returns the address of the register.
            #[inline(always)]
            $vis fn fn_name(&self) -> usize {
                self.devmem.address() + $offset
            }
        });
    };
    ($vis: vis reg($bus: ty) read [$(#[$meta:meta])*] $offset: expr => $name: ident : $ty: ty) => {
        $(#[$meta])*
        #[inline(always)]
        $vis fn $name(&self) -> $ty {
            unsafe { std::ptr::read_volatile(self.devmem.as_ptr().add($offset) as *const $bus) as $ty }
        }
    };
    ($vis: vis reg($bus: ty) write [$(#[$meta:meta])*] $offset: expr => $name: ident : $ty: ty) => {
        $crate::__concat_idents!(fn_name = set_, $name, {
            $(#[$meta])*
            #[inline(always)]
            $vis fn fn_name(&mut self, value: $ty) {
                unsafe { std::ptr::write_volatile(self.devmem.as_ptr().add($offset) as *mut $bus, value as $bus) }
            }
        });
    };
    ($vis: vis reg($bus: ty) modify [$(#[$meta:meta])*] $offset: expr => $name: ident : $ty: ty) => {
        $crate::__concat_idents!(fn_name = modify_, $name, {
            $(#[$meta])*
            #[inline(always)]
            $vis fn fn_name(&mut self, f: impl FnOnce($ty) -> $ty) {
                unsafe {
                    let ptr = self.devmem.as_ptr().add($offset);
                    let val = std::ptr::read_volatile(ptr as *const $bus) as $ty;
                    std::ptr::write_volatile(ptr as *mut $bus, f(val) as $bus);
                }
            }
        });
    }
}
