//! Reference declaration projection shared by reference-style queries.
//!
//! Source scanning and symbol matching stay in analysis query code. This view projects canonical
//! declaration identities into source locations for reference results.

use rg_ir_model::{TargetRef, identity::DeclarationRef};
use rg_parse::{FileId, Span};

use crate::api::view::{IndexedViewDb, declaration::DeclarationView};

/// One indexed source location for a declaration or use-site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct IndexedSourceLocation {
    pub(crate) target: TargetRef,
    pub(crate) file_id: FileId,
    pub(crate) span: Span,
}

pub(crate) struct ReferenceView<'a, 'db> {
    analysis: &'a IndexedViewDb<'db>,
}

impl<'a, 'db> ReferenceView<'a, 'db> {
    pub(crate) fn new(analysis: &'a IndexedViewDb<'db>) -> Self {
        Self { analysis }
    }

    pub(crate) fn declaration_locations(
        &self,
        declarations: &[DeclarationRef],
    ) -> anyhow::Result<Vec<IndexedSourceLocation>> {
        let mut locations = Vec::new();
        for declaration_ref in declarations {
            let Some(declaration) =
                DeclarationView::new(self.analysis).declaration(*declaration_ref)?
            else {
                continue;
            };
            locations.push(IndexedSourceLocation {
                target: declaration.target(),
                file_id: declaration.file_id(),
                span: declaration.selection_span(),
            });
        }
        Ok(locations)
    }
}
