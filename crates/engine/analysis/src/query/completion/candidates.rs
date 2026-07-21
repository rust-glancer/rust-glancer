//! Completion candidate assembly from generic indexed views.
//!
//! Completion renderers need editor-specific policies, but they should not know which frozen
//! storage owns name, member, or type lookup. This adapter accepts completion-domain cursor sites
//! and projects generic view facts into completion-ready candidates.

use anyhow::Context as _;
use rg_ir_model::{
    BodyRef, EnumVariantRef, FieldKey, FieldRef, FunctionRef, ModuleRef, Path, PrimitiveTy,
    ScopeId, identity::DeclarationRef,
};
use rg_ir_view::{
    IndexedViewDb, SymbolKind,
    lookup::name::{
        GenericScopeNameKind, GenericScopeNameTarget, ModuleScopeName, NameLookupView,
        NameNamespace, NameOrigin, ValueOrTypeNamespace,
    },
    member::{MemberMethodCandidate, MemberMethodOrigin, MemberUseSite, MemberView},
    source::{
        IndexedQualifiedPathScope, IndexedSignatureTypeScope, IndexedTypeNamePosition,
        IndexedUnqualifiedNameContext, IndexedUnqualifiedNameScope,
    },
    ty::TyView,
    ty::locals::{BodyLexicalName, BodyNameScope, BodyView},
};

use crate::{
    completion_site::{
        DotCompletionSite, PathCompletionSite, RecordFieldCompletionSite, UnqualifiedCompletionSite,
    },
    model::{CompletionApplicability, CompletionKind, CompletionTarget},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ModuleCompletionCandidate {
    label: String,
    namespace: NameNamespace,
    origin: NameOrigin,
    target: CompletionTarget,
    kind: CompletionKind,
    documentation: Option<String>,
    function: Option<FunctionRef>,
}

impl ModuleCompletionCandidate {
    pub(crate) fn label(&self) -> &str {
        &self.label
    }

    pub(crate) fn namespace(&self) -> NameNamespace {
        self.namespace
    }

    pub(crate) fn origin(&self) -> NameOrigin {
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
    namespace: NameNamespace,
    scope_distance: usize,
    target: CompletionTarget,
    kind: CompletionKind,
    declaration: Option<DeclarationRef>,
    function: Option<FunctionRef>,
    shadow_namespaces: Vec<NameNamespace>,
}

impl LexicalCompletionCandidate {
    pub(crate) fn label(&self) -> &str {
        &self.label
    }

    pub(crate) fn namespace(&self) -> NameNamespace {
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

    pub(crate) fn shadow_namespaces(&self) -> &[NameNamespace] {
        &self.shadow_namespaces
    }
}

/// A named type/const parameter or impl `Self` projected out of a declaration's generic scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GenericScopeCompletionCandidate {
    label: String,
    namespace: NameNamespace,
    target: CompletionTarget,
    kind: CompletionKind,
}

impl GenericScopeCompletionCandidate {
    pub(crate) fn label(&self) -> &str {
        &self.label
    }

    pub(crate) fn namespace(&self) -> NameNamespace {
        self.namespace
    }

    pub(crate) fn target(&self) -> CompletionTarget {
        self.target
    }

    pub(crate) fn kind(&self) -> CompletionKind {
        self.kind
    }
}

/// Scope used to verify that a primitive-looking spelling still resolves to the builtin type.
///
/// Primitive candidates are not unconditional keywords: a declaration can shadow their spelling.
/// Body and signature paths use different resolution anchors, so the check keeps that distinction.
#[derive(Debug, Clone, Copy)]
enum PrimitiveTypePathScope {
    Body { body: BodyRef, scope: ScopeId },
    Signature(IndexedSignatureTypeScope),
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

/// Turns normalized cursor sites into completion candidates from the matching indexed views.
///
/// This is the boundary where a site's semantic scope chooses body-local, declaration-generic, or
/// module lookup. Rendering and sort policy stay outside so they can operate on one candidate
/// vocabulary regardless of which frozen store supplied it.
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
            IndexedQualifiedPathScope::Signature { scope } => scope.context().module,
            IndexedQualifiedPathScope::Import { module } => module,
        };
        self.module_path_candidates(importing_module, source.qualifier())
    }

    pub(crate) fn enum_variant_candidates_for_path(
        &self,
        site: &PathCompletionSite,
    ) -> anyhow::Result<Vec<EnumVariantRef>> {
        let IndexedQualifiedPathScope::Body { scope, .. } = site.source().scope() else {
            return Ok(Vec::new());
        };

        MemberView::new(self.db).enum_variant_candidates_for_body_type_path(
            scope.body_ir(),
            scope.scope_id(),
            site.source().qualifier(),
        )
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
            IndexedUnqualifiedNameScope::Signature { scope, .. } => {
                self.unqualified_module_candidates(scope.context().module)
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
            context,
            visible_bindings,
            ..
        } = site.source().scope()
        else {
            return Ok(Vec::new());
        };
        let namespace = match context {
            IndexedUnqualifiedNameContext::Type { .. } => ValueOrTypeNamespace::Types,
            IndexedUnqualifiedNameContext::Value => ValueOrTypeNamespace::Values,
        };
        let scope = BodyNameScope::new(
            scope.body_ir(),
            scope.scope_id(),
            namespace,
            *visible_bindings,
        );
        let mut candidates = Vec::new();
        for candidate in BodyView::new(self.db).lexical_names(scope)? {
            if let Some(candidate) = self.lexical_candidate(namespace, candidate) {
                candidates.push(candidate);
            }
        }
        Ok(candidates)
    }

    /// Returns generic-scope names allowed by the syntax around an unqualified cursor.
    ///
    /// In `fn load<T, const N>()`, an ordinary type position such as `let _: T$0` accepts `T`.
    /// A bare generic argument such as `Array<N$0>` is ambiguous until `Array` resolves, so it can
    /// also accept `N`; a structured type inside an argument remains type-only.
    pub(crate) fn generic_scope_candidates_for_unqualified(
        &self,
        site: &UnqualifiedCompletionSite,
    ) -> anyhow::Result<Vec<GenericScopeCompletionCandidate>> {
        let (owner, context) = match site.source().scope() {
            IndexedUnqualifiedNameScope::Body { scope, context, .. } => {
                let Some(owner) = BodyView::new(self.db)
                    .generic_owner(scope.body_ir())
                    .context("read completion body generic owner")?
                else {
                    return Ok(Vec::new());
                };
                (owner, *context)
            }
            IndexedUnqualifiedNameScope::Signature { scope, context, .. } => {
                (scope.generic_owner(), *context)
            }
            IndexedUnqualifiedNameScope::Import { .. } => return Ok(Vec::new()),
        };

        let mut candidates = Vec::new();
        for name in NameLookupView::new(self.db)
            .generic_scope_names(owner)
            .context("read completion generic scope names")?
        {
            // A bare argument such as `Container<N>` is syntactically ambiguous without the
            // resolved container's parameter list, so both type and const parameters are useful.
            // TODO: Carry the expected generic parameter kind when the enclosing path resolves.
            let accepted = matches!(
                (name.kind(), context),
                (
                    GenericScopeNameKind::Type,
                    IndexedUnqualifiedNameContext::Type { .. }
                ) | (
                    GenericScopeNameKind::Const,
                    IndexedUnqualifiedNameContext::Value
                ) | (
                    GenericScopeNameKind::Const,
                    IndexedUnqualifiedNameContext::Type {
                        position: IndexedTypeNamePosition::BareGenericArgument,
                    },
                )
            );
            if !accepted {
                continue;
            }

            let (namespace, kind) = match name.kind() {
                GenericScopeNameKind::Type => (NameNamespace::Types, CompletionKind::TypeParameter),
                GenericScopeNameKind::Const => (NameNamespace::Values, CompletionKind::Const),
            };
            let target = match name.target() {
                GenericScopeNameTarget::Param(param) => CompletionTarget::GenericParam(param),
                GenericScopeNameTarget::ImplSelf(impl_ref) => CompletionTarget::ImplSelf(impl_ref),
            };
            candidates.push(GenericScopeCompletionCandidate {
                label: name.label().to_string(),
                namespace,
                target,
                kind,
            });
        }

        Ok(candidates)
    }

    /// Returns matching primitive types that resolve as primitives in this exact source scope.
    ///
    /// Resolving each spelling prevents completion from suggesting a builtin where a declaration
    /// with the same name has shadowed it.
    pub(crate) fn primitive_type_candidates_for_unqualified(
        &self,
        site: &UnqualifiedCompletionSite,
    ) -> anyhow::Result<Vec<PrimitiveTy>> {
        let (member_prefix, resolution) = match site.source().scope() {
            IndexedUnqualifiedNameScope::Body {
                scope,
                context: IndexedUnqualifiedNameContext::Type { .. },
                member_prefix,
                ..
            } => (
                member_prefix,
                PrimitiveTypePathScope::Body {
                    body: scope.body_ir(),
                    scope: scope.scope_id(),
                },
            ),
            IndexedUnqualifiedNameScope::Signature {
                scope,
                context: IndexedUnqualifiedNameContext::Type { .. },
                member_prefix,
            } => (member_prefix, PrimitiveTypePathScope::Signature(*scope)),
            IndexedUnqualifiedNameScope::Body {
                context: IndexedUnqualifiedNameContext::Value,
                ..
            }
            | IndexedUnqualifiedNameScope::Signature {
                context: IndexedUnqualifiedNameContext::Value,
                ..
            }
            | IndexedUnqualifiedNameScope::Import { .. } => return Ok(Vec::new()),
        };

        let mut candidates = Vec::new();

        for primitive in PrimitiveTy::ALL
            .iter()
            .copied()
            .filter(|primitive| primitive.label().starts_with(member_prefix.as_str()))
        {
            let path = Path::unqualified_name(primitive.label());
            let ty = match resolution {
                PrimitiveTypePathScope::Body { body, scope } => TyView::new(self.db)
                    .ty_for_body_type_path(body, scope, &path)
                    .context("resolve body primitive type completion")?,
                PrimitiveTypePathScope::Signature(scope) => TyView::new(self.db)
                    .ty_for_type_path(scope.context(), &path)
                    .context("resolve signature primitive type completion")?,
            };
            if ty.primitive() == Some(primitive) {
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
        for field in members.field_candidates_for_ty(receiver.body_ir().crate_ref, &receiver_ty)? {
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
            namespace: name.namespace(),
            origin: name.origin(),
            target,
            kind,
            documentation: name.documentation().map(ToString::to_string),
            function,
        })
    }

    fn lexical_candidate(
        &self,
        namespace: ValueOrTypeNamespace,
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
                    namespace: NameNamespace::Values,
                    scope_distance,
                    target: CompletionTarget::Declaration(declaration),
                    kind: CompletionKind::Variable,
                    declaration: Some(declaration),
                    function: None,
                    shadow_namespaces: vec![NameNamespace::Values],
                }
            }
            BodyLexicalName::TypeItem {
                item,
                kind,
                label,
                scope_distance,
                has_value_constructor,
            } => {
                let mut shadow_namespaces = vec![NameNamespace::Types];
                if matches!(namespace, ValueOrTypeNamespace::Values) && has_value_constructor {
                    shadow_namespaces.push(NameNamespace::Values);
                }
                let declaration = DeclarationRef::from(item);
                LexicalCompletionCandidate {
                    label,
                    namespace: NameNamespace::Types,
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
                namespace: NameNamespace::Values,
                scope_distance,
                target: CompletionTarget::Declaration(DeclarationRef::from(item)),
                kind: CompletionKind::from_semantic_item_kind(kind)?,
                declaration: Some(DeclarationRef::from(item)),
                function: None,
                shadow_namespaces: vec![NameNamespace::Values],
            },
            BodyLexicalName::Function {
                function,
                label,
                scope_distance,
            } => {
                let declaration = DeclarationRef::from(function);
                LexicalCompletionCandidate {
                    label,
                    namespace: NameNamespace::Values,
                    scope_distance,
                    target: CompletionTarget::Function(function),
                    kind: CompletionKind::Function,
                    declaration: Some(declaration),
                    function: Some(function),
                    shadow_namespaces: vec![NameNamespace::Values],
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
