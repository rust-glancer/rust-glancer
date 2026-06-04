use rg_ir_model::TargetRef;
use rg_ir_view::SymbolKind;
use rg_parse::{FileId, Span};

/// One goto-definition destination.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NavigationTarget {
    pub target: TargetRef,
    pub kind: NavigationTargetKind,
    pub name: String,
    pub file_id: FileId,
    pub span: Option<Span>,
}

/// Navigation target category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, derive_more::Display)]
pub enum NavigationTargetKind {
    #[display("local")]
    LocalBinding,
    #[display("module")]
    Module,
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
}

impl From<SymbolKind> for NavigationTargetKind {
    fn from(kind: SymbolKind) -> Self {
        match kind {
            SymbolKind::Const => Self::Const,
            SymbolKind::Enum => Self::Enum,
            SymbolKind::EnumVariant => Self::EnumVariant,
            SymbolKind::Field => Self::Field,
            SymbolKind::Function | SymbolKind::Method => Self::Function,
            SymbolKind::Impl => Self::Impl,
            SymbolKind::Macro => Self::Macro,
            SymbolKind::Module => Self::Module,
            SymbolKind::Static => Self::Static,
            SymbolKind::Struct => Self::Struct,
            SymbolKind::Trait => Self::Trait,
            SymbolKind::TypeAlias => Self::TypeAlias,
            SymbolKind::Union => Self::Union,
            SymbolKind::Variable => Self::LocalBinding,
        }
    }
}
