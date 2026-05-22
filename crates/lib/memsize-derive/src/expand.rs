//! Code generation for `#[derive(MemorySize)]`.
//!
//! Expansion is intentionally close to the generated Rust. The macro does not try to model memory
//! accounting itself; it builds a `record_memory_children` body that calls the runtime trait and
//! recorder APIs in the same way the old handwritten impls did.

use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::{format_ident, quote};
use syn::{Data, DataEnum, DataStruct, DeriveInput, Field, Fields, Ident, LitStr, Path, Type};

use crate::{
    attrs::{ContainerAttrs, FieldAttrs, VariantAttrs},
    generics::{add_auto_bounds, add_configured_bounds},
};

/// Expands one derive input into an implementation of `MemorySize`.
pub(crate) fn expand_memory_size(input: DeriveInput) -> syn::Result<TokenStream2> {
    let attrs = ContainerAttrs::parse(&input.attrs)?;
    let crate_path = attrs.crate_path();

    // The trait already owns shallow-size accounting. The derive only generates child traversal,
    // so manual impls and generated impls keep the same top-level accounting shape.
    let mut generics = input.generics.clone();
    let body = if let Some(with) = &attrs.with {
        quote! {
            #with(self, recorder);
        }
    } else if attrs.leaf {
        TokenStream2::new()
    } else {
        let expansion = DataExpansion::from_data(&input.data, &crate_path)?;
        if !attrs.no_auto_bound {
            add_auto_bounds(
                &mut generics,
                &input.generics,
                &crate_path,
                &expansion.bound_types,
            );
        }
        expansion.body
    };

    add_configured_bounds(&mut generics, attrs.bounds);

    let ident = input.ident;
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

    Ok(quote! {
        impl #impl_generics #crate_path::MemorySize for #ident #ty_generics #where_clause {
            fn record_memory_children(&self, recorder: &mut #crate_path::MemoryRecorder) {
                #body
            }
        }
    })
}

/// Generated child traversal plus the field types that need inferred bounds.
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
                "`MemorySize` cannot be derived for unions",
            )),
        }
    }

    /// Generates the straightforward struct case: each recorded field becomes one statement.
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

            let label = FieldLabel::from_field(index, field);
            let access = label.struct_access();
            statements.push(record_field(
                access,
                &attrs,
                &label.default_scope(),
                false,
                crate_path,
            ));
        }

        Ok(Self {
            body: quote! { #(#statements)* },
            bound_types,
        })
    }

    /// Generates the enum match, preserving the recorder-path style of the handwritten impls.
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

            // Existing manual impls usually treat one-field variants as transparent wrappers.
            // Multi-field variants keep field/index scopes so reports stay explainable.
            let recorded_count = variant
                .fields
                .iter()
                .map(|field| FieldAttrs::parse(&field.attrs))
                .collect::<syn::Result<Vec<_>>>()?
                .into_iter()
                .filter(|attrs| !attrs.skip)
                .count();
            let single_field_variant = variant.fields.len() == 1 && recorded_count == 1;

            let VariantArmExpansion {
                pattern,
                body,
                bound_field_types,
            } = expand_variant_arm(&variant.fields, single_field_variant, crate_path)?;

            bound_types.extend(bound_field_types);
            let body = wrap_variant_scope(body, &attrs);

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
fn expand_variant_arm(
    fields: &Fields,
    single_field_variant: bool,
    crate_path: &Path,
) -> syn::Result<VariantArmExpansion> {
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
                let label = ident.to_string();
                statements.push(record_field(
                    quote! { #ident },
                    &attrs,
                    &label,
                    single_field_variant,
                    crate_path,
                ));
            }

            // Skipped named fields still need a valid pattern. `..` keeps the generated arm from
            // depending on fields it does not record.
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

                let binding = format_ident!("__memsize_field_{index}");
                patterns.push(quote! { #binding });
                let label = index.to_string();
                statements.push(record_field(
                    quote! { #binding },
                    &attrs,
                    &label,
                    single_field_variant,
                    crate_path,
                ));
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

/// Adds an optional variant scope around the generated arm body.
fn wrap_variant_scope(body: TokenStream2, attrs: &VariantAttrs) -> TokenStream2 {
    let Some(scope) = &attrs.scope else {
        return body;
    };

    quote! {
        recorder.scope(#scope, |recorder| {
            #body
        });
    }
}

/// Generates the statement that records one field-like value.
fn record_field(
    access: TokenStream2,
    attrs: &FieldAttrs,
    default_scope: &str,
    default_inline: bool,
    crate_path: &Path,
) -> TokenStream2 {
    // A custom recorder owns the field's whole accounting story; otherwise the normal trait walk
    // is enough. Scoping is layered around either version below.
    let record = if let Some(with) = &attrs.with {
        quote! {
            #with(#access, recorder);
        }
    } else {
        quote! {
            #crate_path::MemorySize::record_memory_children(#access, recorder);
        }
    };

    if attrs.inline || (default_inline && attrs.scope.is_none()) {
        return record;
    }

    let scope = attrs
        .scope
        .clone()
        .unwrap_or_else(|| LitStr::new(default_scope, Span::call_site()));
    quote! {
        recorder.scope(#scope, |recorder| {
            #record
        });
    }
}

/// Default label/access information for a struct field.
enum FieldLabel<'a> {
    Named(&'a Ident),
    Unnamed(usize),
}

impl<'a> FieldLabel<'a> {
    /// Builds the label from `syn`'s field shape.
    fn from_field(index: usize, field: &'a Field) -> Self {
        match &field.ident {
            Some(ident) => Self::Named(ident),
            None => Self::Unnamed(index),
        }
    }

    /// Returns the recorder scope used when no explicit scope is configured.
    fn default_scope(&self) -> String {
        match self {
            Self::Named(ident) => ident.to_string(),
            Self::Unnamed(index) => index.to_string(),
        }
    }

    /// Returns Rust tokens that borrow the field from `self`.
    fn struct_access(&self) -> TokenStream2 {
        match self {
            Self::Named(ident) => quote! { &self.#ident },
            Self::Unnamed(index) => {
                let index = syn::Index::from(*index);
                quote! { &self.#index }
            }
        }
    }
}
