//! Data model of a `register_map!` invocation and its parser.
//!
//! The grammar (doc comments elided):
//!
//! ```text
//! $vis unsafe map $Name ($bus)? {
//!     $offset => $access $name : $ty ( `{` $bitfield,* `}` )? ,
//!     ...
//! }
//!
//! bitfield := $name : $bits ( as bool | as $int | as enum $Name { $Variant = $value,* } )?
//! bits     := $bit | $lo..=$hi | $lo..$hi          -- literal or parenthesized const expr
//! ```

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
        if let Type::Path(path) = ty {
            if path.qself.is_none() {
                if let Some(ident) = path.path.get_ident() {
                    return Self::from_ident(ident.clone());
                }
            }
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

/// A named bit range within a register.
pub struct Bitfield {
    pub attrs: Vec<Attribute>,
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
    /// `as enum Name { ... }` — a generated enum.
    Enum(EnumDef),
}

pub struct EnumDef {
    pub name: Ident,
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
        let entries = Punctuated::<RegisterEntry, Token![,]>::parse_terminated(&content)?;

        Ok(RegisterMap {
            attrs,
            vis,
            name,
            bus,
            entries: entries.into_iter().collect(),
        })
    }
}

impl Parse for RegisterEntry {
    fn parse(input: ParseStream) -> Result<Self> {
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

        let bitfields = if input.peek(syn::token::Brace) {
            let content;
            braced!(content in input);
            Punctuated::<Bitfield, Token![,]>::parse_terminated(&content)?
                .into_iter()
                .collect()
        } else {
            Vec::new()
        };

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

impl Parse for Bitfield {
    fn parse(input: ParseStream) -> Result<Self> {
        let attrs = input.call(Attribute::parse_outer)?;
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
                input.parse::<Token![enum]>()?;
                let enum_name: Ident = input.parse()?;
                let content;
                braced!(content in input);
                let variants = Punctuated::<EnumVariant, Token![,]>::parse_terminated(&content)?;
                FieldType::Enum(EnumDef {
                    name: enum_name,
                    variants: variants.into_iter().collect(),
                })
            } else {
                let ident: Ident = input.parse()?;
                if ident == "bool" {
                    FieldType::Bool
                } else {
                    FieldType::Int(IntType::from_ident(ident)?)
                }
            }
        } else {
            FieldType::Raw
        };

        Ok(Bitfield {
            attrs,
            name,
            lo,
            hi,
            ty,
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
