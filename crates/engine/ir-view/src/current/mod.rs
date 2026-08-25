//! Semantic views of selected module-level declarations from the editor buffer.
//!
//! These declarations do not yet belong to the saved project generation. Each view lowers only
//! the syntax needed by one request into temporary semantic storage, while continuing to resolve
//! surrounding names through the saved module that contains the declaration.
//!
//! Current-source scanners and rebuilt function bodies remain in their domain modules. This
//! namespace is specifically for module-level declarations that need a request-local semantic
//! identity before an indexed view can answer questions about them.

mod trait_impl;

pub use trait_impl::CurrentTraitImplView;
