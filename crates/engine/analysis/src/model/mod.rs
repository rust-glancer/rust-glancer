//! Transport-neutral result models returned by editor analysis queries.

mod code_action;
mod completion;
mod folding;
mod hover;
mod inlay_hints;
mod navigation;
mod references;
mod rename;
mod symbol;
mod symbols;

pub use code_action::{CodeAction, CodeActionEdit, CodeActionKind};
pub use completion::{
    CompletionAdditionalEdit, CompletionApplicability, CompletionEdit, CompletionInsertText,
    CompletionItem, CompletionKind, CompletionTarget, KeywordCompletion, SyntheticCompletionTarget,
};
pub use folding::{Fold, FoldKind};
pub use hover::{HoverBlock, HoverInfo};
pub use inlay_hints::{InlayHint, InlayHintKind, InlayHintPosition};
pub use navigation::{NavigationTarget, NavigationTargetKind, NavigationTargetSource};
pub use references::ReferenceLocation;
pub use rename::{RenameEdit, RenameResult, RenameTarget};
pub use symbol::SymbolAt;
pub use symbols::{DocumentOutline, DocumentSymbol, WorkspaceSymbol};
