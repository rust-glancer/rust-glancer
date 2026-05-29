//! Storage-free identity model shared by the indexed IR layers.
//!
//! These types name things, but they do not know how those things are stored. Keeping them below
//! DefMap, Semantic IR, and Body IR lets higher layers talk about one declaration or one function
//! without making any single storage crate own the aggregate identity.

pub mod hir;
mod ids;

pub use self::ids::{
    body::{
        BindingId, BodyBindingRef, BodyDeclarationRef, BodyEnumVariantRef, BodyFieldRef,
        BodyFunctionId, BodyFunctionRef, BodyId, BodyImplId, BodyImplRef, BodyItemId, BodyItemRef,
        BodyRef, BodyValueItemId, BodyValueItemRef, ExprId, PatId, ScopeId, StmtId,
    },
    def_map::{
        DefId, DefMapRef, ImportId, ImportRef, LocalDefId, LocalDefRef, LocalImplId, LocalImplRef,
        ModuleId, ModuleRef, TargetRef,
    },
    identity,
    resolution::ResolvedDeclarationRef,
    semantic::{
        AssocItemId, ConstId, ConstRef, EnumId, EnumVariantRef, FieldRef, FunctionId, FunctionRef,
        ImplId, ImplRef, ItemId, ItemOwner, SemanticDeclarationRef, SemanticItemKind,
        SemanticItemRef, StaticId, StaticRef, StructId, TraitApplicability, TraitId, TraitImplRef,
        TraitRef, TypeAliasId, TypeAliasRef, TypeDefId, TypeDefRef, UnionId,
    },
};

// We have a lot of arenas, and each has to have a unique ID.
// This macro takes care of boilerplate.
macro_rules! declare_id {
    (
        $(
            $(#[$attrs:meta])*
            $vis:vis struct $id:ident;
        )+
    ) => {
        $(
            $(#[$attrs])*
            #[derive(
                Debug,
                Clone,
                Copy,
                PartialEq,
                Eq,
                Hash,
                wincode::SchemaRead,
                wincode::SchemaWrite,
                rg_memsize::MemorySize,
            )]
            #[memsize(leaf)]
            $vis struct $id(pub usize);

            impl rg_arena::ArenaId for $id {
                fn from_index(index: usize) -> Self {
                    Self(index)
                }

                fn index(self) -> usize {
                    self.0
                }
            }
        )+
    };
}

pub(crate) use declare_id;
