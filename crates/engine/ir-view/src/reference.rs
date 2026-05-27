//! Reference declaration projection shared by reference-style queries.
//!
//! Source scanning and symbol matching stay in analysis query code. This view projects canonical
//! declaration identities into source locations for reference results.

use rg_ir_model::{TargetRef, identity::DeclarationRef};
use rg_parse::{FileId, Span};

use crate::{IndexedViewDb, declaration::DeclarationView};

/// One indexed source location for a declaration or use-site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndexedSourceLocation {
    pub target: TargetRef,
    pub file_id: FileId,
    pub span: Span,
}

pub struct ReferenceView<'a, 'db> {
    analysis: &'a IndexedViewDb<'db>,
}

impl<'a, 'db> ReferenceView<'a, 'db> {
    pub fn new(analysis: &'a IndexedViewDb<'db>) -> Self {
        Self { analysis }
    }

    pub fn declaration_locations(
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
