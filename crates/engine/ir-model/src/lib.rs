//! Storage-free identity model shared by the indexed IR layers.
//!
//! These types name things, but they do not know how those things are stored. Keeping them below
//! DefMap, Semantic IR, and Body IR lets higher layers talk about one declaration or one function
//! without making any single storage crate own the aggregate identity.

pub mod hir;
mod ids;
pub mod items;
mod mutability;
pub mod path;
mod resolution;

pub use self::hir::body::{
    BindingData, BindingKind, BodyAssociatedPathPrefix, BodyData, BodyMacroCallData, BodyOwner,
    BodyPath, BodyPathSegment, BodyPathSegmentArgs, BodyPathSegmentKind, BodySource,
    BodySourceItem, BodySourceItems, BuiltinMacroExprKind, ClosureCapture, ClosureKind,
    ClosureParamData, ExprAssignOp, ExprBinaryOp, ExprBlockKind, ExprData, ExprKind, ExprRangeKind,
    ExprUnaryOp, ExprWrapperKind, FunctionParamData, LabelData, LiteralKind, MatchArmData,
    PatBindingMode, PatData, PatKind, PatRangeKind, RecordExprField, RecordExprSpread,
    RecordFieldSyntax, RecordPatField, ScopeData, StmtData, StmtKind,
};
pub use self::ids::{
    body::{BindingId, BodyBindingRef, BodyId, BodyRef, ExprId, PatId, ScopeId, StmtId},
    def_map::{
        CrateId, CrateRef, DefId, DefMapRef, ImportId, ImportRef, LocalDefId, LocalDefRef,
        LocalEnumVariantId, LocalEnumVariantRef, LocalImplId, LocalImplRef, ModuleId, ModuleRef,
    },
    identity,
    semantic::{
        AssocItemId, ConstId, ConstParamRef, ConstRef, EnumId, EnumVariantRef, FieldRef,
        FunctionId, FunctionRef, GenericDefRef, GenericParamRef, ImplId, ImplRef, ItemId,
        ItemOwner, LifetimeParamRef, LocalLifetimeParamId, LocalTypeOrConstParamId, OpaqueTyId,
        OpaqueTyRef, SemanticItemKind, SemanticItemRef, StaticId, StaticRef, StructId,
        TraitApplicability, TraitDefRef, TraitId, TraitImplRef, TypeAliasId, TypeAliasRef,
        TypeDefId, TypeDefRef, TypeParamRef, UnionId,
    },
};
pub use self::mutability::Mutability;
pub use self::path::{Path, PathSegment, last_segment_name};
pub use self::resolution::TypePathResolution;
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
