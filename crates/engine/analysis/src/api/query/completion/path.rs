//! Qualified path completion assembly for body and import positions.

use rg_body_ir::{PathCompletionNamespace, PathCompletionSite};
use rg_def_map::{
    DefId, DefMapPathCompletionSite, ModuleRef, Path, ScopeNamespace, VisibleScopeDef,
};
use rg_parse::Span;
use rg_semantic_ir::Documentation;

use crate::{
    Analysis,
    model::{
        CompletionApplicability, CompletionEdit, CompletionItem, CompletionKind, CompletionTarget,
    },
};

use super::{CompletionMetadata, completion_sort::CompletionSortPolicy, def_completion_detail};

pub(super) struct PathCompletionResolver<'a, 'db>(&'a Analysis<'db>);

impl<'a, 'db> PathCompletionResolver<'a, 'db> {
    pub(super) fn new(analysis: &'a Analysis<'db>) -> Self {
        Self(analysis)
    }

    /// Collects qualified-path completions inside a body, such as
    /// `let value = crate::$0` or `let value: crate::api::Us$0`.
    pub(super) fn body_completions(
        &self,
        site: PathCompletionSite,
    ) -> anyhow::Result<Vec<CompletionItem>> {
        let Some(body) = self.0.body_ir.body_data(site.body)? else {
            return Ok(Vec::new());
        };

        self.module_path_completions(
            body.owner_module(),
            &site.qualifier,
            site.member_prefix_span,
            PathCompletionFilter::from(site.namespace),
        )
    }

    /// Collects qualified import completions, such as `use crate::api::$0`.
    pub(super) fn use_completions(
        &self,
        site: DefMapPathCompletionSite,
    ) -> anyhow::Result<Vec<CompletionItem>> {
        self.module_path_completions(
            site.module,
            &site.qualifier,
            site.member_prefix_span,
            PathCompletionFilter::All,
        )
    }

    /// Resolves the qualifier in `crate::api::$0` and renders definitions visible
    /// from the resolved module.
    fn module_path_completions(
        &self,
        importing_module: ModuleRef,
        qualifier: &Path,
        member_prefix_span: Span,
        filter: PathCompletionFilter,
    ) -> anyhow::Result<Vec<CompletionItem>> {
        let resolved = self
            .0
            .def_map
            .resolve_path_in_type_namespace(importing_module, qualifier)?;
        let edit = CompletionEdit {
            replace: member_prefix_span,
        };
        let mut completions = Vec::new();

        // Module path completion needs a module scope to list. Non-module qualifiers
        // such as type names are ignored here because they expose associated items
        // through a different lookup model.
        for def in resolved.resolved {
            let DefId::Module(source_module) = def else {
                continue;
            };
            for visible_def in self
                .0
                .def_map
                .visible_scope_defs(importing_module, source_module)?
            {
                if filter.accepts(visible_def.namespace) {
                    self.push_visible_scope_completion(visible_def, edit, &mut completions)?;
                }
            }
        }

        completions.sort_by(|left, right| left.sort_text.cmp(&right.sort_text));
        Ok(completions)
    }

    /// Adds one visible module-scope definition for path completion, such as
    /// `HashMap` in `std::collections::Ha$0`.
    fn push_visible_scope_completion(
        &self,
        visible_def: VisibleScopeDef,
        edit: CompletionEdit,
        completions: &mut Vec<CompletionItem>,
    ) -> anyhow::Result<()> {
        let Some((kind, metadata)) = self.visible_scope_completion_metadata(&visible_def)? else {
            return Ok(());
        };
        let target = CompletionTarget::Def(visible_def.def);
        if completions
            .iter()
            .any(|completion| completion.target == target && completion.label == metadata.label)
        {
            return Ok(());
        }

        completions.push(CompletionItem {
            label: metadata.label.clone(),
            kind,
            target,
            applicability: CompletionApplicability::Known,
            detail: metadata.detail,
            documentation: metadata.documentation,
            sort_text: CompletionSortPolicy::General.sort_text(
                None,
                &metadata.label,
                kind,
                CompletionApplicability::Known,
                target,
            ),
            edit: Some(edit),
        });
        Ok(())
    }

    fn visible_scope_completion_metadata(
        &self,
        visible_def: &VisibleScopeDef,
    ) -> anyhow::Result<Option<(CompletionKind, CompletionMetadata)>> {
        let (kind, metadata) = match visible_def.def {
            DefId::Module(module) => {
                let Some(data) = self.0.def_map.module(module)? else {
                    return Ok(None);
                };
                (
                    CompletionKind::Module,
                    CompletionMetadata {
                        label: visible_def.label.clone(),
                        detail: Some(format!("mod {}", visible_def.label)),
                        documentation: data.docs.as_ref().map(Documentation::text),
                    },
                )
            }
            DefId::Local(local_def) => {
                let Some(data) = self.0.def_map.local_def(local_def)? else {
                    return Ok(None);
                };
                let kind = CompletionKind::from_local_def_kind(data.kind);
                (
                    kind,
                    CompletionMetadata {
                        label: visible_def.label.clone(),
                        detail: Some(def_completion_detail(kind, &visible_def.label)),
                        documentation: None,
                    },
                )
            }
        };

        Ok(Some((kind, metadata)))
    }
}

/// Namespace policy for the segment being completed in a qualified path.
///
/// Type positions like `let value: crate::$0` accept type-namespace candidates.
/// Value positions like `let value = crate::$0` accept all candidates so modules
/// and types can still be used as prefixes on the way to a value item such as
/// `crate::api::build_user()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PathCompletionFilter {
    Types,
    All,
}

impl PathCompletionFilter {
    fn accepts(self, namespace: ScopeNamespace) -> bool {
        match self {
            Self::Types => matches!(namespace, ScopeNamespace::Types),
            Self::All => true,
        }
    }
}

impl From<PathCompletionNamespace> for PathCompletionFilter {
    fn from(namespace: PathCompletionNamespace) -> Self {
        match namespace {
            PathCompletionNamespace::Types => Self::Types,
            PathCompletionNamespace::Values => Self::All,
        }
    }
}
