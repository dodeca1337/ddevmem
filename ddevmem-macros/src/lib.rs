//! Proc-macro companion of the [`ddevmem`](https://docs.rs/ddevmem) crate.
//!
//! This crate provides the [`register_map!`] macro. It is an implementation
//! detail: depend on `ddevmem` (with the default `register-map` feature) and
//! use `ddevmem::register_map!` instead of depending on this crate directly.
//!
//! The macro documentation, including the full grammar and examples, lives
//! on the re-export in `ddevmem`.
//!
//! # Pipeline
//!
//! - [`ast`] parses the invocation into a data model;
//! - [`validate`] reports everything provably wrong at expansion time
//!   (misaligned offsets, out-of-range bitfields, name collisions, …) with
//!   precise spans;
//! - [`expand`] generates the map struct and its accessors;
//! - [`web`] (feature `web`) additionally generates a static metadata table
//!   and a `ddevmem::web::RegisterMapInfo` implementation over it.

mod ast;
mod expand;
mod validate;
#[cfg(feature = "web")]
mod web;

use proc_macro::TokenStream;

/// Declares a named register map backed by a `ddevmem::DevMem`.
///
/// See the documentation of the re-export
/// ([`ddevmem::register_map`](https://docs.rs/ddevmem/latest/ddevmem/macro.register_map.html))
/// for syntax and examples.
#[proc_macro]
pub fn register_map(input: TokenStream) -> TokenStream {
    let map = syn::parse_macro_input!(input as ast::RegisterMap);
    if let Err(error) = validate::validate(&map) {
        return error.to_compile_error().into();
    }
    expand::expand(&map).into()
}
