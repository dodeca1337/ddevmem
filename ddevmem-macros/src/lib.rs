use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::{
    braced,
    parse::{Parse, ParseStream},
    punctuated::Punctuated,
    Attribute, Expr, Ident, Result, Token, Type, Visibility,
};

// ─── AST ─────────────────────────────────────────────────────────────────────

struct RegisterMap {
    attrs: Vec<Attribute>,
    vis: Visibility,
    name: Ident,
    bus: Type,
    entries: Vec<RegisterEntry>,
}

struct RegisterEntry {
    offset: Expr,
    attrs: Vec<Attribute>,
    kind: AccessKind,
    name: Ident,
    ty: Type,
    /// `Some(N)` when the entry was declared as `[T; N]`, otherwise `None`.
    /// When set, `ty` is the *element* type and the entry generates an
    /// indexed API (`name(idx)`, `set_name(idx, v)`, …).
    array_len: Option<Expr>,
    bitfields: Vec<Bitfield>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AccessKind {
    Rw,
    Ro,
    Wo,
}

struct Bitfield {
    attrs: Vec<Attribute>,
    name: Ident,
    lo: Expr,
    hi: Expr,
    field_type: FieldType,
}

enum FieldType {
    Raw,
    Bool,
    Cast(Type),
    Enum(EnumDef),
}

struct EnumDef {
    name: Ident,
    variants: Vec<EnumVariant>,
}

struct EnumVariant {
    name: Ident,
    value: Expr,
}

// ─── Parse ───────────────────────────────────────────────────────────────────

/// Parse a bit-position expression: a literal integer or a parenthesized
/// expression. This avoids syn's greedy `Expr::parse` consuming `0..=2` as
/// a range.
fn parse_bit_expr(input: ParseStream) -> Result<Expr> {
    if input.peek(syn::token::Paren) {
        let content;
        syn::parenthesized!(content in input);
        let inner: Expr = content.parse()?;
        Ok(syn::parse_quote!((#inner)))
    } else {
        let lit: syn::LitInt = input.parse()?;
        Ok(Expr::Lit(syn::ExprLit {
            attrs: vec![],
            lit: syn::Lit::Int(lit),
        }))
    }
}

impl Parse for RegisterMap {
    fn parse(input: ParseStream) -> Result<Self> {
        let attrs = input.call(Attribute::parse_outer)?;
        let vis: Visibility = input.parse()?;
        input.parse::<Token![unsafe]>()?;
        let map_kw: Ident = input.parse()?;
        if map_kw != "map" {
            return Err(syn::Error::new(map_kw.span(), "expected `map`"));
        }
        let name: Ident = input.parse()?;

        let bus: Type = if input.peek(syn::token::Paren) {
            let content;
            syn::parenthesized!(content in input);
            content.parse()?
        } else {
            syn::parse_quote!(usize)
        };

        let content;
        braced!(content in input);

        let mut entries = Vec::new();
        while !content.is_empty() {
            entries.push(content.parse()?);
            if content.is_empty() {
                break;
            }
            let _ = content.parse::<Token![,]>();
        }

        Ok(RegisterMap {
            attrs,
            vis,
            name,
            bus,
            entries,
        })
    }
}

impl Parse for RegisterEntry {
    fn parse(input: ParseStream) -> Result<Self> {
        let offset: Expr = input.parse()?;
        input.parse::<Token![=>]>()?;
        let attrs = input.call(Attribute::parse_outer)?;

        let kind_ident: Ident = input.parse()?;
        let kind = match kind_ident.to_string().as_str() {
            "rw" => AccessKind::Rw,
            "ro" => AccessKind::Ro,
            "wo" => AccessKind::Wo,
            _ => {
                return Err(syn::Error::new(
                    kind_ident.span(),
                    "expected `rw`, `ro`, or `wo`",
                ))
            }
        };

        let name: Ident = input.parse()?;
        input.parse::<Token![:]>()?;
        let ty_raw: Type = input.parse()?;

        // Detect `[T; N]` — element type goes into `ty`, length into
        // `array_len`. Anything else is a single-register entry.
        let (ty, array_len) = match ty_raw {
            Type::Array(arr) => (*arr.elem, Some(arr.len)),
            other => (other, None),
        };

        let bitfields = if input.peek(syn::token::Brace) {
            let content;
            braced!(content in input);
            let mut fields = Vec::new();
            while !content.is_empty() {
                fields.push(content.parse()?);
                if content.is_empty() {
                    break;
                }
                // Consume trailing comma if present
                let _ = content.parse::<Token![,]>();
            }
            fields
        } else {
            Vec::new()
        };

        Ok(RegisterEntry {
            offset,
            attrs,
            kind,
            name,
            ty,
            array_len,
            bitfields,
        })
    }
}

impl Parse for Bitfield {
    fn parse(input: ParseStream) -> Result<Self> {
        let attrs = input.call(Attribute::parse_outer)?;
        let name: Ident = input.parse()?;
        input.parse::<Token![:]>()?;

        let lo = parse_bit_expr(input)?;

        // Check for range: ..= or ..
        let hi = if input.peek(Token![..=]) {
            input.parse::<Token![..=]>()?;
            parse_bit_expr(input)?
        } else if input.peek(Token![..]) {
            input.parse::<Token![..]>()?;
            let hi_raw = parse_bit_expr(input)?;
            // Exclusive: subtract 1
            syn::parse_quote!((#hi_raw) - 1)
        } else {
            // Single bit: hi = lo
            lo.clone()
        };

        // Check for `as ...`
        let field_type = if input.peek(Token![as]) {
            input.parse::<Token![as]>()?;

            if input.peek(Token![enum]) {
                input.parse::<Token![enum]>()?;
                let enum_name: Ident = input.parse()?;
                let content;
                braced!(content in input);
                let variants: Punctuated<EnumVariant, Token![,]> =
                    content.parse_terminated(EnumVariant::parse, Token![,])?;
                FieldType::Enum(EnumDef {
                    name: enum_name,
                    variants: variants.into_iter().collect(),
                })
            } else if input.peek(Ident) {
                let ident: Ident = input.fork().parse()?;
                if ident == "bool" {
                    let _: Ident = input.parse()?;
                    FieldType::Bool
                } else {
                    let ty: Type = input.parse()?;
                    FieldType::Cast(ty)
                }
            } else {
                let ty: Type = input.parse()?;
                FieldType::Cast(ty)
            }
        } else {
            FieldType::Raw
        };

        Ok(Bitfield {
            attrs,
            name,
            lo,
            hi,
            field_type,
        })
    }
}

impl Parse for EnumVariant {
    fn parse(input: ParseStream) -> Result<Self> {
        let name: Ident = input.parse()?;
        input.parse::<Token![=]>()?;
        let value: Expr = input.parse()?;
        Ok(EnumVariant { name, value })
    }
}

// ─── Code generation ─────────────────────────────────────────────────────────

impl AccessKind {
    fn has_read(self) -> bool {
        matches!(self, AccessKind::Rw | AccessKind::Ro)
    }
    fn has_write(self) -> bool {
        matches!(self, AccessKind::Rw | AccessKind::Wo)
    }
    fn has_modify(self) -> bool {
        self == AccessKind::Rw
    }
    #[cfg_attr(not(feature = "web"), allow(dead_code))]
    fn as_str(self) -> &'static str {
        match self {
            AccessKind::Rw => "rw",
            AccessKind::Ro => "ro",
            AccessKind::Wo => "wo",
        }
    }
}

#[cfg_attr(not(feature = "web"), allow(dead_code))]
fn extract_doc_string(attrs: &[Attribute]) -> String {
    let mut doc = String::new();
    for attr in attrs {
        if attr.path().is_ident("doc") {
            if let syn::Meta::NameValue(nv) = &attr.meta {
                if let Expr::Lit(syn::ExprLit {
                    lit: syn::Lit::Str(s),
                    ..
                }) = &nv.value
                {
                    doc.push_str(&s.value());
                }
            }
        }
    }
    doc
}

fn gen_enum_defs(vis: &Visibility, ty: &Type, entries: &[RegisterEntry]) -> TokenStream2 {
    let mut tokens = TokenStream2::new();
    for entry in entries {
        for bf in &entry.bitfields {
            if let FieldType::Enum(enum_def) = &bf.field_type {
                let ename = &enum_def.name;
                let variant_names: Vec<_> = enum_def.variants.iter().map(|v| &v.name).collect();
                let variant_values: Vec<_> = enum_def.variants.iter().map(|v| &v.value).collect();

                tokens.extend(quote! {
                    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
                    #vis enum #ename {
                        #(#variant_names,)*
                    }

                    impl #ename {
                        /// Convert from a raw register value.
                        ///
                        /// Unknown values map to the first declared variant.
                        #[inline]
                        #[allow(unreachable_patterns)]
                        pub fn from_raw(v: #ty) -> Self {
                            match v {
                                #(#variant_values => Self::#variant_names,)*
                                _ => {
                                    let _variants = [#(Self::#variant_names,)*];
                                    _variants[0]
                                }
                            }
                        }

                        /// Convert to raw register value.
                        #[inline]
                        pub fn to_raw(self) -> #ty {
                            match self {
                                #(Self::#variant_names => #variant_values as #ty,)*
                            }
                        }
                    }

                    impl ::core::fmt::Display for #ename {
                        fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                            ::core::fmt::Debug::fmt(self, f)
                        }
                    }
                });
            }
        }
    }
    tokens
}

fn gen_bounds_checks(bus: &Type, entries: &[RegisterEntry]) -> TokenStream2 {
    let mut checks = TokenStream2::new();
    for entry in entries {
        let offset = &entry.offset;
        let ty = &entry.ty;
        let count_expr = match &entry.array_len {
            Some(n) => quote! { (#n) },
            None => quote! { 1usize },
        };
        checks.extend(quote! {
            const _: () = assert!(
                ::core::mem::size_of::<#ty>() <= ::core::mem::size_of::<#bus>(),
                "register type must not be wider than bus type"
            );
            const _: () = assert!(
                (#offset) % ::core::mem::align_of::<#bus>() == 0,
                "register offset must be aligned to bus width"
            );
            if (#offset) + (#count_expr) * ::core::mem::size_of::<#bus>() > devmem.len() {
                return None;
            }
        });
    }
    checks
}

fn gen_register_methods(vis: &Visibility, bus: &Type, entry: &RegisterEntry) -> TokenStream2 {
    let name = &entry.name;
    let ty = &entry.ty;
    let offset = &entry.offset;
    let attrs = &entry.attrs;

    let offset_fn = format_ident!("{}_offset", name);
    let address_fn = format_ident!("{}_address", name);
    let set_fn = format_ident!("set_{}", name);
    let modify_fn = format_ident!("modify_{}", name);

    // Effective offset expression. For arrays it includes an index parameter.
    // `idx_param` is appended to the method signature when non-empty.
    let (idx_param, eff_offset, len_method) = match &entry.array_len {
        Some(n) => {
            let len_fn = format_ident!("{}_len", name);
            (
                quote! { , idx: usize },
                quote! { ((#offset) + idx * ::core::mem::size_of::<#bus>()) },
                quote! {
                    /// Number of elements in this register array.
                    #[inline(always)]
                    #vis fn #len_fn(&self) -> usize {
                        #n
                    }
                },
            )
        }
        None => (
            TokenStream2::new(),
            quote! { (#offset) },
            TokenStream2::new(),
        ),
    };

    // Runtime bounds check (only for arrays).
    let bounds = match &entry.array_len {
        Some(n) => quote! {
            assert!(idx < (#n), concat!("index out of bounds for `", stringify!(#name), "`"));
        },
        None => TokenStream2::new(),
    };

    let mut methods = quote! {
        #len_method

        /// Returns the offset of the register within the DevMem.
        #[inline(always)]
        #vis fn #offset_fn(&self #idx_param) -> usize {
            #bounds
            #eff_offset
        }

        /// Returns the address of the register.
        #[inline(always)]
        #vis fn #address_fn(&self #idx_param) -> usize {
            #bounds
            self.devmem.address() + #eff_offset
        }
    };

    if entry.kind.has_read() {
        methods.extend(quote! {
            #(#attrs)*
            #[inline(always)]
            #vis fn #name(&self #idx_param) -> #ty {
                #bounds
                unsafe { ::core::ptr::read_volatile(self.devmem.as_ptr().add(#eff_offset) as *const #bus) as #ty }
            }
        });
    }

    if entry.kind.has_write() {
        methods.extend(quote! {
            #(#attrs)*
            #[inline(always)]
            #vis fn #set_fn(&mut self #idx_param, value: #ty) {
                #bounds
                unsafe { ::core::ptr::write_volatile(self.devmem.as_ptr().add(#eff_offset) as *mut #bus, value as #bus) }
            }
        });
    }

    if entry.kind.has_modify() {
        methods.extend(quote! {
            #(#attrs)*
            #[inline(always)]
            #vis fn #modify_fn(&mut self #idx_param, f: impl FnOnce(#ty) -> #ty) {
                #bounds
                unsafe {
                    let ptr = self.devmem.as_ptr().add(#eff_offset);
                    let val = ::core::ptr::read_volatile(ptr as *const #bus) as #ty;
                    ::core::ptr::write_volatile(ptr as *mut #bus, f(val) as #bus);
                }
            }
        });
    }

    // Bitfield methods
    for bf in &entry.bitfields {
        methods.extend(gen_bitfield_methods(vis, bus, entry, bf));
    }

    methods
}

fn gen_bitfield_methods(
    vis: &Visibility,
    bus: &Type,
    entry: &RegisterEntry,
    bf: &Bitfield,
) -> TokenStream2 {
    let reg_name = &entry.name;
    let ty = &entry.ty;
    let offset = &entry.offset;
    let bf_attrs = &bf.attrs;
    let lo = &bf.lo;
    let hi = &bf.hi;

    let getter_name = format_ident!("{}_{}", reg_name, bf.name);
    let setter_name = format_ident!("set_{}_{}", reg_name, bf.name);

    // Array vs scalar register: bitfield methods take an extra `idx`
    // parameter when the underlying register is an array.
    let (idx_param, eff_offset, bounds) = match &entry.array_len {
        Some(n) => (
            quote! { , idx: usize },
            quote! { ((#offset) + idx * ::core::mem::size_of::<#bus>()) },
            quote! {
                assert!(idx < (#n), concat!("index out of bounds for `", stringify!(#reg_name), "`"));
            },
        ),
        None => (
            TokenStream2::new(),
            quote! { (#offset) },
            TokenStream2::new(),
        ),
    };

    let hi_expr: TokenStream2 = if bf.hi == bf.lo {
        // Single bit — same expression
        quote! { #hi }
    } else {
        quote! { #hi }
    };

    // Width and mask computation
    let width_and_mask = quote! {
        let width: u32 = (#hi_expr) - (#lo) + 1;
        let mask: #ty = if width >= <#ty>::BITS { <#ty>::MAX } else { (1 << width) - 1 };
    };

    let read_raw = quote! {
        let raw = unsafe { ::core::ptr::read_volatile(self.devmem.as_ptr().add(#eff_offset) as *const #bus) } as #ty;
    };

    let rmw_body = |value_expr: TokenStream2| {
        quote! {
            #width_and_mask
            unsafe {
                let ptr = self.devmem.as_ptr().add(#eff_offset);
                let old = ::core::ptr::read_volatile(ptr as *const #bus) as #ty;
                let new = (old & !(mask << (#lo))) | ((#value_expr & mask) << (#lo));
                ::core::ptr::write_volatile(ptr as *mut #bus, new as #bus);
            }
        }
    };

    let mut methods = TokenStream2::new();

    match &bf.field_type {
        FieldType::Raw => {
            if entry.kind.has_read() {
                methods.extend(quote! {
                    #(#bf_attrs)*
                    #[inline(always)]
                    #vis fn #getter_name(&self #idx_param) -> #ty {
                        #bounds
                        #read_raw
                        #width_and_mask
                        (raw >> (#lo)) & mask
                    }
                });
            }
            if entry.kind.has_write() {
                let rmw = rmw_body(quote! { value });
                methods.extend(quote! {
                    #(#bf_attrs)*
                    #[inline(always)]
                    #vis fn #setter_name(&mut self #idx_param, value: #ty) {
                        #bounds
                        #rmw
                    }
                });
            }
        }
        FieldType::Bool => {
            if entry.kind.has_read() {
                methods.extend(quote! {
                    #(#bf_attrs)*
                    #[inline(always)]
                    #vis fn #getter_name(&self #idx_param) -> bool {
                        #bounds
                        #read_raw
                        #width_and_mask
                        ((raw >> (#lo)) & mask) != 0
                    }
                });
            }
            if entry.kind.has_write() {
                let rmw = rmw_body(quote! { value });
                methods.extend(quote! {
                    #(#bf_attrs)*
                    #[inline(always)]
                    #vis fn #setter_name(&mut self #idx_param, value: bool) {
                        #bounds
                        let value = value as #ty;
                        #rmw
                    }
                });
            }
        }
        FieldType::Cast(cast_ty) => {
            if entry.kind.has_read() {
                methods.extend(quote! {
                    #(#bf_attrs)*
                    #[inline(always)]
                    #vis fn #getter_name(&self #idx_param) -> #cast_ty {
                        #bounds
                        #read_raw
                        #width_and_mask
                        ((raw >> (#lo)) & mask) as #cast_ty
                    }
                });
            }
            if entry.kind.has_write() {
                let rmw = rmw_body(quote! { value });
                methods.extend(quote! {
                    #(#bf_attrs)*
                    #[inline(always)]
                    #vis fn #setter_name(&mut self #idx_param, value: #cast_ty) {
                        #bounds
                        let value = value as #ty;
                        #rmw
                    }
                });
            }
        }
        FieldType::Enum(enum_def) => {
            let ename = &enum_def.name;
            if entry.kind.has_read() {
                methods.extend(quote! {
                    #(#bf_attrs)*
                    #[inline(always)]
                    #vis fn #getter_name(&self #idx_param) -> #ename {
                        #bounds
                        #read_raw
                        #width_and_mask
                        #ename::from_raw((raw >> (#lo)) & mask)
                    }
                });
            }
            if entry.kind.has_write() {
                let rmw = rmw_body(quote! { value });
                methods.extend(quote! {
                    #(#bf_attrs)*
                    #[inline(always)]
                    #vis fn #setter_name(&mut self #idx_param, value: #ename) {
                        #bounds
                        let value = value.to_raw();
                        #rmw
                    }
                });
            }
        }
    }

    methods
}

fn gen_web_impl(map: &RegisterMap) -> TokenStream2 {
    #[cfg(not(feature = "web"))]
    {
        let _ = map;
        TokenStream2::new()
    }

    #[cfg(feature = "web")]
    {
        let name = &map.name;
        let bus = &map.bus;

        let mut register_infos = TokenStream2::new();
        for entry in &map.entries {
            let reg_name_str = entry.name.to_string();
            let offset = &entry.offset;
            let ty = &entry.ty;
            let access_str = entry.kind.as_str();
            let doc_str = extract_doc_string(&entry.attrs);

            let mut bitfield_pushes = TokenStream2::new();
            for bf in &entry.bitfields {
                let bf_name_str = bf.name.to_string();
                let bf_doc = extract_doc_string(&bf.attrs);
                let lo = &bf.lo;
                let hi = &bf.hi;

                let (ft_str, variants_expr) = match &bf.field_type {
                    FieldType::Raw => ("raw".to_string(), quote! { Vec::new() }),
                    FieldType::Bool => (
                        "bool".to_string(),
                        quote! {
                            vec![
                                ::ddevmem::web::VariantInfo { name: "false", value: 0 },
                                ::ddevmem::web::VariantInfo { name: "true",  value: 1 },
                            ]
                        },
                    ),
                    FieldType::Cast(ct) => {
                        let ct_str = quote!(#ct).to_string();
                        (ct_str, quote! { Vec::new() })
                    }
                    FieldType::Enum(ed) => {
                        let en_str = ed.name.to_string();
                        let v_names: Vec<String> =
                            ed.variants.iter().map(|v| v.name.to_string()).collect();
                        let v_vals: Vec<&Expr> = ed.variants.iter().map(|v| &v.value).collect();
                        (
                            en_str,
                            quote! {
                                vec![
                                    #(::ddevmem::web::VariantInfo {
                                        name: #v_names,
                                        value: #v_vals as u64,
                                    },)*
                                ]
                            },
                        )
                    }
                };

                bitfield_pushes.extend(quote! {
                    bitfields.push(::ddevmem::web::BitfieldInfo {
                        name: #bf_name_str,
                        doc: #bf_doc,
                        lo: #lo,
                        hi: #hi,
                        field_type: #ft_str,
                        variants: #variants_expr,
                    });
                });
            }

            let push_one = quote! {
                {
                    let mut bitfields = Vec::new();
                    #bitfield_pushes
                    regs.push(::ddevmem::web::RegisterInfo {
                        name: __name,
                        doc: #doc_str,
                        offset: __off,
                        access: #access_str,
                        width: ::core::mem::size_of::<#ty>() * 8,
                        bitfields,
                    });
                }
            };

            match &entry.array_len {
                None => {
                    register_infos.extend(quote! {
                        {
                            let __name: ::std::string::String =
                                ::std::string::String::from(#reg_name_str);
                            let __off: usize = #offset;
                            #push_one
                        }
                    });
                }
                Some(n) => {
                    register_infos.extend(quote! {
                        for __i in 0..(#n) {
                            let __name: ::std::string::String =
                                ::std::format!("{}[{}]", #reg_name_str, __i);
                            let __off: usize = (#offset) + __i * ::core::mem::size_of::<#bus>();
                            #push_one
                        }
                    });
                }
            }
        }

        let name_str = name.to_string();
        quote! {
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

                fn registers(&self) -> Vec<::ddevmem::web::RegisterInfo> {
                    let mut regs = Vec::new();
                    #register_infos
                    regs
                }

                fn read_register(&self, offset: usize) -> Option<u64> {
                    self.devmem.read::<#bus>(offset).map(|v| v as u64)
                }

                fn write_register(&mut self, offset: usize, value: u64) -> Option<()> {
                    self.devmem.write::<#bus>(offset, value as #bus)
                }
            }
        }
    }
}

fn generate(map: RegisterMap) -> TokenStream2 {
    let attrs = &map.attrs;
    let vis = &map.vis;
    let name = &map.name;
    let bus = &map.bus;

    // Generate enum definitions at module scope
    let enum_defs = gen_enum_defs(vis, &map.entries[0].ty, &map.entries);

    // Bounds checks in new()
    let bounds_checks = gen_bounds_checks(bus, &map.entries);

    // Register methods
    let mut all_methods = TokenStream2::new();
    for entry in &map.entries {
        all_methods.extend(gen_register_methods(vis, bus, entry));
    }

    // Web impl (conditionally compiled)
    let web_impl = gen_web_impl(&map);

    quote! {
        #enum_defs

        #(#attrs)*
        #vis struct #name {
            devmem: ::std::sync::Arc<::ddevmem::DevMem>,
        }

        impl #name {
            /// Creates a new register map wrapping the given [`DevMem`](::ddevmem::DevMem).
            ///
            /// Returns `None` if any declared register offset falls outside the
            /// mapped region.
            ///
            /// # Safety
            ///
            /// The caller must ensure no other map or register aliases the same
            /// memory range. [`DevMem`](::ddevmem::DevMem) does not track claimed regions.
            #[inline(always)]
            pub unsafe fn new(devmem: ::std::sync::Arc<::ddevmem::DevMem>) -> Option<Self> {
                #bounds_checks
                Some(Self { devmem })
            }

            #all_methods
        }

        unsafe impl Sync for #name {}
        unsafe impl Send for #name {}

        #web_impl
    }
}

// ─── Entry point ─────────────────────────────────────────────────────────────

#[proc_macro]
pub fn register_map(input: TokenStream) -> TokenStream {
    let map = syn::parse_macro_input!(input as RegisterMap);
    generate(map).into()
}
