//! Bounded compiler language identities retained by item-tree lowering.

use rg_std::{MemorySize, Shrink};
use wincode::{SchemaRead, SchemaWrite};

/// Compiler-known identity retained for language behavior that cannot be inferred from a name.
///
/// Rust permits these items to be re-exported or reached through renamed crates, so consumers must
/// not recognize them from paths such as `core::ops::Deref`. This list is intentionally bounded to
/// identities rust-glancer consumes. Adding another consumer should extend the list rather than
/// introducing a second spelling- or path-based lookup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[memsize(leaf)]
#[shrink(leaf)]
pub enum LangItem {
    /// The trait used for trait-backed `*value` and receiver autoderef.
    Deref,
    /// The `Deref::Target` associated type.
    DerefTarget,
    /// The `IntoIterator::into_iter` method called by `for` desugaring.
    ///
    /// This is the method identity, not the `IntoIter` associated type.
    IntoIter,
    /// The callable trait whose receiver can be borrowed immutably.
    Fn,
    /// The callable trait whose receiver can be borrowed mutably.
    FnMut,
    /// The callable trait whose receiver can be consumed.
    FnOnce,
    /// The `FnOnce::Output` associated type shared by the callable traits.
    FnOnceOutput,
}

impl LangItem {
    /// Every supported identity merged into the semantic use-site index.
    ///
    /// New enum variants belong here too; otherwise syntax can retain the identity but downstream
    /// queries will never see it.
    pub const ALL: [Self; 7] = [
        Self::Deref,
        Self::DerefTarget,
        Self::IntoIter,
        Self::Fn,
        Self::FnMut,
        Self::FnOnce,
        Self::FnOnceOutput,
    ];

    /// Callable trait identities accepted by closure and function-call reasoning.
    pub const CALLABLE_TRAITS: [Self; 3] = [Self::Fn, Self::FnMut, Self::FnOnce];

    /// Recognizes the bounded subset of compiler attribute values that analysis consumes.
    ///
    /// Unknown `#[lang = "..."]` values remain ordinary items. This lets rust-glancer retain only
    /// identities with a semantic consumer instead of pretending to implement every rustc lang
    /// item.
    pub fn from_attr_value(value: &str) -> Option<Self> {
        Some(match value {
            "deref" => Self::Deref,
            "deref_target" => Self::DerefTarget,
            "into_iter" => Self::IntoIter,
            "fn" => Self::Fn,
            "fn_mut" => Self::FnMut,
            "fn_once" => Self::FnOnce,
            "fn_once_output" => Self::FnOnceOutput,
            _ => return None,
        })
    }
}
