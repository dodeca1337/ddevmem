use crate::kw;
use proc_macro2::TokenStream as TokenStream2;
use syn::{
    Attribute, Ident, LitInt, Result, Token, braced, bracketed, parse::Parse,
    punctuated::Punctuated, token,
};

pub struct Register {
    pub attrs: Vec<Attribute>,
    pub register_bits: RegisterBits,
    pub access: RegisterAccess,
    pub name: Ident,
    pub colon_token: Token![:],
    pub register_type: RegisterType,
}

impl Parse for Register {
    fn parse(input: syn::parse::ParseStream) -> Result<Self> {
        let attrs = input.call(Attribute::parse_outer)?;
        let register_bits = input.parse()?;
        let access = input.parse()?;
        let name = input.parse()?;
        let colon_token = input.parse()?;
        let register_type = input.parse()?;

        Ok(Register {
            attrs,
            register_bits,
            access,
            name,
            colon_token,
            register_type,
        })
    }
}

pub enum RegisterBits {
    Single {
        bracket_token: token::Bracket,
        bit: LitInt,
    },
    Range {
        bracket_token: token::Bracket,
        end: LitInt,
        colon_token: Token![:],
        start: LitInt,
    },
}

impl Parse for RegisterBits {
    fn parse(input: syn::parse::ParseStream) -> Result<Self> {
        let content;
        let bracket_token = bracketed!(content in input);
        if content.peek(LitInt) && !content.peek(Token![:]) {
            let bit: LitInt = content.parse()?;
            return Ok(RegisterBits::Single { bracket_token, bit });
        }

        let end: LitInt = content.parse()?;
        let colon_token: Token![:] = content.parse()?;
        let start: LitInt = content.parse()?;
        Ok(RegisterBits::Range {
            bracket_token,
            start,
            colon_token,
            end,
        })
    }
}

pub enum RegisterAccess {
    Rw(kw::rw),
    Ro(kw::ro),
}

impl Parse for RegisterAccess {
    fn parse(input: syn::parse::ParseStream) -> Result<Self> {
        if input.peek(kw::rw) {
            let rw: kw::rw = input.parse()?;
            Ok(RegisterAccess::Rw(rw))
        } else if input.peek(kw::ro) {
            let ro: kw::ro = input.parse()?;
            Ok(RegisterAccess::Ro(ro))
        } else {
            Err(input.error("expected 'rw' or 'ro'"))
        }
    }
}

pub enum RegisterType {
    Int(kw::int),
    Uint(kw::uint),
    Enum(RegisterEnumType),
}

impl Parse for RegisterType {
    fn parse(input: syn::parse::ParseStream) -> Result<Self> {
        if input.peek(kw::int) {
            let int: kw::int = input.parse()?;
            return Ok(RegisterType::Int(int));
        } else if input.peek(kw::uint) {
            let uint: kw::uint = input.parse()?;
            return Ok(RegisterType::Uint(uint));
        } else if input.peek(Token![enum]) {
            let enum_type: RegisterEnumType = input.parse()?;
            return Ok(RegisterType::Enum(enum_type));
        }
        Err(input.error("expected 'int', 'uint', or 'enum'"))
    }
}

pub struct RegisterEnumType {
    pub enum_token: Token![enum],
    pub brace_token: token::Brace,
    pub variants: Punctuated<RegisterEnumTypeVariant, Token![,]>,
}

impl Parse for RegisterEnumType {
    fn parse(input: syn::parse::ParseStream) -> Result<Self> {
        let enum_token: Token![enum] = input.parse()?;
        let content;
        let brace_token = braced!(content in input);
        let variants = content.parse_terminated(RegisterEnumTypeVariant::parse, Token![,])?;

        Ok(RegisterEnumType {
            enum_token,
            brace_token,
            variants,
        })
    }
}

pub struct RegisterEnumTypeVariant {
    pub attrs: Vec<Attribute>,
    pub discriminant: LitInt,
    pub eq_token: Token![=],
    pub name: Ident,
}

impl Parse for RegisterEnumTypeVariant {
    fn parse(input: syn::parse::ParseStream) -> Result<Self> {
        let attrs = input.call(Attribute::parse_outer)?;
        let discriminant: LitInt = input.parse()?;
        let eq_token: Token![=] = input.parse()?;
        let name: Ident = input.parse()?;

        Ok(Self {
            attrs,
            discriminant,
            eq_token,
            name,
        })
    }
}
