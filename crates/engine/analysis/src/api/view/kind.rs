//! View-native declaration categories.

use rg_body_ir::{BodyFunctionOwner, BodyItemKind, BodyValueItemKind};
use rg_def_map::LocalDefKind;
use rg_ir_model::SemanticItemKind;

/// Generic indexed declaration category independent from any editor transport model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, derive_more::Display)]
pub(crate) enum IndexedSymbolKind {
    #[display("const")]
    Const,
    #[display("enum")]
    Enum,
    #[display("variant")]
    EnumVariant,
    #[display("field")]
    Field,
    #[display("fn")]
    Function,
    #[display("impl")]
    Impl,
    #[display("macro")]
    Macro,
    #[display("method")]
    Method,
    #[display("module")]
    Module,
    #[display("static")]
    Static,
    #[display("struct")]
    Struct,
    #[display("trait")]
    Trait,
    #[display("type_alias")]
    TypeAlias,
    #[display("union")]
    Union,
    #[display("variable")]
    Variable,
}

impl IndexedSymbolKind {
    pub(crate) fn from_local_def_kind(kind: LocalDefKind) -> Self {
        match kind {
            LocalDefKind::Const => Self::Const,
            LocalDefKind::Enum => Self::Enum,
            LocalDefKind::Function => Self::Function,
            LocalDefKind::MacroDefinition => Self::Macro,
            LocalDefKind::Static => Self::Static,
            LocalDefKind::Struct => Self::Struct,
            LocalDefKind::Trait => Self::Trait,
            LocalDefKind::TypeAlias => Self::TypeAlias,
            LocalDefKind::Union => Self::Union,
        }
    }

    pub(crate) fn from_body_item_kind(kind: BodyItemKind) -> Self {
        match kind {
            BodyItemKind::Struct => Self::Struct,
            BodyItemKind::Enum => Self::Enum,
            BodyItemKind::Union => Self::Union,
            BodyItemKind::TypeAlias => Self::TypeAlias,
            BodyItemKind::Trait => Self::Trait,
        }
    }

    pub(crate) fn from_body_value_item_kind(kind: BodyValueItemKind) -> Self {
        match kind {
            BodyValueItemKind::Const => Self::Const,
            BodyValueItemKind::Static => Self::Static,
        }
    }

    pub(crate) fn from_semantic_item_kind(kind: SemanticItemKind) -> Self {
        match kind {
            SemanticItemKind::Struct => Self::Struct,
            SemanticItemKind::Enum => Self::Enum,
            SemanticItemKind::Union => Self::Union,
            SemanticItemKind::Trait => Self::Trait,
            SemanticItemKind::Impl => Self::Impl,
            SemanticItemKind::Function => Self::Function,
            SemanticItemKind::TypeAlias => Self::TypeAlias,
            SemanticItemKind::Const => Self::Const,
            SemanticItemKind::Static => Self::Static,
        }
    }

    pub(crate) fn from_body_function_owner(owner: BodyFunctionOwner) -> Self {
        match owner {
            BodyFunctionOwner::LocalScope(_) => Self::Function,
            BodyFunctionOwner::LocalImpl(_) => Self::Method,
        }
    }
}
