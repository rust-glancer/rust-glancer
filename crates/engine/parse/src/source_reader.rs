//! Text and syntax utilities share the active captured input, regardless of discovery order.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use rg_ir_model::{FileId, Span};
use rg_source::{QueryContext, SourceInput, SourceRevision};
use rg_syntax::{Parse, SourceFile};
use rg_text::RustEdition;

use crate::{LineIndex, source_syntax};

/// Operation-scoped source access. A file does not have to be discovered by module collection:
/// declaration queries can discover includes later and use the same source authority here.
/// The enclosing operation selects saved inputs or an editor view before this reader is created.
#[derive(Debug, Clone, Copy)]
pub struct SourceReader<'db> {
    db: &'db QueryContext<'db>,
}

impl<'db> SourceReader<'db> {
    pub fn new(db: &'db QueryContext<'db>) -> Self {
        Self { db }
    }

    pub fn path(&self, file: FileId) -> ajika::Result<&'db Path> {
        Ok(source_file_path(self.db, file)?.as_path())
    }

    pub fn text(&self, file: FileId) -> ajika::Result<Arc<str>> {
        SourceInput::entry(self.db, file)?
            .text()
            .map_err(ajika::Error::application)
    }

    pub fn text_for_span(&self, file: FileId, span: Span) -> ajika::Result<Option<String>> {
        Ok(self
            .text(file)?
            .get(span.start as usize..span.end as usize)
            .map(str::to_owned))
    }

    pub fn revision(&self, file: FileId) -> ajika::Result<SourceRevision> {
        Ok(SourceInput::entry(self.db, file)?.revision())
    }

    pub fn syntax(
        &self,
        file: FileId,
        edition: RustEdition,
    ) -> ajika::Result<Arc<Parse<SourceFile>>> {
        source_syntax(self.db, file, edition)
    }

    pub fn line_index(&self, file: FileId) -> ajika::Result<&'db LineIndex> {
        LineIndex::for_file(self.db, file)
    }
}

// Keep decoded paths in the operation so navigation can borrow them without retaining a file
// table for every lazy source in the project.
#[ajika::query(transient, returns(ref))]
fn source_file_path(db: &QueryContext<'_>, file: FileId) -> ajika::Result<PathBuf> {
    file.path(&db.interner())
}
