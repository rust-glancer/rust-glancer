//! Bounded auto-import candidate discovery.

use anyhow::Context as _;
use rg_ir_view::{
    lookup::importable::ImportableNameSearch,
    source::{IndexedUnqualifiedNameContext, IndexedUnqualifiedNameScope},
    ty::locals::BodyView,
};

use crate::query::completion::site::UnqualifiedCompletionSite;

use super::{CompletionCandidateSource, DefinitionCompletionCandidate};

impl<'a, 'db> CompletionCandidateSource<'a, 'db> {
    /// Discover request-local auto-import candidates for ordinary type and value positions.
    pub(crate) fn auto_import_candidates_for_unqualified(
        &self,
        site: &UnqualifiedCompletionSite,
    ) -> anyhow::Result<Vec<DefinitionCompletionCandidate>> {
        let importing_module = match site.source().scope() {
            IndexedUnqualifiedNameScope::Body {
                scope,
                context:
                    IndexedUnqualifiedNameContext::Type { .. } | IndexedUnqualifiedNameContext::Value,
                ..
            } => {
                let Some(module) = BodyView::new(self.db)
                    .owner_module(scope.body_ir())
                    .context("read auto-import owner module")?
                else {
                    return Ok(Vec::new());
                };
                module
            }
            IndexedUnqualifiedNameScope::Signature {
                scope,
                context:
                    IndexedUnqualifiedNameContext::Type { .. } | IndexedUnqualifiedNameContext::Value,
                ..
            } => scope.context().module,
            IndexedUnqualifiedNameScope::Module {
                module,
                context:
                    IndexedUnqualifiedNameContext::Type { .. } | IndexedUnqualifiedNameContext::Value,
                ..
            } => *module,
            IndexedUnqualifiedNameScope::Body {
                context: IndexedUnqualifiedNameContext::Const,
                ..
            }
            | IndexedUnqualifiedNameScope::Signature {
                context: IndexedUnqualifiedNameContext::Const,
                ..
            }
            | IndexedUnqualifiedNameScope::Body {
                context: IndexedUnqualifiedNameContext::Pattern(_),
                ..
            }
            | IndexedUnqualifiedNameScope::Signature {
                context: IndexedUnqualifiedNameContext::Pattern(_),
                ..
            }
            | IndexedUnqualifiedNameScope::Module {
                context:
                    IndexedUnqualifiedNameContext::Const | IndexedUnqualifiedNameContext::Pattern(_),
                ..
            }
            | IndexedUnqualifiedNameScope::Import { .. } => return Ok(Vec::new()),
        };

        let mut candidates = Vec::new();
        for importable in ImportableNameSearch::new(self.db)
            .search(importing_module, site.member_prefix())
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
