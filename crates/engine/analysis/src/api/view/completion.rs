//! Composite completion-site and module-scope views.
//!
//! Completion renderers need editor-specific policies, but they should not know which frozen
//! storage owns cursor-site scanning or module visibility. This view keeps those storage lookups in
//! one place and exposes completion-ready facts.

use rg_body_ir::{
    BodyAutoderef, BodyAutoderefMode, BodyBindingRef, BodyDeclarationRef, BodyTypePathResolution,
    BodyUnqualifiedCompletionCandidate, DotCompletionSite, FieldKey, PathCompletionSite,
    RecordFieldCompletionSite, ResolvedFieldRef, ResolvedFunctionRef, UnqualifiedCompletionSite,
};
use rg_def_map::{
    DefId, DefMapPathCompletionSite, DefMapUnqualifiedCompletionSite, ModuleRef, Path,
    ScopeNamespace, VisibleScopeOrigin,
};
use rg_parse::FileId;
use rg_semantic_ir::Documentation;

use crate::{
    api::{
        Analysis,
        view::{
            declaration::DeclarationRef,
            member::{MemberMethodOrigin, MemberOwnerRef, MemberReceiverTy, MemberView},
        },
    },
    model::{CompletionApplicability, CompletionKind, CompletionTarget},
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LexicalCompletionCandidate {
    label: String,
    namespace: CompletionScopeNamespace,
    scope_distance: usize,
    target: CompletionTarget,
    kind: CompletionKind,
    declaration: Option<DeclarationRef>,
    function: Option<ResolvedFunctionRef>,
    shadow_namespaces: Vec<CompletionScopeNamespace>,
}

impl LexicalCompletionCandidate {
    pub(crate) fn label(&self) -> &str {
        &self.label
    }

    pub(crate) fn namespace(&self) -> CompletionScopeNamespace {
        self.namespace
    }

    pub(crate) fn scope_distance(&self) -> usize {
        self.scope_distance
    }

    pub(crate) fn target(&self) -> CompletionTarget {
        self.target
    }

    pub(crate) fn kind(&self) -> CompletionKind {
        self.kind
    }

    pub(crate) fn declaration_ref(&self) -> Option<DeclarationRef> {
        self.declaration
    }

    pub(crate) fn function_ref(&self) -> Option<ResolvedFunctionRef> {
        self.function
    }

    pub(crate) fn shadow_namespaces(&self) -> &[CompletionScopeNamespace] {
        &self.shadow_namespaces
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DotMethodCompletionCandidate {
    function: ResolvedFunctionRef,
    kind: CompletionKind,
    applicability: CompletionApplicability,
}

impl DotMethodCompletionCandidate {
    pub(crate) fn function_ref(&self) -> ResolvedFunctionRef {
        self.function
    }

    pub(crate) fn kind(&self) -> CompletionKind {
        self.kind
    }

    pub(crate) fn applicability(&self) -> CompletionApplicability {
        self.applicability
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

    pub(crate) fn lexical_candidates_for_unqualified(
        &self,
        site: &UnqualifiedCompletionSite,
    ) -> anyhow::Result<Vec<LexicalCompletionCandidate>> {
        let mut candidates = Vec::new();
        for candidate in self
            .analysis
            .body_ir
            .unqualified_completion_candidates(site)?
        {
            if let Some(candidate) = self.lexical_candidate(site, candidate)? {
                candidates.push(candidate);
            }
        }
        Ok(candidates)
    }

    pub(crate) fn primitive_type_candidates_for_unqualified(
        &self,
        site: &UnqualifiedCompletionSite,
    ) -> anyhow::Result<Vec<rg_ty::PrimitiveTy>> {
        let mut candidates = Vec::new();

        for primitive in rg_ty::PrimitiveTy::ALL
            .iter()
            .copied()
            .filter(|primitive| primitive.label().starts_with(&site.member_prefix))
        {
            let path = Path::unqualified_name(primitive.label());
            let resolution = self.analysis.body_ir.resolve_type_path_in_scope(
                &self.analysis.def_map,
                &self.analysis.semantic_ir,
                site.body,
                site.scope,
                &path,
            )?;
            if resolution.is_primitive(&primitive) {
                candidates.push(primitive);
            }
        }

        Ok(candidates)
    }

    pub(crate) fn field_candidates_for_dot(
        &self,
        site: &DotCompletionSite,
    ) -> anyhow::Result<Vec<ResolvedFieldRef>> {
        let Some(receiver_ty) = self.analysis.body_ir.receiver_ty(*site)? else {
            return Ok(Vec::new());
        };

        let autoderef = BodyAutoderef::new(&self.analysis.def_map, &self.analysis.semantic_ir);
        let members = MemberView::new(self.analysis);
        let mut fields = Vec::new();

        for candidate in autoderef.candidates(BodyAutoderefMode::FieldLookup, receiver_ty) {
            let candidate = candidate?;
            for ty in MemberReceiverTy::in_body_ty(candidate.ty()) {
                for field in members.field_candidates(ty)? {
                    fields.push(field.field_ref());
                }
            }
        }

        Ok(fields)
    }

    pub(crate) fn field_candidates_for_record(
        &self,
        site: &RecordFieldCompletionSite,
    ) -> anyhow::Result<Vec<ResolvedFieldRef>> {
        let resolution = self.analysis.body_ir.resolve_type_path_in_scope(
            &self.analysis.def_map,
            &self.analysis.semantic_ir,
            site.body,
            site.scope,
            &site.owner,
        )?;
        let owners: Vec<_> = match resolution {
            BodyTypePathResolution::BodyLocal(item) => vec![MemberOwnerRef::BodyLocal(item)],
            BodyTypePathResolution::SelfType(types) | BodyTypePathResolution::TypeDefs(types) => {
                types.into_iter().map(MemberOwnerRef::Semantic).collect()
            }
            BodyTypePathResolution::Primitive(_)
            | BodyTypePathResolution::Traits(_)
            | BodyTypePathResolution::Unknown => Vec::new(),
        };

        let members = MemberView::new(self.analysis);
        let mut fields = Vec::new();
        for owner in owners {
            for field in members.field_candidates_for_owner(owner)? {
                let Some(key) = field.key() else {
                    continue;
                };
                if !matches!(key, FieldKey::Named(_))
                    || site.existing_fields.iter().any(|existing| existing == key)
                {
                    continue;
                }
                fields.push(field.field_ref());
            }
        }

        Ok(fields)
    }

    pub(crate) fn method_candidates_for_dot(
        &self,
        site: &DotCompletionSite,
    ) -> anyhow::Result<Vec<DotMethodCompletionCandidate>> {
        let Some(receiver_ty) = self.analysis.body_ir.receiver_ty(*site)? else {
            return Ok(Vec::new());
        };

        let autoderef = BodyAutoderef::new(&self.analysis.def_map, &self.analysis.semantic_ir);
        let members = MemberView::new(self.analysis);
        let mut methods = Vec::new();

        for candidate in autoderef.candidates(BodyAutoderefMode::MethodReceiver, receiver_ty) {
            let candidate = candidate?;
            for ty in MemberReceiverTy::in_body_ty(candidate.ty()) {
                for method in members.method_candidates(ty)? {
                    methods.push(Self::dot_method_candidate(method));
                }
            }
        }

        Ok(methods)
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

    fn lexical_candidate(
        &self,
        site: &UnqualifiedCompletionSite,
        candidate: BodyUnqualifiedCompletionCandidate,
    ) -> anyhow::Result<Option<LexicalCompletionCandidate>> {
        let candidate = match candidate {
            BodyUnqualifiedCompletionCandidate::Binding {
                binding,
                label,
                scope_distance,
            } => {
                let binding = BodyBindingRef {
                    body: site.body,
                    binding,
                };
                LexicalCompletionCandidate {
                    label,
                    namespace: CompletionScopeNamespace::Values,
                    scope_distance,
                    target: CompletionTarget::Binding {
                        body: binding.body,
                        binding: binding.binding,
                    },
                    kind: CompletionKind::Variable,
                    declaration: Some(binding.into()),
                    function: None,
                    shadow_namespaces: vec![CompletionScopeNamespace::Values],
                }
            }
            BodyUnqualifiedCompletionCandidate::LocalItem {
                item,
                kind,
                label,
                scope_distance,
            } => {
                let Some(body) = self.analysis.body_ir.body_data(item.body)? else {
                    return Ok(None);
                };
                let Some(data) = body.local_item(item.item) else {
                    return Ok(None);
                };
                let mut shadow_namespaces = vec![CompletionScopeNamespace::Types];
                if matches!(
                    site.namespace,
                    rg_body_ir::UnqualifiedCompletionNamespace::Values
                ) && data.has_value_constructor()
                {
                    shadow_namespaces.push(CompletionScopeNamespace::Values);
                }
                LexicalCompletionCandidate {
                    label,
                    namespace: CompletionScopeNamespace::Types,
                    scope_distance,
                    target: CompletionTarget::BodyItem(item),
                    kind: CompletionKind::from_body_item_kind(kind),
                    declaration: Some(item.into()),
                    function: None,
                    shadow_namespaces,
                }
            }
            BodyUnqualifiedCompletionCandidate::LocalValueItem {
                item,
                kind,
                label,
                scope_distance,
            } => LexicalCompletionCandidate {
                label,
                namespace: CompletionScopeNamespace::Values,
                scope_distance,
                target: CompletionTarget::BodyValueItem(item),
                kind: CompletionKind::from_body_value_item_kind(kind),
                declaration: Some(item.into()),
                function: None,
                shadow_namespaces: vec![CompletionScopeNamespace::Values],
            },
            BodyUnqualifiedCompletionCandidate::LocalFunction {
                function,
                label,
                scope_distance,
            } => {
                let function_ref = ResolvedFunctionRef::BodyLocal(function);
                LexicalCompletionCandidate {
                    label,
                    namespace: CompletionScopeNamespace::Values,
                    scope_distance,
                    target: CompletionTarget::Function(function_ref),
                    kind: CompletionKind::Function,
                    declaration: Some(BodyDeclarationRef::Function(function).into()),
                    function: Some(function_ref),
                    shadow_namespaces: vec![CompletionScopeNamespace::Values],
                }
            }
        };

        Ok(Some(candidate))
    }

    fn dot_method_candidate(
        method: crate::api::view::member::MemberMethodCandidate<'_>,
    ) -> DotMethodCompletionCandidate {
        match method.origin {
            MemberMethodOrigin::Inherent => DotMethodCompletionCandidate {
                function: method.function.function_ref(),
                kind: CompletionKind::InherentMethod,
                applicability: CompletionApplicability::Known,
            },
            MemberMethodOrigin::Trait { applicability } => DotMethodCompletionCandidate {
                function: method.function.function_ref(),
                kind: CompletionKind::TraitMethod,
                applicability: CompletionApplicability::from(applicability),
            },
        }
    }
}
