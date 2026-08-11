//! Shared identities and small language primitives for the indexed IR layers.
//!
//! This crate deliberately owns no item or body HIR. It only contains stable routing references
//! and context-free Rust primitives needed below more than one owning domain. Item syntax belongs
//! to `rg_item_tree`; structural bodies belong to `rg_body_ir`.

mod body_source;
mod builtin_macro;
mod field;
mod ids;
mod literal;
mod mutability;
mod operator;
pub mod path;
mod primitive;

pub use self::ids::{
    body::{BindingId, BodyBindingRef, BodyId, BodyRef, ExprId, PatId, ScopeId, StmtId},
    def_map::{
        CrateId, CrateRef, DefId, DefMapRef, ImportId, ImportRef, LocalDefId, LocalDefRef,
        LocalEnumVariantId, LocalEnumVariantRef, LocalImplId, LocalImplRef, ModuleId, ModuleRef,
    },
    identity,
    semantic::{
        AssocItemId, ConstId, ConstParamRef, ConstRef, EnumId, EnumVariantFieldRef, EnumVariantRef,
        FieldRef, FunctionId, FunctionRef, GenericDefRef, GenericParamRef, ImplId, ImplRef, ItemId,
        ItemOwner, LifetimeParamRef, LocalLifetimeParamId, LocalTypeOrConstParamId, OpaqueTyId,
        OpaqueTyRef, SemanticItemKind, SemanticItemRef, StaticId, StaticRef, StructId,
        TraitApplicability, TraitDefRef, TraitId, TraitImplRef, TypeAliasId, TypeAliasRef,
        TypeDefId, TypeDefRef, TypeParamRef, UnionId,
    },
};
pub use self::mutability::Mutability;
pub use self::path::{Path, PathRoot};
pub use self::{
    body_source::BodySource,
    builtin_macro::BuiltinMacroExprKind,
    field::FieldKey,
    literal::LiteralKind,
    operator::{ExprBinaryOp, ExprUnaryOp},
    primitive::{FloatTy, PrimitiveTy, SignedIntTy, UnsignedIntTy},
};
pub use rg_parse::{FileId, Span, TextSpan};
pub use rg_workspace::PackageSlot;

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
                SchemaRead,
                SchemaWrite,
                MemorySize,
                Shrink,
            )]
            #[memsize(leaf)]
            #[shrink(leaf)]
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
