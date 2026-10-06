//! Data model of a `register_map!` invocation and its parser.
//!
//! The grammar (doc comments elided):
//!
//! ```text
//! $vis unsafe map $Name ($bus)? {
//!     $offset => $access $name : $ty ( `{` $bitfield,* `}` )? ,
//!     enum $Name { $Variant = $value,* }                -- in any position
//!     ...
//! }
//!
//! bitfield := $name : $bits ( as bool | as $int | as enum $Name { ... } | as $Type )?
//! bits     := $bit | $lo..=$hi | $lo..$hi          -- literal or parenthesized const expr
//! ```
//!
//! `as $Type` names either an enum declared in the same map (inline or as an
//! item) or any type implementing `ddevmem::FieldValue`; references are
//! resolved once the whole body is parsed, so declaration order is free.

use proc_macro2::Span;
use quote::ToTokens;
use syn::{
    braced, parenthesized,
    parse::{Parse, ParseStream},
    punctuated::Punctuated,
    spanned::Spanned,
    Attribute, Expr, Ident, Result, Token, Type, Visibility,
};

/// One of the unsigned integer types a register or bus may use.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum IntKind {
    U8,
    U16,
    U32,
    U64,
    Usize,
}

impl IntKind {
    /// Width in bits, `None` for the target-dependent `usize`.
    pub fn width_bits(self) -> Option<u32> {
        match self {
            IntKind::U8 => Some(8),
            IntKind::U16 => Some(16),
            IntKind::U32 => Some(32),
            IntKind::U64 => Some(64),
            IntKind::Usize => None,
        }
    }

    /// Size in bytes, `None` for `usize`.
    pub fn size_bytes(self) -> Option<u64> {
        self.width_bits().map(|bits| u64::from(bits) / 8)
    }
}

/// An unsigned integer type together with the ident it was written as
/// (kept for spans and verbatim re-emission).
#[derive(Clone)]
pub struct IntType {
    pub kind: IntKind,
    pub ident: Ident,
}

impl IntType {
    pub fn usize_default() -> Self {
        Self {
            kind: IntKind::Usize,
            ident: Ident::new("usize", Span::call_site()),
        }
    }

    fn from_ident(ident: Ident) -> Result<Self> {
        let kind = match ident.to_string().as_str() {
            "u8" => IntKind::U8,
            "u16" => IntKind::U16,
            "u32" => IntKind::U32,
            "u64" => IntKind::U64,
            "usize" => IntKind::Usize,
            _ => {
                return Err(syn::Error::new(
                    ident.span(),
                    "expected an unsigned integer type: u8, u16, u32, u64, or usize",
                ))
            }
        };
        Ok(Self { kind, ident })
    }

    fn from_type(ty: &Type) -> Result<Self> {
        if let Some(ident) = bare_ident(ty) {
            return Self::from_ident(ident.clone());
        }
        Err(syn::Error::new_spanned(
            ty,
            "expected an unsigned integer type: u8, u16, u32, u64, or usize",
        ))
    }

    pub fn span(&self) -> Span {
        self.ident.span()
    }
}

impl ToTokens for IntType {
    fn to_tokens(&self, tokens: &mut proc_macro2::TokenStream) {
        self.ident.to_tokens(tokens);
    }
}

/// The identifier of a type written as a single bare name (`u32`, `Mode`).
fn bare_ident(ty: &Type) -> Option<&Ident> {
    match ty {
        Type::Path(path) if path.qself.is_none() => path.path.get_ident(),
        _ => None,
    }
}

/// A const expression whose value is known at macro-expansion time when it
/// was written as an integer literal. Non-literal expressions still expand
/// (parenthesized), but validation for them is deferred to `const` evaluation.
#[derive(Clone)]
pub struct ConstExpr {
    pub expr: Expr,
    pub value: Option<u64>,
}

/// Extracts the value of an integer-literal expression, looking through
/// parentheses and macro groups.
fn literal_value(expr: &Expr) -> Option<u64> {
    match expr {
        Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Int(lit),
            ..
        }) => lit.base10_parse::<u64>().ok(),
        Expr::Paren(paren) => literal_value(&paren.expr),
        Expr::Group(group) => literal_value(&group.expr),
        _ => None,
    }
}

impl ConstExpr {
    pub fn from_expr(expr: Expr) -> Self {
        let value = literal_value(&expr);
        Self { expr, value }
    }

    pub fn from_u64(value: u64, span: Span) -> Self {
        let lit = syn::LitInt::new(&value.to_string(), span);
        Self {
            expr: syn::parse_quote!(#lit),
            value: Some(value),
        }
    }

    /// Parses a bit position: an integer literal or a parenthesized const
    /// expression. Restricting to these two forms keeps `lo..=hi` ranges
    /// unambiguous (a bare `Expr` would greedily consume the range).
    pub fn parse_bit(input: ParseStream) -> Result<Self> {
        if input.peek(syn::token::Paren) {
            let content;
            parenthesized!(content in input);
            let inner: Expr = content.parse()?;
            Ok(Self::from_expr(syn::parse_quote!((#inner))))
        } else {
            let lit: syn::LitInt = input.parse()?;
            let expr = Expr::Lit(syn::ExprLit {
                attrs: Vec::new(),
                lit: syn::Lit::Int(lit),
            });
            Ok(Self::from_expr(expr))
        }
    }

    pub fn span(&self) -> Span {
        self.expr.span()
    }
}

impl ToTokens for ConstExpr {
    fn to_tokens(&self, tokens: &mut proc_macro2::TokenStream) {
        if self.value.is_some() {
            // Literals are emitted verbatim.
            self.expr.to_tokens(tokens);
        } else {
            let expr = &self.expr;
            tokens.extend(quote::quote!((#expr)));
        }
    }
}

/// Access kind of a register: `rw`, `ro`, or `wo`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Access {
    Rw,
    Ro,
    Wo,
}

impl Access {
    pub fn can_read(self) -> bool {
        matches!(self, Access::Rw | Access::Ro)
    }

    pub fn can_write(self) -> bool {
        matches!(self, Access::Rw | Access::Wo)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Access::Rw => "rw",
            Access::Ro => "ro",
            Access::Wo => "wo",
        }
    }
}

impl Parse for Access {
    fn parse(input: ParseStream) -> Result<Self> {
        let ident: Ident = input.parse()?;
        match ident.to_string().as_str() {
            "rw" => Ok(Access::Rw),
            "ro" => Ok(Access::Ro),
            "wo" => Ok(Access::Wo),
            _ => Err(syn::Error::new(
                ident.span(),
                "expected `rw`, `ro`, or `wo`",
            )),
        }
    }
}

/// The whole `register_map!` invocation.
pub struct RegisterMap {
    pub attrs: Vec<Attribute>,
    pub vis: Visibility,
    pub name: Ident,
    pub bus: IntType,
    pub entries: Vec<RegisterEntry>,
    /// Every enum the map declares — `enum` items and inline `as enum`
    /// declarations alike — in source order.
    pub enums: Vec<EnumDef>,
}

/// A single `offset => access name: ty { bitfields }` entry.
pub struct RegisterEntry {
    pub offset: ConstExpr,
    pub attrs: Vec<Attribute>,
    pub access: Access,
    pub name: Ident,
    /// Element type for arrays, the register type otherwise.
    pub ty: IntType,
    /// `Some(len)` when the entry was declared as `[ty; len]`.
    pub array_len: Option<ConstExpr>,
    pub bitfields: Vec<Bitfield>,
}

impl RegisterEntry {
    pub fn is_array(&self) -> bool {
        self.array_len.is_some()
    }
}

/// Access modifier on an individual bitfield, narrowing the register's own.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FieldAccess {
    /// No modifier: the field follows the register's access kind.
    Inherit,
    /// `ro` — readable but never written; no setter is generated.
    Ro,
    /// `wo` — writable but never read; no getter is generated.
    Wo,
    /// `w1c` — write-1-to-clear; `clear_*` replaces `set_*`.
    W1c,
}

impl FieldAccess {
    pub fn can_read(self, register: Access) -> bool {
        register.can_read() && self != FieldAccess::Wo
    }

    pub fn can_write(self, register: Access) -> bool {
        register.can_write() && self != FieldAccess::Ro
    }

    /// Whether the field's bits must be written as zero by any
    /// read-modify-write that is not deliberately targeting them.
    ///
    /// Writing back a `w1c` bit that happens to read as 1 would acknowledge
    /// the flag by accident; writing back a `wo` bit would re-trigger it.
    pub fn forced_zero(self) -> bool {
        matches!(self, FieldAccess::Wo | FieldAccess::W1c)
    }

    /// Name of the effective access kind, for the web UI metadata.
    #[cfg_attr(not(feature = "web"), allow(dead_code))]
    pub fn as_str(self, register: Access) -> &'static str {
        match self {
            FieldAccess::Inherit => register.as_str(),
            FieldAccess::Ro => "ro",
            FieldAccess::Wo => "wo",
            FieldAccess::W1c => "w1c",
        }
    }
}

/// A named bit range within a register.
pub struct Bitfield {
    pub attrs: Vec<Attribute>,
    pub access: FieldAccess,
    pub name: Ident,
    /// Low bit index, inclusive.
    pub lo: ConstExpr,
    /// High bit index, inclusive (`lo..hi` input is normalized to `hi - 1`).
    pub hi: ConstExpr,
    pub ty: FieldType,
}

/// The getter/setter type of a bitfield.
pub enum FieldType {
    /// No `as` suffix: the register type.
    Raw,
    /// `as bool` — single-bit flag.
    Bool,
    /// `as u8` etc. — a (possibly narrower) integer view.
    Int(IntType),
    /// An enum generated by this map: declared right here
    /// (`as enum Name { ... }`) or named (`as Name`).
    Enum(EnumUse),
    /// `as Type` for any other type, converted through `ddevmem::FieldValue`.
    Custom(Type),
}

/// A bitfield's use of one of the map's enums.
pub struct EnumUse {
    /// Index into [`RegisterMap::enums`].
    pub index: usize,
    /// The enum is declared on this very field rather than referenced.
    pub inline: bool,
    /// The enum's name as written on this field.
    pub span: Span,
}

pub struct EnumDef {
    pub attrs: Vec<Attribute>,
    pub name: Ident,
    /// The type taken by `from_raw` and returned by `to_raw`: the register's
    /// type for an inline enum, the bus type for an `enum` item.
    pub raw: IntType,
    /// Declared as an `enum` item of the map body rather than inline.
    pub standalone: bool,
    pub variants: Vec<EnumVariant>,
}

pub struct EnumVariant {
    pub attrs: Vec<Attribute>,
    pub name: Ident,
    pub value: ConstExpr,
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

        let bus = if input.peek(syn::token::Paren) {
            let content;
            parenthesized!(content in input);
            let ty: Type = content.parse()?;
            if !content.is_empty() {
                return Err(content.error("unexpected token after bus type"));
            }
            IntType::from_type(&ty)?
        } else {
            IntType::usize_default()
        };

        let content;
        braced!(content in input);
        let mut entries = Vec::new();
        let mut enums = Vec::new();
        while !content.is_empty() {
            if peek_enum_item(&content)? {
                let attrs = content.call(Attribute::parse_outer)?;
                enums.push(EnumDef::parse(&content, attrs, bus.clone(), true)?);
                // Like any Rust item, an enum needs no separator; a comma is
                // tolerated all the same.
                if content.peek(Token![,]) {
                    content.parse::<Token![,]>()?;
                }
                continue;
            }
            entries.push(RegisterEntry::parse(&content, &mut enums)?);
            if !content.is_empty() {
                content.parse::<Token![,]>()?;
            }
        }
        resolve_enum_refs(&mut entries, &enums);

        Ok(RegisterMap {
            attrs,
            vis,
            name,
            bus,
            entries,
            enums,
        })
    }
}

/// Whether the map body continues with an `enum` item. A visibility in front
/// of one gets a pointed error instead of a confusing "expected expression".
fn peek_enum_item(input: ParseStream) -> Result<bool> {
    let fork = input.fork();
    fork.call(Attribute::parse_outer)?;
    if fork.peek(Token![pub]) {
        let vis: Visibility = fork.parse()?;
        if fork.peek(Token![enum]) {
            return Err(syn::Error::new_spanned(
                vis,
                "an enum declared in a map takes the map's visibility; remove this",
            ));
        }
    }
    Ok(fork.peek(Token![enum]))
}

/// Turns every `as Name` that names one of the map's enums into a reference
/// to it. Runs after the whole body is parsed, so an enum may be used before
/// its declaration.
fn resolve_enum_refs(entries: &mut [RegisterEntry], enums: &[EnumDef]) {
    for bf in entries.iter_mut().flat_map(|entry| &mut entry.bitfields) {
        let FieldType::Custom(ty) = &bf.ty else {
            continue;
        };
        let Some(ident) = bare_ident(ty) else {
            continue;
        };
        if let Some(index) = enums.iter().position(|def| def.name == *ident) {
            let span = ident.span();
            bf.ty = FieldType::Enum(EnumUse {
                index,
                inline: false,
                span,
            });
        }
    }
}

impl RegisterEntry {
    /// Parses one entry; inline enums it declares are appended to `enums`.
    fn parse(input: ParseStream, enums: &mut Vec<EnumDef>) -> Result<Self> {
        let offset = ConstExpr::from_expr(input.parse()?);
        input.parse::<Token![=>]>()?;

        let attrs = input.call(Attribute::parse_outer)?;
        let access: Access = input.parse()?;
        let name: Ident = input.parse()?;
        input.parse::<Token![:]>()?;

        let (ty, array_len) = match input.parse::<Type>()? {
            Type::Array(arr) => (
                IntType::from_type(&arr.elem)?,
                Some(ConstExpr::from_expr(arr.len)),
            ),
            other => (IntType::from_type(&other)?, None),
        };

        let mut bitfields = Vec::new();
        if input.peek(syn::token::Brace) {
            let content;
            braced!(content in input);
            while !content.is_empty() {
                bitfields.push(Bitfield::parse(&content, &ty, enums)?);
                if !content.is_empty() {
                    content.parse::<Token![,]>()?;
                }
            }
        }

        Ok(RegisterEntry {
            offset,
            attrs,
            access,
            name,
            ty,
            array_len,
            bitfields,
        })
    }
}

impl Bitfield {
    /// Parses one bitfield of a register of type `reg_ty`; an inline enum it
    /// declares is appended to `enums`.
    fn parse(input: ParseStream, reg_ty: &IntType, enums: &mut Vec<EnumDef>) -> Result<Self> {
        let attrs = input.call(Attribute::parse_outer)?;

        // An access modifier is only a modifier when a second identifier
        // follows it — `ro: 0` declares a field genuinely named `ro`.
        let access = if input.peek(Ident) && input.peek2(Ident) {
            let ident: Ident = input.parse()?;
            match ident.to_string().as_str() {
                "ro" => FieldAccess::Ro,
                "wo" => FieldAccess::Wo,
                "w1c" => FieldAccess::W1c,
                _ => {
                    return Err(syn::Error::new(
                        ident.span(),
                        "expected a field name, or one of the access modifiers \
                         `ro`, `wo`, `w1c`",
                    ))
                }
            }
        } else {
            FieldAccess::Inherit
        };

        let name: Ident = input.parse()?;
        input.parse::<Token![:]>()?;

        let lo = ConstExpr::parse_bit(input)?;

        let hi = if input.peek(Token![..=]) {
            input.parse::<Token![..=]>()?;
            ConstExpr::parse_bit(input)?
        } else if input.peek(Token![..]) {
            input.parse::<Token![..]>()?;
            let end = ConstExpr::parse_bit(input)?;
            // Exclusive upper bound: normalize to an inclusive `hi`.
            match end.value {
                Some(0) => {
                    return Err(syn::Error::new(
                        end.span(),
                        "exclusive bit range must end above 0",
                    ))
                }
                Some(v) => ConstExpr::from_u64(v - 1, end.span()),
                None => {
                    let expr = &end.expr;
                    ConstExpr::from_expr(syn::parse_quote!((#expr) - 1))
                }
            }
        } else {
            // Single bit.
            lo.clone()
        };

        let ty = if input.peek(Token![as]) {
            input.parse::<Token![as]>()?;
            if input.peek(Token![enum]) {
                let def = EnumDef::parse(input, Vec::new(), reg_ty.clone(), false)?;
                let span = def.name.span();
                enums.push(def);
                FieldType::Enum(EnumUse {
                    index: enums.len() - 1,
                    inline: true,
                    span,
                })
            } else {
                FieldType::from_type(input.parse()?)?
            }
        } else {
            FieldType::Raw
        };

        Ok(Bitfield {
            attrs,
            access,
            name,
            lo,
            hi,
            ty,
        })
    }
}

impl FieldType {
    /// Classifies the type after `as` (other than an inline `enum`). Names of
    /// enums declared in the map come out as `Custom` here and are turned
    /// into `Enum` references by [`resolve_enum_refs`].
    fn from_type(ty: Type) -> Result<Self> {
        if let Some(ident) = bare_ident(&ty) {
            match ident.to_string().as_str() {
                "bool" => return Ok(FieldType::Bool),
                "u8" | "u16" | "u32" | "u64" | "usize" => {
                    return IntType::from_ident(ident.clone()).map(FieldType::Int)
                }
                "u128" | "i8" | "i16" | "i32" | "i64" | "i128" | "isize" | "f32" | "f64"
                | "char" | "str" => {
                    return Err(syn::Error::new(
                        ident.span(),
                        "expected `bool`, an unsigned integer type (u8, u16, u32, u64, \
                         usize), an enum, or a type implementing `ddevmem::FieldValue`",
                    ))
                }
                _ => {}
            }
        }
        Ok(FieldType::Custom(ty))
    }
}

impl EnumDef {
    /// Parses `enum Name { Variant = value, ... }`.
    fn parse(
        input: ParseStream,
        attrs: Vec<Attribute>,
        raw: IntType,
        standalone: bool,
    ) -> Result<Self> {
        input.parse::<Token![enum]>()?;
        let name: Ident = input.parse()?;
        let content;
        braced!(content in input);
        let variants = Punctuated::<EnumVariant, Token![,]>::parse_terminated(&content)?;
        Ok(EnumDef {
            attrs,
            name,
            raw,
            standalone,
            variants: variants.into_iter().collect(),
        })
    }
}

impl Parse for EnumVariant {
    fn parse(input: ParseStream) -> Result<Self> {
        let attrs = input.call(Attribute::parse_outer)?;
        let name: Ident = input.parse()?;
        input.parse::<Token![=]>()?;
        let value = ConstExpr::from_expr(input.parse()?);
        Ok(EnumVariant { attrs, name, value })
    }
}

/// Extracts the plain-text content of `///` doc comments, joined by `\n`.
pub fn doc_string(attrs: &[Attribute]) -> String {
    let mut doc = String::new();
    for attr in attrs {
        if !attr.path().is_ident("doc") {
            continue;
        }
        if let syn::Meta::NameValue(nv) = &attr.meta {
            if let Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Str(s),
                ..
            }) = &nv.value
            {
                if !doc.is_empty() {
                    doc.push('\n');
                }
                doc.push_str(s.value().trim());
            }
        }
    }
    doc
}
