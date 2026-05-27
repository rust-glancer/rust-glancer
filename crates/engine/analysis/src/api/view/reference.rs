//! Reference declaration projection shared by reference-style queries.
//!
//! Source scanning and symbol matching stay in analysis query code. This view projects canonical
//! declaration identities into source locations for reference results.

use rg_ir_model::identity::DeclarationRef;

use crate::{
    api::{Analysis, view::declaration::DeclarationView},
    model::ReferenceLocation,
};

pub(crate) struct ReferenceView<'a, 'db> {
    analysis: &'a Analysis<'db>,
}

impl<'a, 'db> ReferenceView<'a, 'db> {
    pub(crate) fn new(analysis: &'a Analysis<'db>) -> Self {
        Self { analysis }
    }

    pub(crate) fn declaration_locations(
        &self,
        declarations: &[DeclarationRef],
    ) -> anyhow::Result<Vec<ReferenceLocation>> {
        let mut locations = Vec::new();
        for declaration_ref in declarations {
            let Some(declaration) =
                DeclarationView::new(self.analysis).declaration(*declaration_ref)?
            else {
                continue;
            };
            locations.push(ReferenceLocation {
                target: declaration.target(),
                file_id: declaration.file_id(),
                span: declaration.selection_span(),
            });
        }
        Ok(locations)
    }
}
