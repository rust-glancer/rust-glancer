//! Composite completion-site and module-scope views.
//!
//! Completion renderers need editor-specific policies, but they should not know which frozen
//! storage owns cursor-site scanning or module visibility. This view keeps those storage lookups in
//! one place and exposes completion-ready facts.

use rg_body_ir::{
    DotCompletionSite, PathCompletionSite, RecordFieldCompletionSite, ResolvedFunctionRef,
    UnqualifiedCompletionSite,
};
use rg_def_map::{
    DefId, DefMapPathCompletionSite, DefMapUnqualifiedCompletionSite, ModuleRef, ScopeNamespace,
    VisibleScopeOrigin,
};
use rg_parse::FileId;
use rg_semantic_ir::Documentation;

use crate::{
    api::{Analysis, view::member::MemberView},
    model::{CompletionKind, CompletionTarget},
};

/// Cursor shape recognized before semantic completion rendering.
pub(crate) enum CompletionSite {
    /// Member access, such as `user.na$0`.
    Dot(DotCompletionSite),
    /// Body path position, such as `let value = crate::$0`.
    BodyPath(PathCompletionSite),
    /// Body lexical position, such as `let value = inp$0`.
    BodyUnqualified(UnqualifiedCompletionSite),
    /// Record field position, such as `User { na$0 }`.
    RecordField(RecordFieldCompletionSite),
    /// Import path position, such as `use crate::api::$0`.
    UsePath(DefMapPathCompletionSite),
    /// Import root position, such as `use st$0`.
    UseUnqualified(DefMapUnqualifiedCompletionSite),
}

/// Cheap syntax facts that let completion avoid impossible scanner families.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CompletionSiteSyntax {
    inside_use_item: bool,
    after_dot: bool,
    after_colon_colon: bool,
}

impl CompletionSiteSyntax {
    pub(crate) fn new(inside_use_item: bool, after_dot: bool, after_colon_colon: bool) -> Self {
        Self {
            inside_use_item,
            after_dot,
            after_colon_colon,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum CompletionScopeNamespace {
    Types,
    Values,
    Macros,
}

impl From<ScopeNamespace> for CompletionScopeNamespace {
    fn from(namespace: ScopeNamespace) -> Self {
        match namespace {
            ScopeNamespace::Types => Self::Types,
            ScopeNamespace::Values => Self::Values,
            ScopeNamespace::Macros => Self::Macros,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CompletionScopeOrigin {
    ModuleScope,
    Prelude,
    ExternRoot,
}

impl From<VisibleScopeOrigin> for CompletionScopeOrigin {
    fn from(origin: VisibleScopeOrigin) -> Self {
        match origin {
            VisibleScopeOrigin::ModuleScope => Self::ModuleScope,
            VisibleScopeOrigin::Prelude => Self::Prelude,
            VisibleScopeOrigin::ExternRoot => Self::ExternRoot,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ModuleCompletionCandidate {
    label: String,
    namespace: CompletionScopeNamespace,
    origin: CompletionScopeOrigin,
    target: CompletionTarget,
    kind: CompletionKind,
    documentation: Option<String>,
    function: Option<ResolvedFunctionRef>,
}

impl ModuleCompletionCandidate {
    pub(crate) fn label(&self) -> &str {
        &self.label
    }

    pub(crate) fn namespace(&self) -> CompletionScopeNamespace {
        self.namespace
    }

    pub(crate) fn origin(&self) -> CompletionScopeOrigin {
        self.origin
    }

    pub(crate) fn target(&self) -> CompletionTarget {
        self.target
    }

    pub(crate) fn kind(&self) -> CompletionKind {
        self.kind
    }

    pub(crate) fn documentation(&self) -> Option<&str> {
        self.documentation.as_deref()
    }

    pub(crate) fn function_ref(&self) -> Option<ResolvedFunctionRef> {
        self.function
    }
}

pub(crate) struct CompletionView<'a, 'db> {
    analysis: &'a Analysis<'db>,
}

impl<'a, 'db> CompletionView<'a, 'db> {
    pub(crate) fn new(analysis: &'a Analysis<'db>) -> Self {
        Self { analysis }
    }

    /// Classifies the cursor offset by asking the scanner that owns each syntax shape.
    pub(crate) fn site_at(
        &self,
        target: rg_def_map::TargetRef,
        file_id: FileId,
        offset: u32,
        syntax: Option<CompletionSiteSyntax>,
    ) -> anyhow::Result<Option<CompletionSite>> {
        if let Some(syntax) = syntax {
            if syntax.inside_use_item {
                return self.use_site_at(target, file_id, offset);
            }
            if syntax.after_dot {
                return self.dot_site_at(target, file_id, offset);
            }
            if syntax.after_colon_colon {
                return self.body_path_site_at(target, file_id, offset);
            }
        }

        self.general_site_at(target, file_id, offset)
    }

    pub(crate) fn module_candidates_for_body_path(
        &self,
        site: &PathCompletionSite,
    ) -> anyhow::Result<Vec<ModuleCompletionCandidate>> {
        let Some(body) = self.analysis.body_ir.body_data(site.body)? else {
            return Ok(Vec::new());
        };
        self.module_path_candidates(body.owner_module(), &site.qualifier)
    }

    pub(crate) fn module_candidates_for_use_path(
        &self,
        site: &DefMapPathCompletionSite,
    ) -> anyhow::Result<Vec<ModuleCompletionCandidate>> {
        self.module_path_candidates(site.module, &site.qualifier)
    }

    pub(crate) fn module_candidates_for_body_unqualified(
        &self,
        site: &UnqualifiedCompletionSite,
    ) -> anyhow::Result<Vec<ModuleCompletionCandidate>> {
        let Some(body) = self.analysis.body_ir.body_data(site.body)? else {
            return Ok(Vec::new());
        };
        self.unqualified_module_candidates(body.owner_module())
    }

    pub(crate) fn module_candidates_for_use_unqualified(
        &self,
        site: &DefMapUnqualifiedCompletionSite,
    ) -> anyhow::Result<Vec<ModuleCompletionCandidate>> {
        self.unqualified_module_candidates(site.module)
    }

    fn general_site_at(
        &self,
        target: rg_def_map::TargetRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Option<CompletionSite>> {
        if let Some(site) = self
            .analysis
            .body_ir
            .dot_completion_site(target, file_id, offset)?
        {
            return Ok(Some(CompletionSite::Dot(site)));
        }

        if let Some(site) = self
            .analysis
            .body_ir
            .path_completion_site(target, file_id, offset)?
        {
            return Ok(Some(CompletionSite::BodyPath(site)));
        }

        if let Some(site) = self
            .analysis
            .body_ir
            .record_field_completion_site(target, file_id, offset)?
        {
            return Ok(Some(CompletionSite::RecordField(site)));
        }

        if let Some(site) = self
            .analysis
            .body_ir
            .unqualified_completion_site(target, file_id, offset)?
        {
            return Ok(Some(CompletionSite::BodyUnqualified(site)));
        }

        if let Some(site) = self
            .analysis
            .def_map
            .path_completion_site(target, file_id, offset)?
        {
            return Ok(Some(CompletionSite::UsePath(site)));
        }

        Ok(self
            .analysis
            .def_map
            .unqualified_completion_site(target, file_id, offset)?
            .map(CompletionSite::UseUnqualified))
    }

    fn dot_site_at(
        &self,
        target: rg_def_map::TargetRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Option<CompletionSite>> {
        Ok(self
            .analysis
            .body_ir
            .dot_completion_site(target, file_id, offset)?
            .map(CompletionSite::Dot))
    }

    fn body_path_site_at(
        &self,
        target: rg_def_map::TargetRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Option<CompletionSite>> {
        Ok(self
            .analysis
            .body_ir
            .path_completion_site(target, file_id, offset)?
            .map(CompletionSite::BodyPath))
    }

    fn use_site_at(
        &self,
        target: rg_def_map::TargetRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Option<CompletionSite>> {
        if let Some(site) = self
            .analysis
            .def_map
            .path_completion_site(target, file_id, offset)?
        {
            return Ok(Some(CompletionSite::UsePath(site)));
        }

        Ok(self
            .analysis
            .def_map
            .unqualified_completion_site(target, file_id, offset)?
            .map(CompletionSite::UseUnqualified))
    }

    fn module_path_candidates(
        &self,
        importing_module: ModuleRef,
        qualifier: &rg_def_map::Path,
    ) -> anyhow::Result<Vec<ModuleCompletionCandidate>> {
        let resolved = self
            .analysis
            .def_map
            .resolve_path_in_type_namespace(importing_module, qualifier)?;
        let mut candidates = Vec::new();

        // Module path completion needs a module scope to list. Non-module qualifiers such as type
        // names expose associated items through a different lookup model.
        for def in resolved.resolved {
            let DefId::Module(source_module) = def else {
                continue;
            };
            for visible_def in self
                .analysis
                .def_map
                .visible_scope_defs(importing_module, source_module)?
            {
                if let Some(candidate) = self.module_candidate(visible_def)? {
                    candidates.push(candidate);
                }
            }
        }

        Ok(candidates)
    }

    fn unqualified_module_candidates(
        &self,
        module: ModuleRef,
    ) -> anyhow::Result<Vec<ModuleCompletionCandidate>> {
        let mut candidates = Vec::new();
        for visible_def in self
            .analysis
            .def_map
            .visible_unqualified_scope_defs(module)?
        {
            if let Some(candidate) = self.module_candidate(visible_def)? {
                candidates.push(candidate);
            }
        }
        Ok(candidates)
    }

    fn module_candidate(
        &self,
        visible_def: rg_def_map::VisibleScopeDef,
    ) -> anyhow::Result<Option<ModuleCompletionCandidate>> {
        let mut target = CompletionTarget::Def(visible_def.def);
        let mut function = None;
        let (kind, documentation) = match visible_def.def {
            DefId::Module(module) => {
                let Some(data) = self.analysis.def_map.module(module)? else {
                    return Ok(None);
                };
                (
                    CompletionKind::Module,
                    data.docs.as_ref().map(Documentation::text),
                )
            }
            DefId::Local(local_def) => {
                let Some(data) = self.analysis.def_map.local_def(local_def)? else {
                    return Ok(None);
                };
                let kind = CompletionKind::from_local_def_kind(data.kind);
                if let Some(member_function) =
                    MemberView::new(self.analysis).function_for_local_def(local_def)?
                {
                    let function_ref = member_function.function_ref();
                    target = CompletionTarget::Function(function_ref);
                    function = Some(function_ref);
                }
                (kind, None)
            }
        };

        Ok(Some(ModuleCompletionCandidate {
            label: visible_def.label,
            namespace: visible_def.namespace.into(),
            origin: visible_def.origin.into(),
            target,
            kind,
            documentation,
            function,
        }))
    }
}
