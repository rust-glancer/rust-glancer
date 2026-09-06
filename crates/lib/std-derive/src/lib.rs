//! Procedural macro implementation for `rg_std`.
//!
//! This crate is re-exported by `rg_std`, so normal call sites should write derives such as
//! `#[derive(MemorySize)]` and `#[derive(Shrink)]` rather than depending on this crate directly.
//! Keeping the implementation separate avoids making the runtime `rg_std` crate a proc-macro
//! crate.

use proc_macro::TokenStream;
use syn::{DeriveInput, parse_macro_input};

mod cancelable;
mod generics;
mod memory_size;
mod shrink;

/// Check cancellation before executing a fallible method or function.
///
/// `#[cancelable]` checks `self` through `rg_std::Cancelable`. For an associated or free function,
/// name its cancellation argument with `#[cancelable(token = cancellation)]`. An optional string
/// describes the work, as in `#[cancelable("lower body", token = cancellation)]`; the default label
/// is the module and function name.
/// `token` can also borrow an enclosing operation, such as `token = self.db` in a short-lived view.
///
/// The expansion inserts one `rg_std::check_cancel!` at the start of the body. It leaves loops,
/// early returns, and result publication alone, so those still need their own checks. For async
/// functions the entry check runs when the future is polled. The return error must accept
/// `rg_std::Cancelled`.
#[proc_macro_attribute]
pub fn cancelable(args: TokenStream, input: TokenStream) -> TokenStream {
    cancelable::expand(args.into(), input.into())
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

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

    memory_size::derive(input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// Derives `Shrink` by generating `shrink_to_fit` over owned child fields.
///
/// Supported container attributes:
/// - `#[shrink(leaf)]`: generate a no-op implementation for ids, flags, and marker values.
/// - `#[shrink(crate_path = "::some_path")]`: use a different path to the `rg_std` crate.
/// - `#[shrink(no_auto_bound)]`: skip generated `T: Shrink` bounds.
/// - `#[shrink(bound = "T: SomeBound")]`: add an explicit where-clause predicate.
///
/// Supported field attributes:
/// - `#[shrink(skip)]`: omit the field and do not require a `Shrink` bound for it.
/// - `#[shrink(with = "shrink_field")]`: call a custom `fn(&mut FieldTy)`.
///
/// Supported variant attributes:
/// - `#[shrink(skip)]`: omit every child for that variant.
#[proc_macro_derive(Shrink, attributes(shrink))]
pub fn derive_shrink(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);

    shrink::derive(input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}
