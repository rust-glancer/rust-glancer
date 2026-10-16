//! Bounded compiler identities retained by item-tree lowering.

use rg_std::{MemorySize, Shrink};
use wincode::{SchemaRead, SchemaWrite};

/// Compiler-known identity retained for language behavior that cannot be inferred from a name.
///
/// Rust permits these items to be re-exported or reached through renamed crates, so consumers must
/// not recognize them from paths such as `core::ops::Deref`. This list is intentionally bounded to
/// identities rust-glancer consumes. Adding another consumer should extend the list rather than
/// introducing a second spelling- or path-based lookup.
///
/// Most come from `#[lang = "..."]`. `Arc` and `Rc` use `#[rustc_diagnostic_item = "..."]`
/// instead; they share this index because analysis needs the same declaration lookup for both.
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
    /// The compiler-provided trait implemented by every possible pointee type.
    PointeeSized,
    /// Sizedness identities required by the compiler trait solver.
    Sized,
    MetaSized,
    /// Builtin obligations in callable and allocator signatures.
    Tuple,
    Destruct,
    /// The formatting arguments produced by `format_args!`.
    FormatArguments,
    /// The `Option` enum, including the result of `option_env!`.
    Option,
    /// The trait supplying the successful output of `value?`.
    Try,
    /// The nominal types constructed by ordinary range expressions.
    Range,
    RangeFrom,
    RangeTo,
    RangeInclusive,
    RangeToInclusive,
    RangeFull,
    /// The trait supplying the place type of `base[index]`.
    Index,
    /// The owning pointer whose pointee can be converted to a trait object.
    OwnedBox,
    /// Reference-counted pointers recognized for trait-object conversion.
    Arc,
    Rc,
    /// The iterator trait whose `Item` type is shown for standard adapter hints.
    Iterator,
}

impl LangItem {
    /// Every supported identity merged into the semantic use-site index.
    ///
    /// New enum variants belong here too; otherwise syntax can retain the identity but downstream
    /// queries will never see it.
    pub const ALL: [Self; 26] = [
        Self::Deref,
        Self::DerefTarget,
        Self::IntoIter,
        Self::Fn,
        Self::FnMut,
        Self::FnOnce,
        Self::FnOnceOutput,
        Self::PointeeSized,
        Self::Sized,
        Self::MetaSized,
        Self::Tuple,
        Self::Destruct,
        Self::FormatArguments,
        Self::Option,
        Self::Try,
        Self::Range,
        Self::RangeFrom,
        Self::RangeTo,
        Self::RangeInclusive,
        Self::RangeToInclusive,
        Self::RangeFull,
        Self::Index,
        Self::OwnedBox,
        Self::Arc,
        Self::Rc,
        Self::Iterator,
    ];

    /// Callable trait identities accepted by closure and function-call reasoning.
    pub const CALLABLE_TRAITS: [Self; 3] = [Self::Fn, Self::FnMut, Self::FnOnce];

    /// Recognizes the bounded subset of compiler attribute values that analysis consumes.
    ///
    /// Unknown language and diagnostic identities remain ordinary items. We retain only the
    /// identities with a semantic consumer, rather than every name the compiler recognizes.
    pub fn from_attr_value(attribute: &str, value: &str) -> Option<Self> {
        Some(match (attribute, value) {
            ("lang", "deref") => Self::Deref,
            ("lang", "deref_target") => Self::DerefTarget,
            ("lang", "into_iter") => Self::IntoIter,
            ("lang", "iterator") => Self::Iterator,
            ("lang", "fn") => Self::Fn,
            ("lang", "fn_mut") => Self::FnMut,
            ("lang", "fn_once") => Self::FnOnce,
            ("lang", "fn_once_output") => Self::FnOnceOutput,
            ("lang", "pointee_sized") => Self::PointeeSized,
            ("lang", "sized") => Self::Sized,
            ("lang", "meta_sized") => Self::MetaSized,
            ("lang", "tuple_trait") => Self::Tuple,
            ("lang", "destruct") => Self::Destruct,
            ("lang", "format_arguments") => Self::FormatArguments,
            ("lang", "Option") => Self::Option,
            ("lang", "Try") => Self::Try,
            ("lang", "Range") => Self::Range,
            ("lang", "RangeFrom") => Self::RangeFrom,
            ("lang", "RangeTo") => Self::RangeTo,
            ("lang", "RangeInclusive") => Self::RangeInclusive,
            ("lang", "RangeToInclusive") => Self::RangeToInclusive,
            ("lang", "RangeFull") => Self::RangeFull,
            ("lang", "index") => Self::Index,
            ("lang", "owned_box") => Self::OwnedBox,
            ("rustc_diagnostic_item", "Arc") => Self::Arc,
            ("rustc_diagnostic_item", "Rc") => Self::Rc,
            _ => return None,
        })
    }
}
