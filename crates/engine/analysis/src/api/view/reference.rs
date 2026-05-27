//! Reference identity projection shared by reference-style queries.
//!
//! Source scanning tells us where candidate symbols are. This view owns the separate question of
//! which declaration identities those symbols denote, so reference queries can compare candidates
//! without knowing how each indexing layer resolves names.

use crate::{
    api::{
        Analysis,
        view::{declaration::DeclarationView, resolution::ResolutionView},
    },
    model::{DeclarationRef, ReferenceLocation, SymbolAt},
};

pub(crate) struct ReferenceView<'a, 'db> {
    analysis: &'a Analysis<'db>,
}

impl<'a, 'db> ReferenceView<'a, 'db> {
    pub(crate) fn new(analysis: &'a Analysis<'db>) -> Self {
        Self { analysis }
    }

    pub(crate) fn declarations_for_symbol(
        &self,
        symbol: SymbolAt,
    ) -> anyhow::Result<Vec<DeclarationRef>> {
        let declarations = ResolutionView::new(self.analysis).declarations_for_symbol(symbol)?;
        let mut unique = Vec::new();
        for declaration in declarations {
            if !unique.contains(&declaration) {
                unique.push(declaration);
            }
        }
        Ok(unique)
    }

    pub(crate) fn symbol_matches_declarations(
        &self,
        symbol: SymbolAt,
        declarations: &[DeclarationRef],
    ) -> anyhow::Result<bool> {
        let candidate_declarations = self.declarations_for_symbol(symbol)?;
        Ok(candidate_declarations
            .iter()
            .any(|candidate| declarations.contains(candidate)))
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
