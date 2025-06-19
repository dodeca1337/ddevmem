mod kw;
mod register;

use proc_macro::TokenStream;

#[proc_macro]
pub fn register_map(item: TokenStream) -> TokenStream {
    item
}
