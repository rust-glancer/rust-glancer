//! Completion candidate assembly from generic indexed views.
//!
//! Completion renderers need editor-specific policies, but they should not know which frozen
//! storage owns name, member, or type lookup. This adapter accepts completion-domain cursor sites
//! and projects generic view facts into completion-ready candidates.

use rg_ir_model::items::{FieldKey, PrimitiveTy};
use rg_ir_model::{
    EnumVariantRef, FieldRef, FunctionRef, ModuleRef, Path, TypeDefId, identity::DeclarationRef,
};
use rg_ir_storage::ItemStoreQuery;
use rg_ir_view::{
    IndexedViewDb, SymbolKind,
    lookup::name::{ModuleScopeName, NameLookupView, NameNamespace, NameOrigin},
    member::{MemberMethodCandidate, MemberUseSite, MemberView},
    source::{IndexedNameNamespace, IndexedQualifiedPathScope, IndexedUnqualifiedNameScope},
    ty::TyView,
    ty::locals::{BodyLexicalName, BodyNameNamespace, BodyNameScope, BodyView},
};
use rg_ty::{MemberMethodOrigin, Ty};

use crate::{
    completion_site::{
        DotCompletionSite, PathCompletionSite, RecordFieldCompletionSite, UnqualifiedCompletionSite,
    },
    model::{CompletionApplicability, CompletionKind, CompletionTarget},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum CompletionScopeNamespace {
    Types,
    Values,
    Macros,
}

impl From<NameNamespace> for CompletionScopeNamespace {
    fn from(namespace: NameNamespace) -> Self {
        match namespace {
            NameNamespace::Types => Self::Types,
            NameNamespace::Values => Self::Values,
            NameNamespace::Macros => Self::Macros,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CompletionScopeOrigin {
    ModuleScope,
    Prelude,
    ExternRoot,
}

impl From<NameOrigin> for CompletionScopeOrigin {
    fn from(origin: NameOrigin) -> Self {
        match origin {
            NameOrigin::ModuleScope => Self::ModuleScope,
            NameOrigin::Prelude => Self::Prelude,
            NameOrigin::ExternRoot => Self::ExternRoot,
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
    function: Option<FunctionRef>,
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

    pub(crate) fn function_ref(&self) -> Option<FunctionRef> {
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
    function: Option<FunctionRef>,
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

    pub(crate) fn function_ref(&self) -> Option<FunctionRef> {
        self.function
    }

    pub(crate) fn shadow_namespaces(&self) -> &[CompletionScopeNamespace] {
        &self.shadow_namespaces
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DotMethodCompletionCandidate {
    function: FunctionRef,
    kind: CompletionKind,
    applicability: CompletionApplicability,
}

impl DotMethodCompletionCandidate {
    pub(crate) fn function_ref(&self) -> FunctionRef {
        self.function
    }

    pub(crate) fn kind(&self) -> CompletionKind {
        self.kind
    }

    pub(crate) fn applicability(&self) -> CompletionApplicability {
        self.applicability
    }
}

pub(crate) struct CompletionCandidateSource<'a, 'db> {
    db: &'a IndexedViewDb<'db>,
}

impl<'a, 'db> CompletionCandidateSource<'a, 'db> {
    pub(crate) fn new(db: &'a IndexedViewDb<'db>) -> Self {
        Self { db }
    }

    pub(crate) fn module_candidates_for_path(
        &self,
        site: &PathCompletionSite,
    ) -> anyhow::Result<Vec<ModuleCompletionCandidate>> {
        let source = site.source();
        let importing_module = match source.scope() {
            IndexedQualifiedPathScope::Body { scope, .. } => {
                let Some(module) = BodyView::new(self.db).owner_module(scope.body_ir())? else {
                    return Ok(Vec::new());
                };
                module
            }
            IndexedQualifiedPathScope::Import { module } => module,
        };
        self.module_path_candidates(importing_module, source.qualifier())
    }

    pub(crate) fn enum_variant_candidates_for_path(
        &self,
        site: &PathCompletionSite,
    ) -> anyhow::Result<Vec<EnumVariantRef>> {
        let IndexedQualifiedPathScope::Body { scope, namespace } = site.source().scope() else {
            return Ok(Vec::new());
        };
        if !matches!(namespace, IndexedNameNamespace::Values) {
            return Ok(Vec::new());
        }

        let ty = TyView::new(self.db).ty_for_body_type_path(
            scope.body_ir(),
            scope.scope_id(),
            site.source().qualifier(),
        )?;
        let item_query = ItemStoreQuery::new(self.db);
        let mut variants = Vec::new();

        for nominal in ty.as_nominals() {
            let ty = nominal.def;
            let TypeDefId::Enum(enum_id) = ty.id else {
                continue;
            };
            let Some(data) = item_query.enum_data_for_type_def(ty)? else {
                continue;
            };
            variants.extend((0..data.variants.len()).map(|index| EnumVariantRef {
                origin: ty.origin,
                enum_id,
                index,
            }));
        }

        Ok(variants)
    }

    pub(crate) fn module_candidates_for_unqualified(
        &self,
        site: &UnqualifiedCompletionSite,
    ) -> anyhow::Result<Vec<ModuleCompletionCandidate>> {
        match site.source().scope() {
            IndexedUnqualifiedNameScope::Body { scope, .. } => {
                let body_view = BodyView::new(self.db);
                let mut candidates = Vec::new();

                for (scope_id, module) in
                    body_view.lexical_scope_modules(scope.body_ir(), scope.scope_id())?
                {
                    let direct_item_names =
                        body_view.direct_item_names(scope.body_ir(), scope_id)?;
                    candidates.extend(
                        self.unqualified_module_candidates(module)?
                            .into_iter()
                            .filter(|candidate| {
                                candidate.kind() == CompletionKind::Module
                                    || !direct_item_names.contains(candidate.label())
                            }),
                    );
                }

                if let Some(module) = body_view.owner_module(scope.body_ir())? {
                    candidates.extend(self.unqualified_module_candidates(module)?);
                }

                Ok(candidates)
            }
            IndexedUnqualifiedNameScope::Import { module } => {
                self.unqualified_module_candidates(*module)
            }
        }
    }

    pub(crate) fn lexical_candidates_for_unqualified(
        &self,
        site: &UnqualifiedCompletionSite,
    ) -> anyhow::Result<Vec<LexicalCompletionCandidate>> {
        let IndexedUnqualifiedNameScope::Body {
            scope,
            namespace,
            visible_bindings,
            ..
        } = site.source().scope()
        else {
            return Ok(Vec::new());
        };
        let body_namespace = match namespace {
            IndexedNameNamespace::Types => BodyNameNamespace::Types,
            IndexedNameNamespace::Values => BodyNameNamespace::Values,
        };
        let scope = BodyNameScope::new(
            scope.body_ir(),
            scope.scope_id(),
            body_namespace,
            *visible_bindings,
        );
        let mut candidates = Vec::new();
        for candidate in BodyView::new(self.db).lexical_names(scope)? {
            if let Some(candidate) = self.lexical_candidate(*namespace, candidate) {
                candidates.push(candidate);
            }
        }
        Ok(candidates)
    }

    pub(crate) fn primitive_type_candidates_for_unqualified(
        &self,
        site: &UnqualifiedCompletionSite,
    ) -> anyhow::Result<Vec<PrimitiveTy>> {
        let IndexedUnqualifiedNameScope::Body {
            scope,
            namespace,
            member_prefix,
            ..
        } = site.source().scope()
        else {
            return Ok(Vec::new());
        };
        if !matches!(namespace, IndexedNameNamespace::Types) {
            return Ok(Vec::new());
        }

        let mut candidates = Vec::new();

        for primitive in PrimitiveTy::ALL
            .iter()
            .copied()
            .filter(|primitive| primitive.label().starts_with(member_prefix.as_str()))
        {
            let path = Path::unqualified_name(primitive.label());
            if matches!(
                TyView::new(self.db).ty_for_body_type_path(
                    scope.body_ir(),
                    scope.scope_id(),
                    &path
                )?,
                Ty::Primitive(resolved) if resolved == primitive
            ) {
                candidates.push(primitive);
            }
        }

        Ok(candidates)
    }

    pub(crate) fn field_candidates_for_dot(
        &self,
        site: &DotCompletionSite,
    ) -> anyhow::Result<Vec<FieldRef>> {
        let receiver = site.source().receiver();
        let Some(receiver_ty) =
            BodyView::new(self.db).expr_ty(receiver.body_ir(), receiver.expr_id())?
        else {
            return Ok(Vec::new());
        };

        let members = MemberView::new(self.db);
        let mut fields = Vec::new();
        for field in members.field_candidates_for_ty(receiver.body_ir().target, &receiver_ty)? {
            fields.push(field.field_ref());
        }

        Ok(fields)
    }

    pub(crate) fn field_candidates_for_record(
        &self,
        site: &RecordFieldCompletionSite,
    ) -> anyhow::Result<Vec<FieldRef>> {
        let site = site.source();
        let scope = site.scope();
        let members = MemberView::new(self.db);
        let mut fields = Vec::new();
        for field in members.field_candidates_for_body_type_path(
            scope.body_ir(),
            scope.scope_id(),
            site.owner(),
        )? {
            let Some(key) = field.key() else {
                continue;
            };
            if !matches!(key, FieldKey::Named(_))
                || site
                    .existing_fields()
                    .iter()
                    .any(|existing| existing == key)
            {
                continue;
            }
            fields.push(field.field_ref());
        }

        Ok(fields)
    }

    pub(crate) fn method_candidates_for_dot(
        &self,
        site: &DotCompletionSite,
    ) -> anyhow::Result<Vec<DotMethodCompletionCandidate>> {
        let receiver = site.source().receiver();
        let Some(receiver_ty) =
            BodyView::new(self.db).expr_ty(receiver.body_ir(), receiver.expr_id())?
        else {
            return Ok(Vec::new());
        };

        let members = MemberView::new(self.db);
        let mut methods = Vec::new();
        for method in members
            .method_candidates_for_ty(MemberUseSite::body(receiver.body_ir()), &receiver_ty)?
        {
            methods.push(Self::dot_method_candidate(method));
        }

        Ok(methods)
    }

    fn module_path_candidates(
        &self,
        importing_module: ModuleRef,
        qualifier: &Path,
    ) -> anyhow::Result<Vec<ModuleCompletionCandidate>> {
        let mut candidates = Vec::new();
        for name in
            NameLookupView::new(self.db).module_names_for_path(importing_module, qualifier)?
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
    ) -> anyhow::Result<Vec<ModuleCompletionCandidate>> {
        let mut candidates = Vec::new();
        for name in NameLookupView::new(self.db).unqualified_module_names(module)? {
            if let Some(candidate) = self.module_candidate(name) {
                candidates.push(candidate);
            }
        }
        Ok(candidates)
    }

    fn module_candidate(&self, name: ModuleScopeName) -> Option<ModuleCompletionCandidate> {
        let kind = Self::completion_kind(name.kind())?;
        let function = name.function();
        let target = function
            .map(CompletionTarget::Function)
            .unwrap_or_else(|| CompletionTarget::Declaration(name.declaration()));

        Some(ModuleCompletionCandidate {
            label: name.label().to_string(),
            namespace: name.namespace().into(),
            origin: name.origin().into(),
            target,
            kind,
            documentation: name.documentation().map(ToString::to_string),
            function,
        })
    }

    fn lexical_candidate(
        &self,
        namespace: IndexedNameNamespace,
        candidate: BodyLexicalName,
    ) -> Option<LexicalCompletionCandidate> {
        let candidate = match candidate {
            BodyLexicalName::Binding {
                binding,
                label,
                scope_distance,
            } => {
                let declaration = DeclarationRef::body_binding(binding);
                LexicalCompletionCandidate {
                    label,
                    namespace: CompletionScopeNamespace::Values,
                    scope_distance,
                    target: CompletionTarget::Declaration(declaration),
                    kind: CompletionKind::Variable,
                    declaration: Some(declaration),
                    function: None,
                    shadow_namespaces: vec![CompletionScopeNamespace::Values],
                }
            }
            BodyLexicalName::TypeItem {
                item,
                kind,
                label,
                scope_distance,
                has_value_constructor,
            } => {
                let mut shadow_namespaces = vec![CompletionScopeNamespace::Types];
                if matches!(namespace, IndexedNameNamespace::Values) && has_value_constructor {
                    shadow_namespaces.push(CompletionScopeNamespace::Values);
                }
                let declaration = DeclarationRef::from(item);
                LexicalCompletionCandidate {
                    label,
                    namespace: CompletionScopeNamespace::Types,
                    scope_distance,
                    target: CompletionTarget::Declaration(declaration),
                    kind: CompletionKind::from_semantic_item_kind(kind)?,
                    declaration: Some(declaration),
                    function: None,
                    shadow_namespaces,
                }
            }
            BodyLexicalName::ValueItem {
                item,
                kind,
                label,
                scope_distance,
            } => LexicalCompletionCandidate {
                label,
                namespace: CompletionScopeNamespace::Values,
                scope_distance,
                target: CompletionTarget::Declaration(DeclarationRef::from(item)),
                kind: CompletionKind::from_semantic_item_kind(kind)?,
                declaration: Some(DeclarationRef::from(item)),
                function: None,
                shadow_namespaces: vec![CompletionScopeNamespace::Values],
            },
            BodyLexicalName::Function {
                function,
                label,
                scope_distance,
            } => {
                let declaration = DeclarationRef::from(function);
                LexicalCompletionCandidate {
                    label,
                    namespace: CompletionScopeNamespace::Values,
                    scope_distance,
                    target: CompletionTarget::Function(function),
                    kind: CompletionKind::Function,
                    declaration: Some(declaration),
                    function: Some(function),
                    shadow_namespaces: vec![CompletionScopeNamespace::Values],
                }
            }
        };

        Some(candidate)
    }

    fn completion_kind(kind: SymbolKind) -> Option<CompletionKind> {
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

    fn dot_method_candidate(method: MemberMethodCandidate<'_>) -> DotMethodCompletionCandidate {
        match method.origin() {
            MemberMethodOrigin::Inherent => DotMethodCompletionCandidate {
                function: method.function().function_ref(),
                kind: CompletionKind::InherentMethod,
                applicability: CompletionApplicability::Known,
            },
            MemberMethodOrigin::Trait { applicability } => DotMethodCompletionCandidate {
                function: method.function().function_ref(),
                kind: CompletionKind::TraitMethod,
                applicability: CompletionApplicability::from(applicability),
            },
        }
    }
}
