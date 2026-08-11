//! Associated-item and pattern-constructor candidate lookup.

use anyhow::Context as _;
use rg_ir_model::{EnumVariantRef, TypeDefRef, identity::DeclarationRef};
use rg_ir_view::{
    lookup::name::NameNamespace,
    member::{ConstructorShape, MemberAssociatedItem, MemberView},
    source::{
        IndexedAssociatedTypeBindingScope, IndexedAssociatedTypeBindingSite,
        IndexedQualifiedPathScope, IndexedUnqualifiedNameContext, IndexedUnqualifiedNameScope,
    },
    ty::{TyView, locals::BodyView},
};

use crate::{
    model::{CompletionApplicability, CompletionKind, CompletionTarget},
    query::completion::site::{PathCompletionSite, UnqualifiedCompletionSite},
};

use super::{CompletionCandidateSource, DefinitionCompletionCandidate};

impl<'a, 'db> CompletionCandidateSource<'a, 'db> {
    /// Resolve the qualifier as a type/trait and return its associated declarations.
    ///
    /// Body and signature sites can expose functions, consts, types, and enum variants. Import
    /// sites deliberately expose only enum variants because other associated items cannot be
    /// imported through a type path.
    pub(crate) fn associated_definition_candidates_for_path(
        &self,
        site: &PathCompletionSite,
    ) -> anyhow::Result<Vec<DefinitionCompletionCandidate>> {
        let members = MemberView::new(self.db);
        let candidates = match site.source().scope() {
            IndexedQualifiedPathScope::Body { scope, .. } => {
                let Some(qualifier) = site.source().associated_qualifier() else {
                    return Ok(Vec::new());
                };
                members
                    .associated_item_candidates_for_body(
                        scope.body_ir(),
                        scope.scope_id(),
                        qualifier,
                    )
                    .context("collect body associated path candidates")?
            }
            IndexedQualifiedPathScope::Signature { scope } => {
                let Some(qualifier) = site.source().associated_qualifier() else {
                    return Ok(Vec::new());
                };
                members
                    .associated_item_candidates_for_signature(scope, qualifier)
                    .context("collect signature associated path candidates")?
            }
            IndexedQualifiedPathScope::Import { module } => {
                let Some(qualifier) = site.source().module_qualifier() else {
                    return Ok(Vec::new());
                };
                members
                    .associated_enum_variants_for_import(module, qualifier)
                    .context("collect import enum variant candidates")?
            }
        };

        self.definition_candidates_from_associated(&members, candidates)
    }

    /// Return only associated type declarations valid as binding names for the selected trait.
    pub(crate) fn associated_type_binding_candidates(
        &self,
        site: &IndexedAssociatedTypeBindingSite,
    ) -> anyhow::Result<Vec<DefinitionCompletionCandidate>> {
        let members = MemberView::new(self.db);
        let candidates = match site.scope() {
            IndexedAssociatedTypeBindingScope::Body { scope } => members
                .trait_associated_item_candidates_for_body(
                    scope.body_ir(),
                    scope.scope_id(),
                    site.trait_ref(),
                )
                .context("collect body associated type binding candidates")?,
            IndexedAssociatedTypeBindingScope::Signature { scope } => members
                .trait_associated_item_candidates_for_signature(scope, site.trait_ref())
                .context("collect signature associated type binding candidates")?,
        };
        let mut definitions = self
            .definition_candidates_from_associated(&members, candidates)
            .context("project associated type binding candidates")?;
        definitions.retain(|candidate| {
            candidate.kind() == CompletionKind::TypeAlias
                && !site
                    .existing_bindings()
                    .iter()
                    .any(|name| name == candidate.label())
        });
        Ok(definitions)
    }

    /// Normalize member-view results into the definition vocabulary shared by all renderers.
    fn definition_candidates_from_associated(
        &self,
        members: &MemberView<'_, '_>,
        candidates: Vec<rg_ir_view::member::MemberAssociatedItemCandidate>,
    ) -> anyhow::Result<Vec<DefinitionCompletionCandidate>> {
        let mut definitions = Vec::new();
        for candidate in candidates {
            let Some(definition) = members
                .associated_item_definition(candidate)
                .context("read associated completion definition")?
            else {
                continue;
            };
            let Some(kind) = Self::completion_kind(definition.kind()) else {
                continue;
            };
            let item = definition.candidate().item();
            let (namespace, target, function) = match item {
                MemberAssociatedItem::Function(function) => (
                    NameNamespace::Values,
                    CompletionTarget::Function(function),
                    Some(function),
                ),
                MemberAssociatedItem::TypeAlias(alias) => (
                    NameNamespace::Types,
                    CompletionTarget::Declaration(DeclarationRef::from(alias)),
                    None,
                ),
                MemberAssociatedItem::Const(konst) => (
                    NameNamespace::Values,
                    CompletionTarget::Declaration(DeclarationRef::from(konst)),
                    None,
                ),
                MemberAssociatedItem::EnumVariant(variant) => {
                    let namespace = match members
                        .enum_variant(variant)
                        .context("read associated enum variant candidate")?
                        .map(|variant| variant.constructor_shape())
                    {
                        Some(ConstructorShape::Record { .. }) => NameNamespace::Types,
                        Some(ConstructorShape::Unit | ConstructorShape::Tuple { .. }) | None => {
                            NameNamespace::Values
                        }
                    };
                    (namespace, CompletionTarget::EnumVariant(variant), None)
                }
            };
            definitions.push(DefinitionCompletionCandidate {
                label: definition.label().to_string(),
                namespace,
                module_origin: None,
                target,
                kind,
                applicability: CompletionApplicability::from(
                    definition.candidate().applicability(),
                ),
                documentation: definition.documentation().map(ToString::to_string),
                function,
                macro_kind: None,
                import_path: None,
                import_path_len: None,
            });
        }
        Ok(definitions)
    }

    /// Return variants of the expected enum for an unresolved unqualified pattern name.
    pub(crate) fn expected_enum_variants_for_unqualified_pattern(
        &self,
        site: &UnqualifiedCompletionSite,
    ) -> anyhow::Result<Vec<EnumVariantRef>> {
        let IndexedUnqualifiedNameScope::Body {
            context: IndexedUnqualifiedNameContext::Pattern(_),
            expected_type_binding: Some(binding),
            ..
        } = site.source().scope()
        else {
            return Ok(Vec::new());
        };
        let Some(expected_ty) = BodyView::new(self.db)
            .binding_ty(*binding)
            .context("read expected pattern binding type")?
        else {
            return Ok(Vec::new());
        };
        MemberView::new(self.db)
            .enum_variant_candidates_for_ty(&expected_ty)
            .context("collect expected enum variants")
    }

    /// Project a completion target back to its nominal type identity when it names one.
    ///
    /// Expected enum variants use this to retain the visible spelling of an imported or
    /// body-local enum instead of inventing a canonical path that may not resolve at the cursor.
    pub(crate) fn type_def_for_target(
        &self,
        target: CompletionTarget,
    ) -> anyhow::Result<Option<TypeDefRef>> {
        let declaration = match target {
            CompletionTarget::Declaration(declaration) => declaration,
            CompletionTarget::EnumVariant(_)
            | CompletionTarget::EnumVariantField(_)
            | CompletionTarget::Field(_)
            | CompletionTarget::Function(_)
            | CompletionTarget::GenericParam(_)
            | CompletionTarget::ImplSelf(_)
            | CompletionTarget::Keyword(_)
            | CompletionTarget::PrimitiveType(_)
            | CompletionTarget::Synthetic(_) => return Ok(None),
        };
        let Some(ty) = TyView::new(self.db)
            .ty_for_declaration(declaration)
            .context("resolve completion target type")?
        else {
            return Ok(None);
        };
        Ok(ty.unique_nominal_type_def().into_option())
    }

    /// Return constructor shape for a candidate that can introduce a pattern constructor.
    pub(crate) fn pattern_constructor_shape(
        &self,
        target: CompletionTarget,
    ) -> anyhow::Result<Option<ConstructorShape>> {
        let members = MemberView::new(self.db);
        match target {
            CompletionTarget::Declaration(declaration) => members
                .constructor_shape_for_declaration(declaration)
                .context("read declaration constructor shape"),
            CompletionTarget::EnumVariant(variant) => Ok(members
                .enum_variant(variant)
                .context("read enum variant constructor shape")?
                .map(|variant| variant.constructor_shape())),
            CompletionTarget::EnumVariantField(_)
            | CompletionTarget::Field(_)
            | CompletionTarget::Function(_)
            | CompletionTarget::GenericParam(_)
            | CompletionTarget::ImplSelf(_)
            | CompletionTarget::Keyword(_)
            | CompletionTarget::PrimitiveType(_)
            | CompletionTarget::Synthetic(_) => Ok(None),
        }
    }
}
