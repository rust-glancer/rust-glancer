use ra_syntax::TextRange;

use rg_parse::{FileId, Span};
use rg_text::Name;

pub use self::{
    decl::{
        ConstItem, EnumItem, EnumVariantItem, FieldItem, FieldKey, FieldList, FunctionItem,
        FunctionQualifiers, GenericParams, ImplItem, ParamItem, ParamKind, StaticItem, StructItem,
        TraitItem, TypeAliasItem, UnionItem, WherePredicate,
    },
    docs::Documentation,
    import::{
        ExternCrateItem, ImportAlias, UseImport, UseImportKind, UseItem, UsePath, UsePathSegment,
        UsePathSegmentKind,
    },
    kind::{ItemKind, ItemTag},
    module::{ModuleItem, ModuleSource},
    type_ref::{GenericArg, Mutability, TypeBound, TypePath, TypePathSegment, TypeRef},
    visibility::VisibilityLevel,
};

pub(crate) use self::decl::{ConstParamData, LifetimeParamData, TypeParamData};

mod decl;
mod docs;
mod import;
mod kind;
mod module;
mod type_ref;
mod visibility;

/// Stable file-local identifier for one lowered item-tree node.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize,
)]
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
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize,
)]
pub struct ItemTreeRef {
    pub file_id: FileId,
    pub item: ItemTreeId,
}

/// AST-independent item-tree node used by later lowering stages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemNode {
    pub kind: ItemKind,
    /// Name (when applicable), e.g. for functions or structs.
    pub name: Option<Name>,
    /// Source span of the declaration name, when the item has one.
    pub name_span: Option<Span>,
    pub visibility: VisibilityLevel,
    /// User-facing documentation lowered from doc comments or `#[doc = "..."]`.
    pub docs: Option<Documentation>,
    /// File where this item is declared.
    pub file_id: FileId,
    /// Source span of the declaration.
    pub span: Span,
}

impl ItemNode {
    /// Creates a fully-populated item node from already-lowered parts.
    pub(super) fn new(
        kind: ItemKind,
        name: Option<Name>,
        name_range: Option<TextRange>,
        visibility: VisibilityLevel,
        docs: Option<Documentation>,
        text_range: TextRange,
        file_id: FileId,
    ) -> Self {
        Self {
            kind,
            name,
            name_span: name_range.map(Span::from_text_range),
            visibility,
            docs,
            file_id,
            span: Span::from_text_range(text_range),
        }
    }
}

pub(super) fn normalized_syntax(node: &impl ra_syntax::AstNode) -> String {
    node.syntax()
        .text()
        .to_string()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
