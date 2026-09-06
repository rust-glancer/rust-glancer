//! Entry checks share the same expansion as explicit checkpoints inside an operation.

use proc_macro2::TokenStream;
use quote::quote;
use syn::{Expr, Ident, ItemFn, LitStr, Token, parse::Parse, parse::ParseStream};

#[derive(Default)]
struct Arguments {
    label: Option<LitStr>,
    token: Option<Expr>,
}

impl Parse for Arguments {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let mut arguments = Self::default();
        while !input.is_empty() {
            if input.peek(LitStr) {
                if arguments.label.is_some() {
                    return Err(input.error("expected only one cancellation label"));
                }
                arguments.label = Some(input.parse()?);
            } else {
                let name: Ident = input.parse()?;
                if name != "token" {
                    return Err(syn::Error::new_spanned(name, "expected `token = argument`"));
                }
                if arguments.token.is_some() {
                    return Err(syn::Error::new_spanned(name, "expected only one token"));
                }
                input.parse::<Token![=]>()?;
                arguments.token = Some(input.parse()?);
            }
            if !input.is_empty() {
                input.parse::<Token![,]>()?;
            }
        }
        Ok(arguments)
    }
}

pub(crate) fn expand(args: TokenStream, input: TokenStream) -> syn::Result<TokenStream> {
    let arguments: Arguments = syn::parse2(args)?;
    let mut function: ItemFn = syn::parse2(input)?;
    if let Some(constness) = function.sig.constness {
        return Err(syn::Error::new_spanned(
            constness,
            "cancellation cannot be checked in a const function",
        ));
    }

    let source = if let Some(token) = arguments.token {
        quote!(#token)
    } else if function.sig.receiver().is_some() {
        quote!(self)
    } else {
        return Err(syn::Error::new_spanned(
            &function.sig,
            "a function without a receiver needs `token = argument`",
        ));
    };
    let label = if let Some(label) = arguments.label {
        quote!(#label)
    } else {
        let name = &function.sig.ident;
        quote!(concat!(module_path!(), "::", stringify!(#name)))
    };

    // Insert a statement rather than wrapping the body. In particular, an async function should
    // check when its body starts running, and existing early returns must keep their meaning.
    function.block.stmts.insert(
        0,
        syn::parse_quote!(::rg_std::check_cancel!(#source, #label);),
    );
    Ok(quote!(#function))
}
