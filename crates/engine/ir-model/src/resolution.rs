use rg_memsize::MemorySize;
use wincode::{SchemaRead, SchemaWrite};

use crate::{
    BodyBindingRef, BodyDeclarationRef, BodyEnumVariantRef, BodyFieldRef, BodyFunctionRef,
    BodyImplRef, BodyItemRef, BodyValueItemRef, ConstRef, DefId, EnumVariantRef, FieldRef,
    FunctionRef, ImplRef, SemanticDeclarationRef, SemanticItemRef, StaticRef, TraitRef,
    TypeAliasRef, TypeDefRef,
};

/// Storage-level declaration target produced by path and expression resolution.
///
/// This intentionally preserves the originating storage layer. Higher-level APIs can project it
/// into opaque concepts such as "function" or "field" without making Body IR own that aggregate
/// identity.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, derive_more::From, SchemaRead, SchemaWrite, MemorySize,
)]
pub enum ResolvedDeclarationRef {
    #[from]
    Def(DefId),
    #[from(
        SemanticDeclarationRef,
        SemanticItemRef,
        TypeDefRef,
        TraitRef,
        ImplRef,
        FunctionRef,
        TypeAliasRef,
        ConstRef,
        StaticRef,
        FieldRef,
        EnumVariantRef
    )]
    Semantic(SemanticDeclarationRef),
    #[from(
        BodyDeclarationRef,
        BodyBindingRef,
        BodyItemRef,
        BodyValueItemRef,
        BodyImplRef,
        BodyFieldRef,
        BodyEnumVariantRef,
        BodyFunctionRef
    )]
    Body(BodyDeclarationRef),
}
