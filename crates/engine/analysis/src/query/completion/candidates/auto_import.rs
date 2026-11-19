//! Bounded auto-import candidate discovery.

use std::time::Instant;

use anyhow::Context as _;
use rg_ir_model::Path;
use rg_ir_view::{
    lookup::{
        importable::{ImportPathKind, ImportableNameSearch},
        name::NameNamespace,
    },
    source::IndexedUnqualifiedNameScope,
    ty::locals::BodyView,
};

use super::{CompletionCandidateSource, DefinitionCompletionCandidate};
use crate::{
    model::CompletionKind,
    query::{
        completion::site::{NameCompletionContext, UnqualifiedCompletionSite},
        import::ImportContext,
    },
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

    /// Recheck the name in the selected path. The editor may already have replaced the
    /// partial spelling, and a newly introduced local or generic can now hide the import.
    pub(crate) fn can_import_selected_name(
        &self,
        site: &UnqualifiedCompletionSite,
        path: &Path,
        selected: Option<(&str, CompletionKind)>,
    ) -> anyhow::Result<bool> {
        let Some(context) = ImportContext::for_unqualified_site(self.db, site.source())? else {
            return Ok(false);
        };
        let Some(leaf) = path.segments().last() else {
            return Ok(false);
        };
        let mut hidden = Vec::new();
        if let Some((label, _)) = selected {
            if label.strip_prefix("r#").unwrap_or(label) != leaf.as_str() {
                return Ok(false);
            }
            // Block items and local uses live in the lexical def map checked below.
            // Parameters and let bindings are separate and observe declaration order.
            if site.context() == NameCompletionContext::Value
                && let IndexedUnqualifiedNameScope::Body {
                    scope,
                    visible_bindings,
                    ..
                } = site.source().scope()
                && BodyView::new(self.db).has_visible_binding(
                    *scope,
                    *visible_bindings,
                    leaf.as_str(),
                )?
            {
                hidden.push(NameNamespace::Values);
            }
            for name in self.generic_scope_candidates_for_unqualified(site, Some(leaf.as_str()))? {
                hidden.push(name.namespace());
            }
        }
        let scope = match site.source().scope() {
            IndexedUnqualifiedNameScope::Body { scope, .. } => Some(*scope),
            _ => None,
        };
        ImportableNameSearch::new(self.db).can_import_name(
            context.module(),
            path,
            site.import_path_kind(),
            scope,
            |ns, symbol| {
                !hidden.contains(&ns)
                    && selected.is_none_or(|(_, expected)| {
                        let actual = Self::completion_kind(symbol);
                        actual == Some(expected)
                            || (expected == CompletionKind::Struct
                                && actual == Some(CompletionKind::Union))
                    })
            },
        )
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
