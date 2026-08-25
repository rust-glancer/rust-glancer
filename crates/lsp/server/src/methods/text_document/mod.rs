//! Document-scoped LSP handlers.
//!
//! Most files contain a free function with the feature's real logic so the already-large
//! `LanguageServer` implementation remains an index of protocol methods. Ordinary document
//! handlers receive a `DocumentMethodContext` with the snapshot chosen before the handler started.
//! Completion receives a `CompletionMethodContext` because it may replace that snapshot and retry
//! after another edit.
//!
//! Open/save/close handlers are different. The immediate editor-state work has already happened,
//! so those handlers perform only the async engine or registry work that remains.

pub(crate) mod code_action;
pub(crate) mod completion;
pub(crate) mod definition;
pub(crate) mod did_close;
pub(crate) mod did_open;
pub(crate) mod did_save;
pub(crate) mod document_highlight;
pub(crate) mod document_symbol;
pub(crate) mod formatting;
pub(crate) mod hover;
pub(crate) mod implementation;
pub(crate) mod inlay_hint;
pub(crate) mod references;
pub(crate) mod rename;
pub(crate) mod type_definition;
