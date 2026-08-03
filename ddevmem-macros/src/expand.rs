//! Code generation.
//!
//! The generated code is meant to read like hand-written MMIO accessors:
//!
//! - a single set of private helpers (`__read`, `__write`, `__update`,
//!   `__index`) owns all `unsafe` volatile access, and every public method
//!   is a one-line call into them;
//! - bitfield masks and shifts are computed at macro-expansion time and
//!   emitted as plain literals (`(self.__read(0x0) >> 2) & 0xF`) whenever
//!   the bit positions are literals; otherwise they fall back to private
//!   associated consts validated by `const` assertions;
//! - checks that depend on the target (`usize` widths, non-literal offsets)
//!   are emitted as `const` assertions next to the struct.

use proc_macro2::{Span, TokenStream};
use quote::{format_ident, quote, ToTokens};
use syn::{Attribute, Ident, LitInt};

use crate::ast::{
    Access, Bitfield, ConstExpr, EnumDef, FieldAccess, FieldType, RegisterEntry, RegisterMap,
};

pub fn expand(map: &RegisterMap) -> TokenStream {
    let enums = expand_enums(map);
    let strukt = expand_struct(map);
    let asserts = expand_const_asserts(map);
    let core_impl = expand_impl(map);

    #[cfg(feature = "web")]
    let web = crate::web::expand(map);
    #[cfg(not(feature = "web"))]
    let web = TokenStream::new();

    quote! {
        #enums
        #strukt
        #asserts
        #core_impl
        #web
    }
}

// ─── Small token helpers ─────────────────────────────────────────────────────

/// Unsuffixed decimal literal.
fn dec(value: u64) -> LitInt {
    LitInt::new(&value.to_string(), Span::call_site())
}

/// Unsuffixed hex literal with `_` separators every four digits (`0xDEAD_BEEF`).
fn hex(value: u64) -> LitInt {
    let digits = format!("{value:X}");
    let mut grouped = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 4 == 0 {
            grouped.push('_');
        }
        grouped.push(c);
    }
    LitInt::new(&format!("0x{grouped}"), Span::call_site())
}

fn has_doc(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|a| a.path().is_ident("doc"))
}

/// Forwards the user's attributes and appends a generated summary line,
/// separated by a blank line when the user wrote docs of their own.
fn docs(user: &[Attribute], summary: &str) -> TokenStream {
    let blank = has_doc(user).then(|| quote!(#[doc = ""]));
    quote! {
        #(#user)*
        #blank
        #[doc = #summary]
    }
}

/// Human-readable offset for doc comments: `0x10` for literals, the source
/// expression otherwise.
fn offset_display(offset: &ConstExpr) -> String {
    match offset.value {
        Some(v) => format!("{v:#x}"),
        None => offset.expr.to_token_stream().to_string(),
    }
}

/// `cr` for scalar registers, `fifo[idx]` for arrays.
fn element_display(entry: &RegisterEntry) -> String {
    if entry.is_array() {
        format!("{}[idx]", entry.name)
    } else {
        entry.name.to_string()
    }
}

/// `bit 3` / `bits 5:2`, falling back to the source expressions.
fn bits_display(bf: &Bitfield) -> String {
    match (bf.lo.value, bf.hi.value) {
        (Some(lo), Some(hi)) if lo == hi => format!("bit {lo}"),
        (Some(lo), Some(hi)) => format!("bits {hi}:{lo}"),
        _ => format!(
            "bits {}:{}",
            bf.hi.expr.to_token_stream(),
            bf.lo.expr.to_token_stream()
        ),
    }
}

// ─── Bitfield positions ──────────────────────────────────────────────────────

/// A bitfield's resolved position within its register.
///
/// `Literal` carries values computed at expansion time and produces literal
/// masks in the output. `Const` (non-literal positions, or a `usize` register
/// type whose width is unknown here) defers the math to generated private
/// associated consts, validated by `const` assertions.
enum FieldPos {
    Literal {
        lo: u32,
        width: u32,
        /// The field covers the whole bus word, so masking is a no-op.
        full_bus: bool,
    },
    Const {
        lo: Ident,
        mask: Ident,
    },
}

impl FieldPos {
    fn mask_value(width: u32) -> u64 {
        if width >= 64 {
            u64::MAX
        } else {
            (1u64 << width) - 1
        }
    }

    /// Extracts the field from a bus-domain value: `(read >> lo) & mask`.
    fn extract(&self, read: TokenStream) -> TokenStream {
        match self {
            FieldPos::Literal { lo, width, full_bus } => {
                if *full_bus {
                    return read;
                }
                let mask = hex(Self::mask_value(*width));
                if *lo == 0 {
                    quote!(#read & #mask)
                } else {
                    let lo = dec(u64::from(*lo));
                    quote!((#read >> #lo) & #mask)
                }
            }
            FieldPos::Const { lo, mask } => {
                quote!((#read & Self::#mask) >> Self::#lo)
            }
        }
    }

    /// Tests a single-bit field: `read & 0x8 != 0`.
    fn test_bit(&self, read: TokenStream) -> TokenStream {
        match self {
            FieldPos::Literal { lo, .. } => {
                let mask = hex(1u64 << lo);
                quote!(#read & #mask != 0)
            }
            FieldPos::Const { mask, .. } => {
                quote!(#read & Self::#mask != 0)
            }
        }
    }

    /// Positions a bus-domain value into the field: `(value & mask) << lo`.
    ///
    /// `masked` is false when the value provably fits the field (a `bool`,
    /// or an enum whose variant values were all validated).
    fn insert(&self, value: TokenStream, masked: bool) -> TokenStream {
        match self {
            FieldPos::Literal { lo, width, full_bus } => {
                let core = if masked && !*full_bus {
                    let mask = hex(Self::mask_value(*width));
                    quote!((#value & #mask))
                } else {
                    value
                };
                if *lo == 0 {
                    core
                } else {
                    let lo = dec(u64::from(*lo));
                    quote!(#core << #lo)
                }
            }
            FieldPos::Const { lo, mask } => {
                quote!(((#value) << Self::#lo) & Self::#mask)
            }
        }
    }

    /// The mask shifted into position, for `__update`.
    fn positioned_mask(&self) -> Mask {
        match self {
            FieldPos::Literal { lo, width, .. } => {
                Mask::Lit(Self::mask_value(*width) << lo)
            }
            FieldPos::Const { mask, .. } => Mask::Expr(quote!(Self::#mask)),
        }
    }
}

/// A bit mask, kept as a plain value while every contributing field had
/// literal positions so that the generated code reads `0x107` rather than
/// `A | B | C`.
#[derive(Clone)]
enum Mask {
    Lit(u64),
    Expr(TokenStream),
}

impl Mask {
    fn is_empty(&self) -> bool {
        matches!(self, Mask::Lit(0))
    }

    fn or(self, other: Mask) -> Mask {
        match (self, other) {
            (Mask::Lit(a), Mask::Lit(b)) => Mask::Lit(a | b),
            (a, b) if a.is_empty() => b,
            (a, b) if b.is_empty() => a,
            (a, b) => {
                let (a, b) = (a.tokens(), b.tokens());
                Mask::Expr(quote!((#a | #b)))
            }
        }
    }

    fn tokens(&self) -> TokenStream {
        match self {
            Mask::Lit(value) => {
                let lit = hex(*value);
                quote!(#lit)
            }
            Mask::Expr(tokens) => tokens.clone(),
        }
    }
}

/// Resolves a bitfield's position, emitting const definitions into `consts`
/// when the fast literal path is not available.
fn field_pos(map: &RegisterMap, entry: &RegisterEntry, bf: &Bitfield, consts: &mut TokenStream) -> FieldPos {
    // The fast path needs literal positions plus a register type of known
    // width (`usize` registers take the const path so the range check can
    // happen against the real target width).
    if entry.ty.kind.width_bits().is_some() {
        if let (Some(lo), Some(hi)) = (bf.lo.value, bf.hi.value) {
            let width = (hi - lo + 1) as u32;
            return FieldPos::Literal {
                lo: lo as u32,
                width,
                full_bus: lo == 0 && map.bus.kind.width_bits() == Some(width),
            };
        }
    }

    let prefix = format!(
        "__{}_{}",
        entry.name.to_string().to_uppercase(),
        bf.name.to_string().to_uppercase()
    );
    let lo_ident = format_ident!("{prefix}_LO");
    let mask_ident = format_ident!("{prefix}_MASK");

    let bus = &map.bus;
    let ty = &entry.ty;
    let lo = &bf.lo;
    let hi = &bf.hi;
    let field = format!("{}::{}", entry.name, bf.name);
    let msg_range = format!("bitfield `{field}`: high bit is below low bit");
    let msg_width = format!("bitfield `{field}`: bit range exceeds the register type");
    let bool_assert = matches!(bf.ty, FieldType::Bool).then(|| {
        let msg = format!("bitfield `{field}`: `as bool` requires a single-bit field");
        quote!(assert!(hi == lo, #msg);)
    });

    consts.extend(quote! {
        const #lo_ident: u32 = #lo as u32;
        const #mask_ident: #bus = {
            let lo = #lo as u32;
            let hi = #hi as u32;
            assert!(hi >= lo, #msg_range);
            assert!((hi as usize) < ::core::mem::size_of::<#ty>() * 8, #msg_width);
            #bool_assert
            let width = hi - lo + 1;
            let unshifted = if width >= <#bus>::BITS {
                <#bus>::MAX
            } else {
                ((1 as #bus) << width) - 1
            };
            unshifted << lo
        };
    });

    FieldPos::Const {
        lo: lo_ident,
        mask: mask_ident,
    }
}

// ─── Enums ───────────────────────────────────────────────────────────────────

fn expand_enums(map: &RegisterMap) -> TokenStream {
    let mut out = TokenStream::new();
    for entry in &map.entries {
        for bf in &entry.bitfields {
            if let FieldType::Enum(def) = &bf.ty {
                out.extend(expand_enum(map, entry, def));
            }
        }
    }
    out
}

/// Emits an enum variant value retyped to `ty`: literal values become plain
/// literals (adopting `ty` by inference), expressions get an explicit cast.
fn variant_value(value: &ConstExpr, ty: &crate::ast::IntType) -> TokenStream {
    match value.value {
        Some(v) => {
            let lit = dec(v);
            quote!(#lit)
        }
        None => {
            let expr = &value.expr;
            quote!((#expr) as #ty)
        }
    }
}

fn expand_enum(map: &RegisterMap, entry: &RegisterEntry, def: &EnumDef) -> TokenStream {
    let vis = &map.vis;
    let ty = &entry.ty;
    let name = &def.name;
    let first = &def.variants[0].name;

    let variant_defs = def.variants.iter().map(|v| {
        let attrs = &v.attrs;
        let vname = &v.name;
        quote! {
            #(#attrs)*
            #vname
        }
    });

    let all_literal = def.variants.iter().all(|v| v.value.value.is_some());
    let from_raw_doc = format!(
        "Converts a raw field value into the enum. Values not matching any \
         declared variant map to [`{name}::{first}`]."
    );
    let from_raw_body = if all_literal {
        // Every value is a literal, so a `match` is possible; the first
        // variant is covered by the `_` arm.
        let arms = def.variants.iter().skip(1).map(|v| {
            let vname = &v.name;
            let value = variant_value(&v.value, ty);
            quote!(#value => Self::#vname,)
        });
        quote! {
            match raw {
                #(#arms)*
                _ => Self::#first,
            }
        }
    } else {
        // Non-literal values cannot be match patterns; compare explicitly.
        let checks = def.variants.iter().map(|v| {
            let vname = &v.name;
            let value = variant_value(&v.value, ty);
            quote! {
                if raw == #value {
                    return Self::#vname;
                }
            }
        });
        quote! {
            #(#checks)*
            Self::#first
        }
    };

    let to_raw_arms = def.variants.iter().map(|v| {
        let vname = &v.name;
        let value = variant_value(&v.value, ty);
        quote!(Self::#vname => #value,)
    });

    quote! {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        #vis enum #name {
            #(#variant_defs,)*
        }

        impl #name {
            #[doc = #from_raw_doc]
            #[inline]
            #vis fn from_raw(raw: #ty) -> Self {
                #from_raw_body
            }

            /// Returns the raw field value of this variant.
            #[inline]
            #vis fn to_raw(self) -> #ty {
                match self {
                    #(#to_raw_arms)*
                }
            }
        }

        impl ::core::fmt::Display for #name {
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                ::core::fmt::Debug::fmt(self, f)
            }
        }
    }
}

// ─── Struct and compile-time assertions ──────────────────────────────────────

fn expand_struct(map: &RegisterMap) -> TokenStream {
    let attrs = &map.attrs;
    let vis = &map.vis;
    let name = &map.name;

    // A register summary table for the struct's rustdoc.
    let mut lines = vec![
        "## Registers".to_string(),
        String::new(),
        "| Offset | Register | Access | Type | Description |".to_string(),
        "|---|---|---|---|---|".to_string(),
    ];
    for entry in &map.entries {
        let ty = match &entry.array_len {
            Some(len) => format!("[{}; {}]", entry.ty.ident, len.expr.to_token_stream()),
            None => entry.ty.ident.to_string(),
        };
        let doc = crate::ast::doc_string(&entry.attrs);
        let first_line = doc.lines().next().unwrap_or("").replace('|', "\\|");
        lines.push(format!(
            "| `{}` | `{}` | {} | `{}` | {} |",
            offset_display(&entry.offset),
            entry.name,
            entry.access.as_str(),
            ty,
            first_line,
        ));
    }
    let blank = has_doc(attrs).then(|| quote!(#[doc = ""]));
    let table = lines.iter().map(|line| quote!(#[doc = #line]));

    quote! {
        #(#attrs)*
        #blank
        #(#table)*
        #vis struct #name {
            devmem: ::std::sync::Arc<::ddevmem::DevMem>,
        }
    }
}

/// Emits `const` assertions for whatever `validate` could not prove at
/// expansion time (non-literal offsets, `usize` widths).
fn expand_const_asserts(map: &RegisterMap) -> TokenStream {
    let bus = &map.bus;
    let bus_size = map.bus.kind.size_bytes();
    let mut out = TokenStream::new();

    for entry in &map.entries {
        let ty = &entry.ty;

        if !(entry.offset.value.is_some() && bus_size.is_some()) {
            let offset = &entry.offset;
            let msg = format!(
                "register `{}`: offset is not aligned to the bus width",
                entry.name
            );
            out.extend(quote! {
                const _: () = assert!(#offset % ::core::mem::size_of::<#bus>() == 0, #msg);
            });
        }

        if !(entry.ty.kind.size_bytes().is_some() && bus_size.is_some()) {
            let msg = format!(
                "register `{}`: type is wider than the bus type",
                entry.name
            );
            out.extend(quote! {
                const _: () = assert!(
                    ::core::mem::size_of::<#ty>() <= ::core::mem::size_of::<#bus>(),
                    #msg
                );
            });
        }
    }

    out
}

// ─── The main impl block ─────────────────────────────────────────────────────

fn expand_impl(map: &RegisterMap) -> TokenStream {
    let name = &map.name;

    let mut consts = TokenStream::new();
    let mut methods = TokenStream::new();
    // Module-scope items (the whole-register writers of `wo` registers).
    let mut items = TokenStream::new();
    for entry in &map.entries {
        methods.extend(expand_entry(map, entry, &mut consts, &mut items));
    }

    let new = expand_new(map);
    let helpers = expand_helpers(map);

    // No `unsafe impl Send/Sync` is emitted: the struct's only field is an
    // `Arc<DevMem>`, and `DevMem` is `Send + Sync`, so the auto traits apply.
    quote! {
        #items

        impl #name {
            #consts
            #new
            #helpers
            #methods
        }
    }
}

fn expand_new(map: &RegisterMap) -> TokenStream {
    let bus = &map.bus;
    let bus_size = map.bus.kind.size_bytes();

    // Minimum region length: max(offset + count * bus_size) over all entries.
    let all_known = bus_size.is_some()
        && map.entries.iter().all(|e| {
            e.offset.value.is_some()
                && e.array_len.as_ref().is_none_or(|len| len.value.is_some())
        });

    let (length_check, doc_len) = if map.entries.is_empty() {
        (TokenStream::new(), None)
    } else if all_known {
        let required = map
            .entries
            .iter()
            .map(|e| {
                let count = e.array_len.as_ref().and_then(|l| l.value).unwrap_or(1);
                e.offset.value.unwrap() + count * bus_size.unwrap()
            })
            .max()
            .unwrap();
        let lit = hex(required);
        (
            quote! {
                if devmem.len() < #lit {
                    return ::core::option::Option::None;
                }
            },
            Some(format!("{required:#x}")),
        )
    } else {
        let ends = map.entries.iter().map(|e| {
            let offset = &e.offset;
            let count = match &e.array_len {
                Some(len) => quote!(#len),
                None => quote!(1),
            };
            quote! {
                let end = #offset + #count * ::core::mem::size_of::<#bus>();
                if end > required {
                    required = end;
                }
            }
        });
        (
            quote! {
                const __REQUIRED_LEN: usize = {
                    let mut required = 0usize;
                    #(#ends)*
                    required
                };
                if devmem.len() < __REQUIRED_LEN {
                    return ::core::option::Option::None;
                }
            },
            None,
        )
    };

    let none_doc = match doc_len {
        Some(len) => format!(
            "Returns `None` when the mapped region is shorter than the {len} bytes \
             this map requires."
        ),
        None => "Returns `None` when the mapped region is too short for the declared \
                 registers."
            .to_string(),
    };

    quote! {
        /// Creates the register map on top of `devmem`.
        ///
        #[doc = #none_doc]
        ///
        /// # Safety
        ///
        /// The caller must ensure that no other register map aliases the same
        /// region — [`DevMem`](::ddevmem::DevMem) does not track claimed ranges.
        pub unsafe fn new(devmem: ::std::sync::Arc<::ddevmem::DevMem>) -> ::core::option::Option<Self> {
            #length_check
            ::core::option::Option::Some(Self { devmem })
        }
    }
}

/// The private volatile-access helpers. Only the ones actually used by the
/// generated methods are emitted.
fn expand_helpers(map: &RegisterMap) -> TokenStream {
    let bus = &map.bus;
    let entries = &map.entries;

    let any_read = entries.iter().any(|e| e.access.can_read());
    let any_write = entries.iter().any(|e| e.access.can_write());
    let any_update = entries
        .iter()
        .any(|e| e.access == Access::Rw && !e.bitfields.is_empty());
    let any_index = entries.iter().any(RegisterEntry::is_array);

    let read = any_read.then(|| {
        quote! {
            /// Volatile bus-wide read at `offset`.
            ///
            /// SAFETY: sound because every offset passed by generated methods
            /// was bounds-checked in `new()` and is bus-aligned by construction.
            #[inline(always)]
            fn __read(&self, offset: usize) -> #bus {
                unsafe { self.devmem.read_unchecked(offset) }
            }
        }
    });
    let write = any_write.then(|| {
        quote! {
            /// Volatile bus-wide write at `offset`.
            ///
            /// SAFETY: sound because every offset passed by generated methods
            /// was bounds-checked in `new()` and is bus-aligned by construction.
            #[inline(always)]
            fn __write(&mut self, offset: usize, value: #bus) {
                unsafe { self.devmem.write_unchecked(offset, value) }
            }
        }
    });
    let update = any_update.then(|| {
        quote! {
            /// Replaces the `mask` bits at `offset` with `bits` (which must
            /// already be masked and shifted into position).
            #[inline(always)]
            fn __update(&mut self, offset: usize, mask: #bus, bits: #bus) {
                let old = self.__read(offset);
                self.__write(offset, (old & !mask) | bits);
            }
        }
    });
    let index = any_index.then(|| {
        quote! {
            /// Byte offset of element `idx` of the register array starting at
            /// `offset`, panicking on out-of-bounds indices.
            #[inline(always)]
            #[track_caller]
            fn __index(offset: usize, idx: usize, len: usize, register: &'static str) -> usize {
                assert!(
                    idx < len,
                    "index {idx} out of bounds for register array `{register}` (len {len})"
                );
                offset + idx * ::core::mem::size_of::<#bus>()
            }
        }
    });

    quote! {
        #read
        #write
        #update
        #index
    }
}

// ─── Per-register methods ────────────────────────────────────────────────────

fn expand_entry(
    map: &RegisterMap,
    entry: &RegisterEntry,
    consts: &mut TokenStream,
    items: &mut TokenStream,
) -> TokenStream {
    let vis = &map.vis;
    let bus = &map.bus;
    let reg = &entry.name;
    let reg_str = reg.to_string();
    let ty = &entry.ty;
    let attrs = &entry.attrs;
    let offset = &entry.offset;
    let same_ty = entry.ty.kind == map.bus.kind;

    // Bit positions are resolved for the whole register up front: the write
    // mask of any one field depends on which *other* fields must be forced to
    // zero during a read-modify-write.
    let positions: Vec<FieldPos> = entry
        .bitfields
        .iter()
        .map(|bf| field_pos(map, entry, bf, consts))
        .collect();
    let force_zero = entry
        .bitfields
        .iter()
        .zip(&positions)
        .filter(|(bf, _)| bf.access.forced_zero())
        .fold(Mask::Lit(0), |acc, (_, pos)| acc.or(pos.positioned_mask()));

    let offset_fn = format_ident!("{reg}_offset");
    let address_fn = format_ident!("{reg}_address");
    let set_fn = format_ident!("set_{reg}");
    let modify_fn = format_ident!("modify_{reg}");

    let element = element_display(entry);
    let at_offset = format!("(offset {})", offset_display(offset));

    // Signature fragment and offset expression for array vs. scalar entries.
    let (idx_param, idx_arg, off) = if entry.is_array() {
        (
            quote!(, idx: usize),
            quote!(idx),
            quote!(self.#offset_fn(idx)),
        )
    } else {
        (TokenStream::new(), TokenStream::new(), quote!(#offset))
    };

    let mut out = TokenStream::new();

    // `fifo_len()`
    if let Some(len) = &entry.array_len {
        let len_fn = format_ident!("{reg}_len");
        let doc = format!("Number of elements in the `{reg_str}` register array.");
        out.extend(quote! {
            #[doc = #doc]
            #[inline(always)]
            #vis fn #len_fn(&self) -> usize {
                #len
            }
        });
    }

    // `cr_offset()` / `fifo_offset(idx)`
    {
        let doc = format!("Byte offset of `{element}` within the mapped region.");
        let body = if let Some(len) = &entry.array_len {
            quote!(Self::__index(#offset, idx, #len, #reg_str))
        } else {
            quote!(#offset)
        };
        out.extend(quote! {
            #[doc = #doc]
            #[inline(always)]
            #vis fn #offset_fn(&self #idx_param) -> usize {
                #body
            }
        });
    }

    // `cr_address()`
    {
        let doc = format!("Physical address of `{element}`.");
        out.extend(quote! {
            #[doc = #doc]
            #[inline(always)]
            #vis fn #address_fn(&self #idx_param) -> usize {
                self.devmem.address() + self.#offset_fn(#idx_arg)
            }
        });
    }

    // Getter
    if entry.access.can_read() {
        let doc = docs(attrs, &format!("Volatile read of `{element}` {at_offset}."));
        let body = if same_ty {
            quote!(self.__read(#off))
        } else {
            quote!(self.__read(#off) as #ty)
        };
        out.extend(quote! {
            #doc
            #[inline(always)]
            #vis fn #reg(&self #idx_param) -> #ty {
                #body
            }
        });
    }

    // Setter
    if entry.access.can_write() {
        let doc = docs(attrs, &format!("Volatile write of `{element}` {at_offset}."));
        let value = if same_ty {
            quote!(value)
        } else {
            quote!(value as #bus)
        };
        out.extend(quote! {
            #doc
            #[inline(always)]
            #vis fn #set_fn(&mut self #idx_param, value: #ty) {
                self.__write(#off, #value)
            }
        });
    }

    // Modify (composed from the public getter and setter)
    if entry.access.can_read() && entry.access.can_write() {
        let mut summary = format!("Volatile read-modify-write of `{element}` {at_offset}.");
        if !force_zero.is_empty() {
            summary.push_str(
                " This is the raw escape hatch: `f` sees the value as read and its \
                 result is written back verbatim, so the `w1c` and `wo` bits of this \
                 register go back exactly as returned — acknowledging any flag that \
                 happened to be pending. Prefer the per-field accessors, which force \
                 those bits to zero.",
            );
        }
        let doc = docs(attrs, &summary);
        let idx_pass = if entry.is_array() {
            quote!(idx,)
        } else {
            TokenStream::new()
        };
        out.extend(quote! {
            #doc
            #[inline(always)]
            #vis fn #modify_fn(&mut self #idx_param, f: impl FnOnce(#ty) -> #ty) {
                let value = f(self.#reg(#idx_arg));
                self.#set_fn(#idx_pass value);
            }
        });
    }

    // Bitfields
    for (bf, pos) in entry.bitfields.iter().zip(&positions) {
        out.extend(expand_bitfield(
            map,
            entry,
            bf,
            pos,
            &force_zero,
            &idx_param,
            &off,
        ));
    }

    // A write-only register cannot be updated field by field without zeroing
    // everything else, so it also gets a builder that writes the whole
    // register in one transaction.
    if entry.access == Access::Wo && !entry.bitfields.is_empty() {
        items.extend(expand_writer(map, entry, &positions));

        let writer_ty = format_ident!("{}", writer_type_name(map, entry));
        let write_fn = format_ident!("write_{reg}");
        let summary = format!(
            "Writes `{element}` {at_offset} in a single transaction. Fields the \
             closure does not touch are written as zero."
        );
        let doc = docs(attrs, &summary);
        out.extend(quote! {
            #doc
            #[inline(always)]
            #vis fn #write_fn(&mut self #idx_param, f: impl FnOnce(#writer_ty) -> #writer_ty) {
                self.__write(#off, f(#writer_ty(0)).0)
            }
        });
    }

    out
}

/// Name of the builder type generated for a write-only register's
/// whole-register write (`UartRegs` + `cmd` → `UartRegsCmdWrite`).
pub fn writer_type_name(map: &RegisterMap, entry: &RegisterEntry) -> String {
    let mut pascal = String::new();
    for part in entry.name.to_string().split('_').filter(|p| !p.is_empty()) {
        let mut chars = part.chars();
        if let Some(first) = chars.next() {
            pascal.extend(first.to_uppercase());
            pascal.push_str(chars.as_str());
        }
    }
    format!("{}{}Write", map.name, pascal)
}

/// The builder behind `write_<reg>`: a bare bus word plus one chainable
/// method per writable field.
fn expand_writer(map: &RegisterMap, entry: &RegisterEntry, positions: &[FieldPos]) -> TokenStream {
    let vis = &map.vis;
    let bus = &map.bus;
    let writer_ty = format_ident!("{}", writer_type_name(map, entry));
    let element = element_display(entry);

    let mut methods = TokenStream::new();
    for (bf, pos) in entry.bitfields.iter().zip(positions) {
        if !bf.access.can_write(entry.access) {
            continue;
        }
        let name = &bf.name;
        let mask = pos.positioned_mask().tokens();
        let bits = bits_display(bf);

        if bf.access == FieldAccess::W1c {
            let doc = docs(
                &bf.attrs,
                &format!("Requests a clear of {bits} by writing ones to it."),
            );
            methods.extend(quote! {
                #doc
                #[inline(always)]
                #vis fn #name(mut self) -> Self {
                    self.0 |= #mask;
                    self
                }
            });
        } else {
            let (value_ty, bits_expr) = field_write_expr(map, entry, bf, pos);
            let doc = docs(&bf.attrs, &format!("Sets {bits}."));
            methods.extend(quote! {
                #doc
                #[inline(always)]
                #vis fn #name(mut self, value: #value_ty) -> Self {
                    self.0 = (self.0 & !#mask) | #bits_expr;
                    self
                }
            });
        }
    }

    let doc = format!(
        "Builder for a whole-register write of `{element}`.\n\n\
         Starts with every bit zero; each method sets one field. Obtained from \
         the register map's `write_{}` method.",
        entry.name
    );
    quote! {
        #[doc = #doc]
        #[derive(Debug, Clone, Copy)]
        #vis struct #writer_ty(#bus);

        impl #writer_ty {
            #methods
        }
    }
}

/// The setter's value type and the expression that positions `value` into the
/// field, in the bus domain.
fn field_write_expr(
    map: &RegisterMap,
    entry: &RegisterEntry,
    bf: &Bitfield,
    pos: &FieldPos,
) -> (TokenStream, TokenStream) {
    let bus = &map.bus;
    let ty = &entry.ty;
    let same_ty = entry.ty.kind == map.bus.kind;

    match &bf.ty {
        FieldType::Raw => {
            let value = if same_ty {
                quote!(value)
            } else {
                quote!((value as #bus))
            };
            (quote!(#ty), pos.insert(value, true))
        }
        FieldType::Bool => (quote!(bool), pos.insert(quote!((value as #bus)), false)),
        FieldType::Int(cast) => {
            let value = if cast.kind == map.bus.kind {
                quote!(value)
            } else {
                quote!((value as #bus))
            };
            (quote!(#cast), pos.insert(value, true))
        }
        FieldType::Enum(def) => {
            let ename = &def.name;
            let value = if same_ty {
                quote!(value.to_raw())
            } else {
                quote!((value.to_raw() as #bus))
            };
            // Literal variant values were validated to fit the field.
            let masked = !def.variants.iter().all(|v| v.value.value.is_some())
                || matches!(pos, FieldPos::Const { .. });
            (quote!(#ename), pos.insert(value, masked))
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn expand_bitfield(
    map: &RegisterMap,
    entry: &RegisterEntry,
    bf: &Bitfield,
    pos: &FieldPos,
    force_zero: &Mask,
    idx_param: &TokenStream,
    off: &TokenStream,
) -> TokenStream {
    let vis = &map.vis;
    let ty = &entry.ty;
    let reg = &entry.name;
    let field_attrs = &bf.attrs;
    let same_ty = entry.ty.kind == map.bus.kind;

    let getter = format_ident!("{}_{}", reg, bf.name);

    let element = element_display(entry);
    let bits = bits_display(bf);
    let read = quote!(self.__read(#off));

    // (getter return type, decoded getter body)
    let (api_ty, get_body): (TokenStream, TokenStream) = match &bf.ty {
        FieldType::Raw => {
            let extract = pos.extract(read.clone());
            let body = if same_ty {
                extract
            } else {
                quote!((#extract) as #ty)
            };
            (quote!(#ty), body)
        }
        FieldType::Bool => (quote!(bool), pos.test_bit(read.clone())),
        FieldType::Int(cast) => {
            let extract = pos.extract(read.clone());
            let body = if cast.kind == map.bus.kind {
                extract
            } else {
                quote!((#extract) as #cast)
            };
            (quote!(#cast), body)
        }
        FieldType::Enum(def) => {
            let ename = &def.name;
            let extract = pos.extract(read.clone());
            let raw = if same_ty {
                quote!(#extract)
            } else {
                quote!((#extract) as #ty)
            };
            (quote!(#ename), quote!(#ename::from_raw(#raw)))
        }
    };

    let mut out = TokenStream::new();

    if bf.access.can_read(entry.access) {
        let doc = docs(field_attrs, &format!("Reads {bits} of `{element}`."));
        out.extend(quote! {
            #doc
            #[inline(always)]
            #vis fn #getter(&self #idx_param) -> #api_ty {
                #get_body
            }
        });
    }

    if !bf.access.can_write(entry.access) {
        return out;
    }

    // Sentence appended whenever a read-modify-write has to suppress bits
    // belonging to *other* fields.
    let others_forced = if force_zero.is_empty() {
        String::new()
    } else {
        format!(
            " The `w1c` and `wo` bits of `{}` are written as zero, so no flag is \
             acknowledged and no command re-triggered.",
            entry.name
        )
    };
    let field_mask = pos.positioned_mask();

    if bf.access == FieldAccess::W1c {
        let clear_fn = format_ident!("clear_{}_{}", reg, bf.name);
        let mask_tokens = field_mask.tokens();
        let (summary, body) = if entry.access.can_read() {
            let keep = force_zero.clone().tokens();
            (
                format!(
                    "Clears {bits} of `{element}` by writing ones to it \
                     (write-1-to-clear).{others_forced}"
                ),
                quote!(self.__update(#off, #keep, #mask_tokens)),
            )
        } else {
            (
                format!(
                    "Clears {bits} of `{element}` by writing ones to it \
                     (write-1-to-clear). The register is write-only, so every other \
                     bit is written as zero."
                ),
                quote!(self.__write(#off, #mask_tokens)),
            )
        };
        let doc = docs(field_attrs, &summary);
        out.extend(quote! {
            #doc
            #[inline(always)]
            #vis fn #clear_fn(&mut self #idx_param) {
                #body
            }
        });
        return out;
    }

    let setter = format_ident!("set_{}_{}", reg, bf.name);
    let (value_ty, bits_expr) = field_write_expr(map, entry, bf, pos);
    let (summary, body) = if entry.access.can_read() {
        let mask = force_zero.clone().or(field_mask).tokens();
        (
            format!(
                "Writes {bits} of `{element}` via read-modify-write; the other bits \
                 are preserved.{others_forced}"
            ),
            quote!(self.__update(#off, #mask, #bits_expr)),
        )
    } else {
        (
            format!(
                "Writes {bits} of `{element}`. The register is write-only, so a \
                 read-modify-write is impossible: all other bits are written as \
                 zero. Use `write_{}` to set several fields in one transaction.",
                entry.name
            ),
            quote!(self.__write(#off, #bits_expr)),
        )
    };
    let doc = docs(field_attrs, &summary);
    out.extend(quote! {
        #doc
        #[inline(always)]
        #vis fn #setter(&mut self #idx_param, value: #value_ty) {
            #body
        }
    });

    out
}
