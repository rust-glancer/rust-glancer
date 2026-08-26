//! Bounded auto-import candidate discovery.

use anyhow::Context as _;
use rg_ir_view::lookup::importable::ImportableNameSearch;

use crate::query::{completion::site::UnqualifiedCompletionSite, import::ImportContext};

use super::{CompletionCandidateSource, DefinitionCompletionCandidate};

impl<'a, 'db> CompletionCandidateSource<'a, 'db> {
    /// Discover request-local auto-import candidates for ordinary type and value positions.
    pub(crate) fn auto_import_candidates_for_unqualified(
        &self,
        site: &UnqualifiedCompletionSite,
    ) -> anyhow::Result<Vec<DefinitionCompletionCandidate>> {
        let Some(import_context) = ImportContext::for_unqualified_site(self.db, site.source())
            .context("read auto-import context")?
        else {
            return Ok(Vec::new());
        };

        let mut candidates = Vec::new();
        for importable in ImportableNameSearch::new(self.db)
            .search(import_context.module(), site.member_prefix())
            .context("search auto-import candidates")?
        {
            let Some(mut candidate) = self.module_candidate(importable.name().clone()) else {
                continue;
            };
            candidate.module_origin = None;
            candidate.import_path = Some(importable.path().clone());
            candidate.import_path_len = Some(importable.path_len());
            candidates.push(candidate);
        }
        Ok(candidates)
    }
}
