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

pub use self::{
    code_action::{CodeAction, CodeActionEdit, CodeActionKind},
    completion::{
        CompletionAdditionalEdit, CompletionApplicability, CompletionEdit, CompletionInsertText,
        CompletionItem, CompletionKind, CompletionTarget, KeywordCompletion,
        SyntheticCompletionTarget,
    },
    folding::{Fold, FoldKind},
    highlight::{Highlight, HighlightKind},
    hover::{DocumentationLink, HoverBlock, HoverInfo},
    inlay_hints::{InlayHint, InlayHintKind, InlayHintPosition},
    navigation::{NavigationTarget, NavigationTargetKind, NavigationTargetSource},
    references::ReferenceLocation,
    rename::{RenameEdit, RenameResult, RenameTarget},
    symbol::SymbolAt,
    symbols::{DocumentOutline, DocumentSymbol, WorkspaceSymbol},
};
