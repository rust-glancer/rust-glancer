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
mod signature_help;
mod symbol;
mod symbols;

pub use self::{
    code_action::{CodeAction, CodeActionEdit, CodeActionKind},
    completion::{
        CompletionAdditionalEdit, CompletionApplicability, CompletionEdit, CompletionImport,
        CompletionInsertText, CompletionItem, CompletionKind, CompletionResult, CompletionTarget,
        KeywordCompletion, SyntheticCompletionTarget,
    },
    folding::{Fold, FoldKind},
    highlight::{Highlight, HighlightKind},
    hover::{DocumentationLink, HoverBlock, HoverInfo},
    inlay_hints::{InlayHint, InlayHintKind, InlayHintPosition},
    navigation::{NavigationTarget, NavigationTargetKind},
    references::ReferenceLocation,
    rename::{RenameEdit, RenameResult, RenameTarget},
    signature_help::SignatureHelp,
    symbol::SymbolAt,
    symbols::{DocumentSymbol, WorkspaceSymbol},
};
