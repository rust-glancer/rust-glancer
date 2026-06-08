//! Procedural macro implementation for `rg_std`.
//!
//! This crate is re-exported by `rg_std`, so normal call sites should write
//! `#[derive(MemorySize)]` rather than depending on this crate directly.
//! Keeping the implementation separate avoids making the runtime `rg_std` crate a proc-macro
//! crate, while still letting users opt into the derive through a regular feature.

use proc_macro::TokenStream;
use syn::{DeriveInput, parse_macro_input};

mod attrs;
mod expand;
mod generics;

/// Derives `MemorySize` by generating `record_memory_children`.
///
/// The generated implementation follows the existing `rg_std` accounting convention:
/// `MemorySize::record_memory_size` records the shallow size, while the derive only walks owned
/// child values. Struct fields are recorded under their field names, tuple fields under their
/// numeric indexes, and one-field enum variants are treated as transparent wrappers by default.
///
/// Supported container attributes:
/// - `#[memsize(leaf)]`: generate a no-op child traversal for ids, flags, and marker values.
/// - `#[memsize(crate_path = "::some_path")]`: use a different path to the `rg_std` crate.
/// - `#[memsize(with = "record_type")]`: call a custom `fn(&Self, &mut MemoryRecorder)`.
/// - `#[memsize(no_auto_bound)]`: skip generated `T: MemorySize` bounds.
/// - `#[memsize(bound = "T: SomeBound")]`: add an explicit where-clause predicate.
///
/// Supported field attributes:
/// - `#[memsize(skip)]`: omit the field and do not require a `MemorySize` bound for it.
/// - `#[memsize(inline)]`: record the field without adding a scope.
/// - `#[memsize(scope = "label")]`: override the default recorder path label.
/// - `#[memsize(with = "record_field")]`: call a custom `fn(&FieldTy, &mut MemoryRecorder)`.
///
/// Supported variant attributes:
/// - `#[memsize(skip)]`: omit every child for that variant.
/// - `#[memsize(scope = "label")]`: wrap the variant's children in one extra scope.
///
/// ```ignore
/// use rg_std::MemorySize;
///
/// #[derive(MemorySize)]
/// struct Package {
///     name: String,
///     #[memsize(scope = "roots")]
///     target_roots: Vec<String>,
/// }
///
/// #[derive(MemorySize)]
/// #[memsize(leaf)]
/// struct PackageSlot(usize);
/// ```
#[proc_macro_derive(MemorySize, attributes(memsize))]
pub fn derive_memory_size(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);

    expand::expand_memory_size(input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}
