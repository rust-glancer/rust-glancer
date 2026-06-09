//! Code generation for `#[derive(Shrink)]`.

use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::{format_ident, quote};
use syn::{Data, DataEnum, DataStruct, DeriveInput, Field, Fields, Ident, Path, Type};

use crate::generics::{add_auto_bounds, add_configured_bounds};

use super::attrs::{ContainerAttrs, FieldAttrs, VariantAttrs};

/// Expands one derive input into an implementation of `Shrink`.
pub(crate) fn expand_shrink(input: DeriveInput) -> syn::Result<TokenStream2> {
    let attrs = ContainerAttrs::parse(&input.attrs)?;
    let crate_path = attrs.crate_path();

    let mut generics = input.generics.clone();
    let body = if attrs.leaf {
        TokenStream2::new()
    } else {
        let expansion = DataExpansion::from_data(&input.data, &crate_path)?;
        if !attrs.no_auto_bound {
            add_auto_bounds(
                &mut generics,
                &input.generics,
                &crate_path,
                &Ident::new("Shrink", Span::call_site()),
                &expansion.bound_types,
            );
        }
        expansion.body
    };

    add_configured_bounds(&mut generics, attrs.bounds);

    let ident = input.ident;
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

    Ok(quote! {
        impl #impl_generics #crate_path::Shrink for #ident #ty_generics #where_clause {
            fn shrink_to_fit(&mut self) {
                #body
            }
        }
    })
}

/// Generated compaction body plus the field types that need inferred bounds.
struct DataExpansion {
    body: TokenStream2,
    bound_types: Vec<Type>,
}

impl DataExpansion {
    /// Dispatches between Rust data forms. Unions are rejected because there is no safe field walk.
    fn from_data(data: &Data, crate_path: &Path) -> syn::Result<Self> {
        match data {
            Data::Struct(data) => Self::from_struct(data, crate_path),
            Data::Enum(data) => Self::from_enum(data, crate_path),
            Data::Union(data) => Err(syn::Error::new_spanned(
                data.union_token,
                "`Shrink` cannot be derived for unions",
            )),
        }
    }

    /// Generates the straightforward struct case: each compacted field becomes one statement.
    fn from_struct(data: &DataStruct, crate_path: &Path) -> syn::Result<Self> {
        let mut bound_types = Vec::new();
        let mut statements = Vec::new();

        for (index, field) in data.fields.iter().enumerate() {
            let attrs = FieldAttrs::parse(&field.attrs)?;
            if attrs.skip {
                continue;
            }

            if attrs.needs_auto_bound() {
                bound_types.push(field.ty.clone());
            }

            let access = FieldAccess::from_field(index, field).struct_access();
            statements.push(shrink_field(access, &attrs, crate_path));
        }

        Ok(Self {
            body: quote! { #(#statements)* },
            bound_types,
        })
    }

    /// Generates the enum match, compacting only the fields present in the active variant.
    fn from_enum(data: &DataEnum, crate_path: &Path) -> syn::Result<Self> {
        let mut bound_types = Vec::new();
        let mut arms = Vec::new();

        for variant in &data.variants {
            let attrs = VariantAttrs::parse(&variant.attrs)?;
            let variant_ident = &variant.ident;

            if attrs.skip {
                arms.push(skipped_variant_arm(variant_ident, &variant.fields));
                continue;
            }

            let VariantArmExpansion {
                pattern,
                body,
                bound_field_types,
            } = expand_variant_arm(&variant.fields, crate_path)?;

            bound_types.extend(bound_field_types);
            arms.push(quote! {
                Self::#variant_ident #pattern => {
                    #body
                }
            });
        }

        Ok(Self {
            body: quote! {
                match self {
                    #(#arms),*
                }
            },
            bound_types,
        })
    }
}

/// The generated pieces for one enum variant arm.
struct VariantArmExpansion {
    pattern: TokenStream2,
    body: TokenStream2,
    bound_field_types: Vec<Type>,
}

/// Builds the pattern and body for one enum variant.
fn expand_variant_arm(fields: &Fields, crate_path: &Path) -> syn::Result<VariantArmExpansion> {
    match fields {
        Fields::Unit => Ok(VariantArmExpansion {
            pattern: TokenStream2::new(),
            body: TokenStream2::new(),
            bound_field_types: Vec::new(),
        }),
        Fields::Named(fields) => {
            let mut patterns = Vec::new();
            let mut statements = Vec::new();
            let mut bound_field_types = Vec::new();
            let mut omitted_field = false;

            for field in &fields.named {
                let ident = field
                    .ident
                    .as_ref()
                    .expect("named fields always have identifiers");
                let attrs = FieldAttrs::parse(&field.attrs)?;

                if attrs.skip {
                    omitted_field = true;
                    continue;
                }

                if attrs.needs_auto_bound() {
                    bound_field_types.push(field.ty.clone());
                }

                patterns.push(quote! { #ident });
                statements.push(shrink_field(quote! { #ident }, &attrs, crate_path));
            }

            // Skipped named fields still need a valid pattern. `..` keeps the generated arm from
            // depending on fields it does not compact.
            let pattern = if patterns.is_empty() {
                quote! { { .. } }
            } else if omitted_field {
                quote! { { #(#patterns),*, .. } }
            } else {
                quote! { { #(#patterns),* } }
            };

            Ok(VariantArmExpansion {
                pattern,
                body: quote! { #(#statements)* },
                bound_field_types,
            })
        }
        Fields::Unnamed(fields) => {
            let mut patterns = Vec::new();
            let mut statements = Vec::new();
            let mut bound_field_types = Vec::new();

            for (index, field) in fields.unnamed.iter().enumerate() {
                let attrs = FieldAttrs::parse(&field.attrs)?;

                if attrs.skip {
                    patterns.push(quote! { _ });
                    continue;
                }

                if attrs.needs_auto_bound() {
                    bound_field_types.push(field.ty.clone());
                }

                let binding = format_ident!("__shrink_field_{index}");
                patterns.push(quote! { #binding });
                statements.push(shrink_field(quote! { #binding }, &attrs, crate_path));
            }

            Ok(VariantArmExpansion {
                pattern: quote! { ( #(#patterns),* ) },
                body: quote! { #(#statements)* },
                bound_field_types,
            })
        }
    }
}

/// Generates a no-op arm for a skipped variant while still matching its shape.
fn skipped_variant_arm(variant_ident: &Ident, fields: &Fields) -> TokenStream2 {
    match fields {
        Fields::Unit => quote! { Self::#variant_ident => {} },
        Fields::Named(_) => quote! { Self::#variant_ident { .. } => {} },
        Fields::Unnamed(_) => quote! { Self::#variant_ident(..) => {} },
    }
}

/// Generates the statement that compacts one field-like value.
fn shrink_field(access: TokenStream2, attrs: &FieldAttrs, crate_path: &Path) -> TokenStream2 {
    if let Some(with) = &attrs.with {
        quote! {
            #with(#access);
        }
    } else {
        quote! {
            #crate_path::Shrink::shrink_to_fit(#access);
        }
    }
}

/// Access information for a struct field.
enum FieldAccess<'a> {
    Named(&'a Ident),
    Unnamed(usize),
}

impl<'a> FieldAccess<'a> {
    /// Builds the access from `syn`'s field shape.
    fn from_field(index: usize, field: &'a Field) -> Self {
        match &field.ident {
            Some(ident) => Self::Named(ident),
            None => Self::Unnamed(index),
        }
    }

    /// Returns Rust tokens that mutably borrow the field from `self`.
    fn struct_access(&self) -> TokenStream2 {
        match self {
            Self::Named(ident) => quote! { &mut self.#ident },
            Self::Unnamed(index) => {
                let index = syn::Index::from(*index);
                quote! { &mut self.#index }
            }
        }
    }
}
