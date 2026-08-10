//! Module, macro, and visible-scope candidate lookup.

use anyhow::Context as _;
use rg_ir_model::{ModuleRef, Path};
use rg_ir_view::{
    SymbolKind,
    lookup::name::{MacroKind, ModuleScopeName, NameLookupView},
    source::{IndexedQualifiedPathScope, IndexedUnqualifiedNameScope},
    ty::locals::BodyView,
};

use crate::{
    model::{CompletionApplicability, CompletionKind, CompletionTarget},
    query::completion::site::{
        ModuleMacroCompletionSite, PathCompletionSite, UnqualifiedCompletionSite,
    },
};

use super::{CompletionCandidateSource, DefinitionCompletionCandidate};

impl<'a, 'db> CompletionCandidateSource<'a, 'db> {
    /// Return definitions visible from an explicit module or one resolved qualifier.
    pub(crate) fn module_candidates_at(
        &self,
        module: ModuleRef,
        qualifier: Option<&Path>,
    ) -> anyhow::Result<Vec<DefinitionCompletionCandidate>> {
        match qualifier {
            Some(qualifier) => self.module_path_candidates(module, qualifier),
            None => self.unqualified_module_candidates(module),
        }
    }

    /// Keep one requested proc/declarative macro family from module lookup.
    pub(crate) fn macro_candidates_at(
        &self,
        module: ModuleRef,
        qualifier: Option<&Path>,
        kind: MacroKind,
    ) -> anyhow::Result<Vec<DefinitionCompletionCandidate>> {
        let mut candidates = self
            .module_candidates_at(module, qualifier)
            .context("collect macro completion candidates")?;
        candidates.retain(|candidate| {
            candidate.kind() == CompletionKind::Macro && candidate.macro_kind() == Some(kind)
        });
        Ok(candidates)
    }

    /// Return dependency roots valid after `extern crate` in this module.
    pub(crate) fn extern_crate_candidates(
        &self,
        module: ModuleRef,
    ) -> anyhow::Result<Vec<DefinitionCompletionCandidate>> {
        Ok(NameLookupView::new(self.db)
            .extern_crate_names(module)
            .context("read extern crate completion candidates")?
            .into_iter()
            .filter_map(|name| self.module_candidate(name))
            .collect())
    }

    /// Return only module names legal in a restricted visibility path.
    pub(crate) fn visibility_module_candidates(
        &self,
        module: ModuleRef,
        qualifier: &Path,
    ) -> anyhow::Result<Vec<DefinitionCompletionCandidate>> {
        Ok(NameLookupView::new(self.db)
            .visibility_module_names_for_path(module, qualifier)
            .context("read visibility module completion candidates")?
            .into_iter()
            .filter_map(|name| self.module_candidate(name))
            .collect())
    }

    /// Find the importing module for a qualified site, then resolve its written qualifier.
    pub(crate) fn module_candidates_for_path(
        &self,
        site: &PathCompletionSite,
    ) -> anyhow::Result<Vec<DefinitionCompletionCandidate>> {
        let source = site.source();
        let importing_module = match source.scope() {
            IndexedQualifiedPathScope::Body { scope, .. } => {
                let Some(module) = BodyView::new(self.db)
                    .owner_module(scope.body_ir())
                    .context("read qualified path owner module")?
                else {
                    return Ok(Vec::new());
                };
                module
            }
            IndexedQualifiedPathScope::Signature { scope } => scope.context().module,
            IndexedQualifiedPathScope::Import { module } => module,
        };
        let Some(qualifier) = source.module_qualifier() else {
            return Ok(Vec::new());
        };
        self.module_path_candidates(importing_module, qualifier)
    }

    /// Return only macros that can be invoked with `!` from a module-level macro call.
    pub(crate) fn module_macro_candidates(
        &self,
        site: &ModuleMacroCompletionSite,
    ) -> anyhow::Result<Vec<DefinitionCompletionCandidate>> {
        let mut candidates = if let Some(qualifier) = site.qualifier() {
            self.module_path_candidates(site.source().module(), qualifier)
                .context("collect qualified module macro candidates")?
        } else {
            self.unqualified_module_candidates(site.source().module())
                .context("collect unqualified module macro candidates")?
        };
        candidates.retain(|candidate| {
            candidate.kind() == CompletionKind::Macro && candidate.is_invocation_macro()
        });
        Ok(candidates)
    }

    /// Gather module-scope names visible at an unqualified body, signature, or import site.
    ///
    /// Body-local item modules are visited from inner to outer scope before the containing semantic
    /// module. Direct local items suppress same-named non-module rows inherited from an outer map.
    pub(crate) fn module_candidates_for_unqualified(
        &self,
        site: &UnqualifiedCompletionSite,
    ) -> anyhow::Result<Vec<DefinitionCompletionCandidate>> {
        match site.source().scope() {
            IndexedUnqualifiedNameScope::Body { scope, .. } => {
                let body_view = BodyView::new(self.db);
                let mut candidates = Vec::new();

                for (scope_id, module) in body_view
                    .lexical_scope_modules(scope.body_ir(), scope.scope_id())
                    .context("read body-local module scopes")?
                {
                    let direct_item_names = body_view
                        .direct_item_names(scope.body_ir(), scope_id)
                        .context("read direct body-local item names")?;
                    candidates.extend(
                        self.unqualified_module_candidates(module)
                            .context("collect body-local module candidates")?
                            .into_iter()
                            .filter(|candidate| {
                                candidate.kind() == CompletionKind::Module
                                    || !direct_item_names.contains(candidate.label())
                            }),
                    );
                }

                if let Some(module) = body_view
                    .owner_module(scope.body_ir())
                    .context("read unqualified completion owner module")?
                {
                    candidates.extend(
                        self.unqualified_module_candidates(module)
                            .context("collect owner module candidates")?,
                    );
                }

                Ok(candidates)
            }
            IndexedUnqualifiedNameScope::Signature { scope, .. } => {
                self.unqualified_module_candidates(scope.context().module)
            }
            IndexedUnqualifiedNameScope::Import { module, .. } => {
                self.unqualified_module_candidates(*module)
            }
        }
    }

    fn module_path_candidates(
        &self,
        importing_module: ModuleRef,
        qualifier: &Path,
    ) -> anyhow::Result<Vec<DefinitionCompletionCandidate>> {
        let mut candidates = Vec::new();
        for name in NameLookupView::new(self.db)
            .module_names_for_path(importing_module, qualifier)
            .context("read qualified module candidate names")?
        {
            if let Some(candidate) = self.module_candidate(name) {
                candidates.push(candidate);
            }
        }
        Ok(candidates)
    }

    fn unqualified_module_candidates(
        &self,
        module: ModuleRef,
    ) -> anyhow::Result<Vec<DefinitionCompletionCandidate>> {
        let mut candidates = Vec::new();
        for name in NameLookupView::new(self.db)
            .unqualified_module_names(module)
            .context("read unqualified module candidate names")?
        {
            if let Some(candidate) = self.module_candidate(name) {
                candidates.push(candidate);
            }
        }
        Ok(candidates)
    }

    pub(super) fn module_candidate(
        &self,
        name: ModuleScopeName,
    ) -> Option<DefinitionCompletionCandidate> {
        let kind = Self::completion_kind(name.kind())?;
        let function = name.function();
        let target = function
            .map(CompletionTarget::Function)
            .unwrap_or_else(|| CompletionTarget::Declaration(name.declaration()));

        Some(DefinitionCompletionCandidate {
            label: name.label().to_string(),
            namespace: name.namespace(),
            module_origin: Some(name.origin()),
            target,
            kind,
            applicability: CompletionApplicability::Known,
            documentation: name.documentation().map(ToString::to_string),
            function,
            macro_kind: name.macro_kind(),
            import_path: None,
            import_path_len: None,
        })
    }

    pub(super) fn completion_kind(kind: SymbolKind) -> Option<CompletionKind> {
        Some(match kind {
            SymbolKind::Const => CompletionKind::Const,
            SymbolKind::Enum => CompletionKind::Enum,
            SymbolKind::EnumVariant => CompletionKind::EnumVariant,
            SymbolKind::Field => CompletionKind::Field,
            SymbolKind::Function => CompletionKind::Function,
            SymbolKind::Macro => CompletionKind::Macro,
            SymbolKind::Method => CompletionKind::Function,
            SymbolKind::Module => CompletionKind::Module,
            SymbolKind::Static => CompletionKind::Static,
            SymbolKind::Struct => CompletionKind::Struct,
            SymbolKind::Trait => CompletionKind::Trait,
            SymbolKind::TypeAlias => CompletionKind::TypeAlias,
            SymbolKind::Union => CompletionKind::Union,
            SymbolKind::Variable => CompletionKind::Variable,
            SymbolKind::Impl => return None,
        })
    }
}
