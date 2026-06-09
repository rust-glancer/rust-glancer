use rg_cfg_eval::CfgExpr;
use rg_parse::{FileId, Span};
use rg_std::{MemorySize, Shrink};
use rg_text::Name;
use wincode::{SchemaRead, SchemaWrite};

pub use self::{
    decl::{
        ConstItem, ConstParamData, EnumItem, EnumVariantItem, FieldItem, FieldKey, FieldList,
        FunctionItem, FunctionQualifiers, GenericParams, ImplItem, LifetimeParamData, ParamItem,
        ParamKind, StaticItem, StructItem, TraitItem, TypeAliasItem, TypeParamData, UnionItem,
        WherePredicate,
    },
    docs::Documentation,
    import::{
        ExternCrateItem, ImportAlias, UseImport, UseImportKind, UseItem, UsePath, UsePathSegment,
        UsePathSegmentKind,
    },
    kind::{ItemKind, ItemTag},
    macro_item::{
        BuiltinMacroItem, CfgAttrMacroUse, CfgSelectArmItem, CfgSelectArmPayload, MacroCallItem,
        MacroDefinitionAttrs, MacroDefinitionItem, MacroUseAttr, MacroUseSelector,
    },
    module::{ModuleItem, ModuleSource},
    primitive::{FloatTy, PrimitiveTy, SignedIntTy, UnsignedIntTy},
    type_ref::{GenericArg, Mutability, TypeBound, TypePath, TypePathSegment, TypeRef},
    visibility::VisibilityLevel,
};

mod decl;
mod docs;
mod import;
mod kind;
mod macro_item;
mod module;
mod primitive;
mod type_ref;
mod visibility;

/// Stable file-local identifier for one lowered item-tree node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[memsize(leaf)]
#[shrink(leaf)]
pub struct ItemTreeId(pub usize);

impl rg_arena::ArenaId for ItemTreeId {
    fn from_index(index: usize) -> Self {
        Self(index)
    }

    fn index(self) -> usize {
        self.0
    }
}

/// Stable project-local reference to one item-tree node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[shrink(leaf)]
pub struct ItemTreeRef {
    pub file_id: FileId,
    pub item: ItemTreeId,
}

/// AST-independent item-tree node used by later lowering stages.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct ItemNode {
    pub kind: ItemKind,
    /// Name (when applicable), e.g. for functions or structs.
    pub name: Option<Name>,
    /// Source span of the declaration name, when the item has one.
    pub name_span: Option<Span>,
    pub visibility: VisibilityLevel,
    /// Target-dependent cfg gates attached to the item.
    pub cfg: CfgExpr,
    /// User-facing documentation lowered from doc comments or `#[doc = "..."]`.
    pub docs: Option<Documentation>,
    /// File where this item is declared.
    pub file_id: FileId,
    /// Source span of the declaration.
    pub span: Span,
}

impl ItemNode {
    /// Creates an item node from source-like syntax that does not have target-specific cfg state.
    pub fn source(
        kind: ItemKind,
        name: Option<Name>,
        name_span: Option<Span>,
        visibility: VisibilityLevel,
        docs: Option<Documentation>,
        span: Span,
        file_id: FileId,
    ) -> Self {
        Self::new(kind, name, name_span, visibility, docs, span, file_id)
    }

    /// Creates a fully-populated item node from already-lowered parts.
    pub fn new(
        kind: ItemKind,
        name: Option<Name>,
        name_span: Option<Span>,
        visibility: VisibilityLevel,
        docs: Option<Documentation>,
        span: Span,
        file_id: FileId,
    ) -> Self {
        Self {
            kind,
            name,
            name_span,
            visibility,
            cfg: CfgExpr::default(),
            docs,
            file_id,
            span,
        }
    }
}
