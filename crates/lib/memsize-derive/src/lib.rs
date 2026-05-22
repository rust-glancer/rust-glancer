//! Procedural macro implementation for `rg_memsize`.
//!
//! This crate is re-exported by `rg_memsize`, so normal call sites should write
//! `#[derive(rg_memsize::MemorySize)]` rather than depending on this crate directly.
//! Keeping the implementation separate avoids making the runtime `rg_memsize` crate a proc-macro
//! crate, while still letting users opt into the derive through a regular feature.

use proc_macro::TokenStream;
use syn::{DeriveInput, parse_macro_input};

mod attrs;
mod expand;
mod generics;

/// Derives `rg_memsize::MemorySize` by generating `record_memory_children`.
///
/// The generated implementation follows the existing `rg_memsize` accounting convention:
/// `MemorySize::record_memory_size` records the shallow size, while the derive only walks owned
/// child values. Struct fields are recorded under their field names, tuple fields under their
/// numeric indexes, and one-field enum variants are treated as transparent wrappers by default.
///
/// Supported container attributes:
/// - `#[memsize(leaf)]`: generate a no-op child traversal for ids, flags, and marker values.
/// - `#[memsize(crate_path = "::some_path")]`: use a different path to the `rg_memsize` crate.
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
/// use rg_memsize::MemorySize;
///
/// #[derive(rg_memsize::MemorySize)]
/// struct Package {
///     name: String,
///     #[memsize(scope = "roots")]
///     target_roots: Vec<String>,
/// }
///
/// #[derive(rg_memsize::MemorySize)]
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
