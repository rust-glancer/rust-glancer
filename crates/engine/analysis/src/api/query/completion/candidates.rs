//! Completion candidate assembly from generic indexed views.
//!
//! Completion renderers need editor-specific policies, but they should not know which frozen
//! storage owns name, member, or type lookup. This adapter accepts completion-domain cursor sites
//! and projects generic view facts into completion-ready candidates.

use rg_def_map::Path;
use rg_ir_model::{
    ModuleRef,
    identity::{DeclarationRef, EnumVariantRef, FieldRef, FunctionRef},
};
use rg_semantic_ir::FieldKey;
use rg_ty::IndexedTy;

use crate::{
    api::{
        completion_site::{
            DotCompletionSite, PathCompletionSite, RecordFieldCompletionSite,
            UnqualifiedCompletionSite,
        },
        view::{
            IndexedSymbolKind, IndexedViewDb,
            body::{BodyLexicalName, BodyNameNamespace, BodyNameScope, BodyView},
            enum_variant::EnumVariantView,
            member::{MemberMethodOrigin, MemberView},
            name_lookup::{ModuleScopeName, NameLookupView, NameNamespace, NameOrigin},
            source::{
                IndexedNameNamespace, IndexedQualifiedPathScope, IndexedUnqualifiedNameScope,
            },
            ty::TyView,
        },
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

        Ok(EnumVariantView::new(self.db)
            .variants_for_body_type_path(
                scope.body_ir(),
                scope.scope_id(),
                site.source().qualifier(),
            )?
            .into_iter()
            .map(|variant| variant.variant_ref())
            .collect())
    }

    pub(crate) fn module_candidates_for_unqualified(
        &self,
        site: &UnqualifiedCompletionSite,
    ) -> anyhow::Result<Vec<ModuleCompletionCandidate>> {
        let module = match site.source().scope() {
            IndexedUnqualifiedNameScope::Body { scope, .. } => {
                let Some(module) = BodyView::new(self.db).owner_module(scope.body_ir())? else {
                    return Ok(Vec::new());
                };
                module
            }
            IndexedUnqualifiedNameScope::Import { module } => *module,
        };
        self.unqualified_module_candidates(module)
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
    ) -> anyhow::Result<Vec<rg_ty::PrimitiveTy>> {
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

        for primitive in rg_ty::PrimitiveTy::ALL
            .iter()
            .copied()
            .filter(|primitive| primitive.label().starts_with(member_prefix))
        {
            let path = Path::unqualified_name(primitive.label());
            if matches!(
                TyView::new(self.db).ty_for_body_type_path(
                    scope.body_ir(),
                    scope.scope_id(),
                    &path
                )?,
                IndexedTy::Primitive(resolved) if resolved == primitive
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
            BodyView::new(self.db).receiver_ty(receiver.body_ir(), receiver.expr_id())?
        else {
            return Ok(Vec::new());
        };

        let members = MemberView::new(self.db);
        let mut fields = Vec::new();
        for field in members.field_candidates_for_ty(&receiver_ty)? {
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
            BodyView::new(self.db).receiver_ty(receiver.body_ir(), receiver.expr_id())?
        else {
            return Ok(Vec::new());
        };

        let members = MemberView::new(self.db);
        let mut methods = Vec::new();
        for method in members.method_candidates_for_ty(&receiver_ty)? {
            methods.push(Self::dot_method_candidate(method));
        }

        Ok(methods)
    }

    fn module_path_candidates(
        &self,
        importing_module: ModuleRef,
        qualifier: &rg_def_map::Path,
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
                let declaration = DeclarationRef::body_item(item);
                LexicalCompletionCandidate {
                    label,
                    namespace: CompletionScopeNamespace::Types,
                    scope_distance,
                    target: CompletionTarget::Declaration(declaration),
                    kind: CompletionKind::from_body_item_kind(kind),
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
                target: CompletionTarget::Declaration(DeclarationRef::body_value_item(item)),
                kind: CompletionKind::from_body_value_item_kind(kind),
                declaration: Some(DeclarationRef::body_value_item(item)),
                function: None,
                shadow_namespaces: vec![CompletionScopeNamespace::Values],
            },
            BodyLexicalName::Function {
                function,
                label,
                scope_distance,
            } => {
                let function_ref = FunctionRef::body_local(function);
                let declaration = DeclarationRef::body_function(function);
                LexicalCompletionCandidate {
                    label,
                    namespace: CompletionScopeNamespace::Values,
                    scope_distance,
                    target: CompletionTarget::Function(function_ref),
                    kind: CompletionKind::Function,
                    declaration: Some(declaration),
                    function: Some(function_ref),
                    shadow_namespaces: vec![CompletionScopeNamespace::Values],
                }
            }
        };

        Some(candidate)
    }

    fn completion_kind(kind: IndexedSymbolKind) -> Option<CompletionKind> {
        Some(match kind {
            IndexedSymbolKind::Const => CompletionKind::Const,
            IndexedSymbolKind::Enum => CompletionKind::Enum,
            IndexedSymbolKind::EnumVariant => CompletionKind::EnumVariant,
            IndexedSymbolKind::Field => CompletionKind::Field,
            IndexedSymbolKind::Function => CompletionKind::Function,
            IndexedSymbolKind::Macro => CompletionKind::Macro,
            IndexedSymbolKind::Method => CompletionKind::Function,
            IndexedSymbolKind::Module => CompletionKind::Module,
            IndexedSymbolKind::Static => CompletionKind::Static,
            IndexedSymbolKind::Struct => CompletionKind::Struct,
            IndexedSymbolKind::Trait => CompletionKind::Trait,
            IndexedSymbolKind::TypeAlias => CompletionKind::TypeAlias,
            IndexedSymbolKind::Union => CompletionKind::Union,
            IndexedSymbolKind::Variable => CompletionKind::Variable,
            IndexedSymbolKind::Impl => return None,
        })
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
