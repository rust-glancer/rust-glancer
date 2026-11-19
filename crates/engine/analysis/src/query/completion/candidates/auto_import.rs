//! Bounded auto-import candidate discovery.

use std::time::Instant;

use anyhow::Context as _;
use rg_ir_model::Path;
use rg_ir_view::lookup::{
    importable::{ImportPathKind, ImportableNameSearch},
    name::NameNamespace,
};

use super::{CompletionCandidateSource, DefinitionCompletionCandidate};
use crate::query::{
    completion::site::{NameCompletionContext, UnqualifiedCompletionSite},
    import::ImportContext,
};

impl<'a, 'db> CompletionCandidateSource<'a, 'db> {
    /// Discover request-local auto-import candidates for ordinary type and value positions.
    pub(crate) fn auto_import_candidates_for_unqualified(
        &self,
        site: &UnqualifiedCompletionSite,
        deadline: Instant,
        occupied: impl Fn(&str, NameNamespace) -> bool,
    ) -> anyhow::Result<Vec<(DefinitionCompletionCandidate, Path)>> {
        let Some(import_context) = ImportContext::for_unqualified_site(self.db, site.source())
            .context("read auto-import context")?
        else {
            return Ok(Vec::new());
        };

        let mut candidates = Vec::new();
        for importable in ImportableNameSearch::new(self.db)
            .search(
                import_context.module(),
                site.member_prefix(),
                site.import_path_kind(),
                deadline,
                occupied,
            )
            .context("search auto-import candidates")?
        {
            let Some(candidate) = self
                .module_candidate(importable.name().clone())
                .context("prepare auto-import candidate")?
            else {
                continue;
            };
            candidates.push((candidate, importable.path().clone()));
        }
        Ok(candidates)
    }
}

impl UnqualifiedCompletionSite {
    fn import_path_kind(&self) -> ImportPathKind {
        if self.context() == NameCompletionContext::Type {
            ImportPathKind::Type
        } else {
            ImportPathKind::TypeOrValue
        }
    }
}
