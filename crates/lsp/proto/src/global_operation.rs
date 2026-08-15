//! Result shape for operations whose locations or edits come from saved source.
//!
//! References, rename, and similar cross-file operations use identities and byte ranges from the
//! saved project. If an applicable open document has different text, or is not in that project yet,
//! the engine asks for a save instead of returning a location or edit that may point at the wrong
//! text.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Either a completed cross-file result or the open document that must be saved first.
///
/// `SaveRequired` is not an empty result and not an engine failure. It means the operation cannot
/// safely map its saved byte ranges to the document currently shown at `path`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub enum GlobalOperationResult<T> {
    /// The operation completed using source ranges that are safe for the open documents.
    Ready(T),
    /// This document differs from saved source, or is not indexed yet, and must be saved first.
    SaveRequired { path: PathBuf },
}

impl<T> GlobalOperationResult<T> {
    pub fn ready(value: T) -> Self {
        Self::Ready(value)
    }

    pub fn save_required(path: impl AsRef<Path>) -> Self {
        Self::SaveRequired {
            path: path.as_ref().to_path_buf(),
        }
    }
}
