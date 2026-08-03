//! Generation of the `ddevmem::web::RegisterMapInfo` implementation
//! (compiled only with the `web` feature, which `ddevmem/web` enables).
//!
//! The macro emits pure *data* — a `&'static [spec::Register]` table — and
//! one-line trait methods that hand it to library code in `ddevmem::web`.
//! All expansion and validation logic lives in the library, keeping the
//! generated tokens small.

use proc_macro2::TokenStream;
use quote::quote;

use crate::ast::{ConstExpr, FieldType, IntKind, RegisterEntry, RegisterMap};

pub fn expand(map: &RegisterMap) -> TokenStream {
    let name = &map.name;
    let name_str = name.to_string();
    let bus = &map.bus;
    let specs = map.entries.iter().map(spec_tokens);

    // A u64 bus accepts any u64 value; narrower buses reject values that
    // would be silently truncated. (`usize` is checked too — it may be 32-bit.)
    let range_check = (map.bus.kind != IntKind::U64).then(|| {
        quote! {
            if value > <#bus>::MAX as u64 {
                return ::core::option::Option::None;
            }
        }
    });

    quote! {
        impl #name {
            /// Register metadata consumed by [`ddevmem::web`](::ddevmem::web).
            const __SPEC: &'static [::ddevmem::web::spec::Register] = &[
                #(#specs),*
            ];
        }

        impl ::ddevmem::web::RegisterMapInfo for #name {
            fn map_name(&self) -> &'static str {
                #name_str
            }

            fn bus_width(&self) -> usize {
                ::core::mem::size_of::<#bus>()
            }

            fn base_address(&self) -> usize {
                self.devmem.address()
            }

            fn registers(&self) -> ::std::vec::Vec<::ddevmem::web::RegisterInfo> {
                ::ddevmem::web::spec::expand(Self::__SPEC, ::core::mem::size_of::<#bus>())
            }

            fn read_register(&self, offset: usize) -> ::core::option::Option<u64> {
                if !::ddevmem::web::spec::is_readable(Self::__SPEC, ::core::mem::size_of::<#bus>(), offset) {
                    return ::core::option::Option::None;
                }
                self.devmem.read::<#bus>(offset).map(|value| value as u64)
            }

            fn write_register(&mut self, offset: usize, value: u64) -> ::core::option::Option<()> {
                if !::ddevmem::web::spec::is_writable(Self::__SPEC, ::core::mem::size_of::<#bus>(), offset) {
                    return ::core::option::Option::None;
                }
                #range_check
                self.devmem.write::<#bus>(offset, value as #bus)
            }
        }
    }
}

/// A `ConstExpr` as a `u32` value (bit positions in the spec table).
fn u32_tokens(value: &ConstExpr) -> TokenStream {
    match value.value {
        Some(_) => quote!(#value),
        None => quote!(#value as u32),
    }
}

/// A `ConstExpr` as a `u64` value (variant values in the spec table).
fn u64_tokens(value: &ConstExpr) -> TokenStream {
    match value.value {
        Some(_) => quote!(#value),
        None => quote!(#value as u64),
    }
}

fn spec_tokens(entry: &RegisterEntry) -> TokenStream {
    let name = entry.name.to_string();
    let doc = crate::ast::doc_string(&entry.attrs);
    let offset = &entry.offset;
    let ty = &entry.ty;
    let access = match entry.access {
        crate::ast::Access::Rw => quote!(::ddevmem::web::spec::Access::Rw),
        crate::ast::Access::Ro => quote!(::ddevmem::web::spec::Access::Ro),
        crate::ast::Access::Wo => quote!(::ddevmem::web::spec::Access::Wo),
    };
    let count = match &entry.array_len {
        Some(len) => quote!(#len),
        None => quote!(1),
    };

    let bitfields = entry.bitfields.iter().map(|bf| {
        let bf_name = bf.name.to_string();
        let bf_doc = crate::ast::doc_string(&bf.attrs);
        let lo = u32_tokens(&bf.lo);
        let hi = u32_tokens(&bf.hi);

        let (type_name, variants) = match &bf.ty {
            FieldType::Raw => ("raw".to_string(), quote!(&[])),
            FieldType::Bool => (
                "bool".to_string(),
                quote!(::ddevmem::web::spec::BOOL_VARIANTS),
            ),
            FieldType::Int(cast) => (cast.ident.to_string(), quote!(&[])),
            FieldType::Enum(def) => {
                let entries = def.variants.iter().map(|v| {
                    let v_name = v.name.to_string();
                    let v_value = u64_tokens(&v.value);
                    quote! {
                        ::ddevmem::web::spec::Variant {
                            name: #v_name,
                            value: #v_value,
                        }
                    }
                });
                (def.name.to_string(), quote!(&[#(#entries),*]))
            }
        };

        let access = bf.access.as_str(entry.access);

        quote! {
            ::ddevmem::web::spec::Bitfield {
                name: #bf_name,
                doc: #bf_doc,
                lo: #lo,
                hi: #hi,
                access: #access,
                type_name: #type_name,
                variants: #variants,
            }
        }
    });

    quote! {
        ::ddevmem::web::spec::Register {
            name: #name,
            doc: #doc,
            offset: #offset,
            access: #access,
            width_bits: ::core::mem::size_of::<#ty>() * 8,
            count: #count,
            bitfields: &[#(#bitfields),*],
        }
    }
}
