//! Lexical, generic, and primitive scope candidate lookup.

use anyhow::Context as _;
use rg_ir_model::{Path, PrimitiveTy, identity::DeclarationRef};
use rg_ir_view::{
    lookup::name::{
        GenericScopeNameKind, GenericScopeNameTarget, NameLookupView, NameNamespace,
        ValueOrTypeNamespace,
    },
    source::{IndexedTypeNamePosition, IndexedUnqualifiedNameContext, IndexedUnqualifiedNameScope},
    ty::{
        TyView,
        locals::{BodyLexicalName, BodyNameScope, BodyView},
    },
};

use crate::{
    model::{CompletionKind, CompletionTarget},
    query::completion::site::UnqualifiedCompletionSite,
};

use super::{
    CompletionCandidateSource, GenericScopeCompletionCandidate, LexicalCompletionCandidate,
    PrimitiveTypePathScope,
};

impl<'a, 'db> CompletionCandidateSource<'a, 'db> {
    /// Read lexical body names in every namespace accepted by the surrounding grammar.
    ///
    /// Pattern positions ask both value and type scopes: constructors/constants can be completed
    /// directly, while a type name can still become the qualifier of a variant path.
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
        let mut candidates = Vec::new();
        let namespaces: &[ValueOrTypeNamespace] = match context {
            IndexedUnqualifiedNameContext::Type { .. } => &[ValueOrTypeNamespace::Types],
            IndexedUnqualifiedNameContext::Value | IndexedUnqualifiedNameContext::Const => {
                &[ValueOrTypeNamespace::Values]
            }
            // Pattern paths use value constructors and constants, while type names remain useful
            // as qualifiers for enum variants and associated constants.
            IndexedUnqualifiedNameContext::Pattern(_) => {
                &[ValueOrTypeNamespace::Values, ValueOrTypeNamespace::Types]
            }
        };
        for namespace in namespaces {
            let name_scope = BodyNameScope::new(
                scope.body_ir(),
                scope.scope_id(),
                *namespace,
                *visible_bindings,
            );
            for candidate in BodyView::new(self.db)
                .lexical_names(name_scope)
                .context("read lexical completion candidates")?
            {
                let Some(candidate) = self.lexical_candidate(*namespace, candidate) else {
                    continue;
                };
                if candidates
                    .iter()
                    .any(|existing: &LexicalCompletionCandidate| {
                        existing.target() == candidate.target()
                            && existing.label() == candidate.label()
                    })
                {
                    continue;
                }
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
            IndexedUnqualifiedNameScope::Body {
                scope,
                context,
                generic_owner,
                ..
            } => {
                let owner = if let Some(owner) = generic_owner {
                    *owner
                } else {
                    let Some(owner) = BodyView::new(self.db)
                        .generic_owner(scope.body_ir())
                        .context("read completion body generic owner")?
                    else {
                        return Ok(Vec::new());
                    };
                    owner
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
                    IndexedUnqualifiedNameContext::Value | IndexedUnqualifiedNameContext::Const
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
            | IndexedUnqualifiedNameScope::Body {
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
                context: IndexedUnqualifiedNameContext::Value,
                ..
            }
            | IndexedUnqualifiedNameScope::Signature {
                context: IndexedUnqualifiedNameContext::Pattern(_),
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
                    namespace: namespace.into(),
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
}
