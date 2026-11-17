use rg_cfg_eval::CfgExpr;
use rg_ir_model::{FileId, SourceId};
use rg_std::{MemorySize, Shrink};
use rg_text::Name;
use wincode::{SchemaRead, SchemaWrite};

pub use self::{
    attrs::UserFacingAttrs,
    const_expr::ConstExprData,
    decl::{
        ConstItem, ConstParamData, EnumItem, EnumVariantItem, ExternBlockItem, FieldItem,
        FieldList, FunctionItem, FunctionQualifiers, GenericParams, ImplItem, ImplPolarity,
        LifetimeParamData, ParamItem, ParamKind, ProcMacroDefinition, ProcMacroKind, SelfParamKind,
        StaticItem, StructItem, TraitItem, TypeAliasItem, TypeOrConstParamData, TypeParamData,
        UnionItem, WherePredicate,
    },
    derive::{BuiltinDeriveKind, DeriveAttrs, DeriveMacroCall},
    docs::{Documentation, DocumentationPlacement, DocumentationSource},
    import::{
        ExternCrateItem, ImportAlias, UseImport, UseImportKind, UseItem, UsePath, UsePathSegment,
        UsePathSegmentKind,
    },
    kind::{ItemKind, ItemTag},
    lang_item::LangItem,
    macro_item::{
        BuiltinMacroItem, BuiltinMacroKind, CfgAttrMacroUse, CfgSelectArmItem, CfgSelectArmPayload,
        IncludePathExpression, MacroCallItem, MacroDefinitionAttrs, MacroDefinitionItem,
        MacroUseAttr, MacroUseSelector,
    },
    module::{ModuleItem, ModuleSource},
    type_ref::{
        ConstExpr, GenericArg, TraitBoundModifier, TypeBound, TypeBoundListDisplay,
        TypeNameFormatter, TypePath, TypePathAnchor, TypePathSegment, TypeRef, TypeRefDisplay,
    },
    visibility::VisibilityLevel,
};

mod attrs;
mod const_expr;
mod decl;
mod derive;
mod docs;
mod import;
mod kind;
mod lang_item;
mod macro_item;
mod module;
mod type_ref;
mod visibility;

mod lowering;

pub use rg_ir_model::FieldKey;

pub use self::lowering::{
    FromAst, InnerDocs, MacroCallContext, MacroDefAst, MacroDefContext, MacroRulesAst,
    MacroRulesContext, MaybeFromAst, OuterDocs,
};

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
    /// Source location of the declaration name, when the item has one.
    pub name_location: Option<SourceId>,
    pub visibility: VisibilityLevel,
    /// Target-dependent cfg gates attached to the item.
    pub cfg: CfgExpr,
    /// Recognized compiler identity from a language-item or diagnostic-item attribute.
    pub lang_item: Option<LangItem>,
    /// User-facing documentation lowered from doc comments or `#[doc = "..."]`.
    pub docs: Option<Documentation>,
    /// Compact attribute subset used to filter global user-facing discovery.
    pub user_facing_attrs: UserFacingAttrs,
    /// File where this item is declared.
    pub file_id: FileId,
    /// Source location of the declaration.
    pub location: SourceId,
}

impl ItemNode {
    /// Creates a fully-populated item node from already-lowered parts.
    pub fn new(
        kind: ItemKind,
        name: Option<Name>,
        name_location: Option<SourceId>,
        visibility: VisibilityLevel,
        docs: Option<Documentation>,
        location: SourceId,
        file_id: FileId,
    ) -> Self {
        Self {
            kind,
            name,
            name_location,
            visibility,
            cfg: CfgExpr::default(),
            lang_item: None,
            docs,
            user_facing_attrs: UserFacingAttrs::default(),
            file_id,
            location,
        }
    }
}
