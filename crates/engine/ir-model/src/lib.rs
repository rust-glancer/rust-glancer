//! Storage-free identity model shared by the indexed IR layers.
//!
//! These types name things, but they do not know how those things are stored. Keeping them below
//! DefMap, Semantic IR, and Body IR lets higher layers talk about one declaration or one function
//! without making any single storage crate own the aggregate identity.

macro_rules! impl_arena_id {
    ($($id:ty),+ $(,)?) => {
        $(
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

mod body;
mod def_map;
pub mod identity;
mod resolution;
mod semantic;

pub use self::{
    body::{
        BindingId, BodyBindingRef, BodyDeclarationRef, BodyEnumVariantRef, BodyFieldRef,
        BodyFunctionId, BodyFunctionRef, BodyId, BodyImplId, BodyImplRef, BodyItemId, BodyItemRef,
        BodyRef, BodyValueItemId, BodyValueItemRef, ExprId, PatId, ScopeId, StmtId,
    },
    def_map::{
        DefId, ImportId, ImportRef, LocalDefId, LocalDefRef, LocalImplId, LocalImplRef, ModuleId,
        ModuleRef, TargetRef,
    },
    resolution::ResolvedDeclarationRef,
    semantic::{
        AssocItemId, ConstId, ConstRef, EnumId, EnumVariantRef, FieldRef, FunctionId, FunctionRef,
        ImplId, ImplRef, ItemId, ItemOwner, SemanticDeclarationRef, SemanticItemKind,
        SemanticItemRef, StaticId, StaticRef, StructId, TraitApplicability, TraitId, TraitImplRef,
        TraitRef, TypeAliasId, TypeAliasRef, TypeDefId, TypeDefRef, UnionId,
    },
};
