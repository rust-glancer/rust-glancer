//! `#[derive(MemorySize)]` implementation.

use proc_macro2::TokenStream as TokenStream2;
use syn::DeriveInput;

mod attrs;
mod expand;

pub(crate) fn derive(input: DeriveInput) -> syn::Result<TokenStream2> {
    expand::expand_memory_size(input)
}
