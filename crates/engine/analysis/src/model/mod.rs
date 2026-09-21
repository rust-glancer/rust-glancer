//! Transport-neutral result models returned by editor analysis queries.

mod code_action;
mod completion;
mod folding;
mod highlight;
mod hover;
mod inlay_hints;
mod navigation;
mod references;
mod rename;
mod symbol;
mod symbols;

pub use self::code_action::{CodeAction, CodeActionEdit, CodeActionKind};
pub use self::completion::{
    CompletionAdditionalEdit, CompletionApplicability, CompletionEdit, CompletionInsertText,
    CompletionItem, CompletionKind, CompletionTarget, KeywordCompletion, SyntheticCompletionTarget,
};
pub use self::folding::{Fold, FoldKind};
pub use self::highlight::{Highlight, HighlightKind};
pub use self::hover::{DocumentationLink, HoverBlock, HoverInfo};
pub use self::inlay_hints::{InlayHint, InlayHintKind, InlayHintPosition};
pub use self::navigation::{NavigationTarget, NavigationTargetKind, NavigationTargetSource};
pub use self::references::ReferenceLocation;
pub use self::rename::{RenameEdit, RenameResult, RenameTarget};
pub use self::symbol::SymbolAt;
pub use self::symbols::{DocumentOutline, DocumentSymbol, WorkspaceSymbol};
