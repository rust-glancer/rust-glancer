//! Associated declarations resolved from body, signature, and import qualifiers.

use anyhow::Context as _;
use rg_body_ir::BodyAssociatedPathPrefix;
use rg_ir_model::{BodyRef, EnumVariantRef, Path, ScopeId, TraitApplicability, TypeDefId};
use rg_item_tree::Documentation;
use rg_semantic_ir::{ItemStoreQuery, TypePathResolution};
use rg_ty::{
    AssociatedItemCandidateRef, AssociatedItemQuery, AssociatedItemRef, ItemPathQuery,
    SemanticSignatureQuery, Ty, TyContext, TypeLoweringAnchor, TypeLoweringEnv, TypeLoweringQuery,
};

use super::{
    MemberAssociatedItem, MemberAssociatedItemCandidate, MemberAssociatedItemDefinition, MemberView,
};
use crate::{
    SymbolKind,
    body::BodyResolutionView,
    source::{IndexedAssociatedPathQualifier, IndexedSignatureTypeScope},
};

impl<'a, 'db> MemberView<'a, 'db> {
    /// Return associated declarations for a body qualifier, including local impl overlays.
    ///
    /// `Widget::` can contribute enum variants, inherent items, and items from implemented traits;
    /// `T::` uses bounds visible in the body; `<T as Factory>::` uses the selected trait hierarchy.
    /// Body-local impls participate even though they are not part of the crate-level item store.
    pub fn associated_item_candidates_for_body(
        &self,
        body: BodyRef,
        scope: ScopeId,
        qualifier: &IndexedAssociatedPathQualifier,
    ) -> anyhow::Result<Vec<MemberAssociatedItemCandidate>> {
        let prefix = Self::body_associated_prefix(qualifier);
        let Some(candidates) = BodyResolutionView::new(self.db)
            .associated_item_candidate_refs(body, scope, &prefix)
            .context("resolve body associated item candidates")?
        else {
            return Ok(Vec::new());
        };
        Ok(Self::project_associated_candidates(candidates))
    }

    /// Return declarations from the trait used by a body-owned associated binding.
    ///
    /// For `Iterator<It$0 = u8>`, `Iterator` is a trait constraint rather than an associated-path
    /// receiver. Only that trait and its supertraits contribute names; inherent items and enum
    /// variants do not.
    pub fn trait_associated_item_candidates_for_body(
        &self,
        body: BodyRef,
        scope: ScopeId,
        trait_ref: &rg_item_tree::TypeRef,
    ) -> anyhow::Result<Vec<MemberAssociatedItemCandidate>> {
        let Some(candidates) = BodyResolutionView::new(self.db)
            .trait_associated_item_candidate_refs(body, scope, trait_ref)
            .context("resolve body trait associated item candidates")?
        else {
            return Ok(Vec::new());
        };
        Ok(Self::project_associated_candidates(candidates))
    }

    /// Return associated declarations for a type-shaped qualifier in an item signature.
    ///
    /// The written prefix determines which semantic sources can contribute:
    ///
    /// ```text
    /// Widget::            -> inherent items and traits implemented by Widget
    /// T::                 -> traits from bounds such as `T: Factory`
    /// Factory::           -> items declared by Factory and its supertraits
    /// <T as Factory>::    -> items from the explicitly selected trait hierarchy
    /// ```
    ///
    /// These sources may overlap. This view merges duplicate identities while projecting the type
    /// engine's refs; completion applies context filtering to the projected candidates afterward.
    pub fn associated_item_candidates_for_signature(
        &self,
        scope: IndexedSignatureTypeScope,
        qualifier: &IndexedAssociatedPathQualifier,
    ) -> anyhow::Result<Vec<MemberAssociatedItemCandidate>> {
        let use_site = scope.context().module.origin.origin_crate();
        let item_lookup_query = self
            .db
            .item_lookup_query(use_site)
            .context("assemble signature associated item lookup")?;
        let ty_context = TyContext::new(
            self.db,
            self.db,
            item_lookup_query,
            self.db.trait_selection(use_site),
        );
        let query = AssociatedItemQuery::new(ty_context);
        let item_paths = ItemPathQuery::new(self.db, self.db);
        let lowering = TypeLoweringQuery::new(&item_paths, &item_paths);
        let env = TypeLoweringEnv::new(
            scope.generic_owner(),
            TypeLoweringAnchor::Context(scope.context()),
        );

        let mut candidates = Vec::new();
        match qualifier {
            IndexedAssociatedPathQualifier::Type(prefix_ty_ref) => {
                let prefix_ty = lowering
                    .lower(prefix_ty_ref, env.clone())
                    .context("lower associated path type qualifier")?;

                // A concrete nominal prefix contributes enum variants, inherent items, and items
                // from matching trait impls.
                for receiver_ty in prefix_ty.as_adts() {
                    candidates.extend(
                        query
                            .candidates_for_nominal(receiver_ty)
                            .context("resolve nominal associated item candidates")?,
                    );
                }

                // A generic prefix has no nominal impl universe of its own. Bounds written on the
                // owning declaration, such as `T: Factory`, supply its associated-item surface.
                let mut session = lowering
                    .session(env)
                    .context("create associated path lowering session")?;
                let owner_traits = session
                    .trait_applications_for_type(&prefix_ty)
                    .context("resolve associated path owner traits")?;
                candidates.extend(
                    query
                        .candidates_for_trait_applications(owner_traits, TraitApplicability::Yes)
                        .context("resolve owner trait associated item candidates")?,
                );

                // The prefix spelling may itself name a trait. This handles `Factory::Item`
                // independently of whether the lowered type also looks nominal or generic.
                if let Some(direct_trait) = session
                    .lower_trait_ref(prefix_ty_ref, prefix_ty.clone())
                    .context("lower direct associated path trait")?
                {
                    candidates.extend(
                        query
                            .candidates_for_trait_applications(
                                [direct_trait.application],
                                TraitApplicability::Yes,
                            )
                            .context("resolve direct trait associated item candidates")?,
                    );
                }
            }
            IndexedAssociatedPathQualifier::QualifiedTrait { self_ty, trait_ref } => {
                let self_ty = lowering
                    .lower(self_ty, env.clone())
                    .context("lower qualified associated path self type")?;
                let mut session = lowering
                    .session(env)
                    .context("create qualified associated path session")?;
                if let Some(trait_ref) = session
                    .lower_trait_ref(trait_ref, self_ty)
                    .context("lower qualified associated path trait")?
                {
                    candidates.extend(
                        query
                            .candidates_for_trait_applications(
                                [trait_ref.application],
                                TraitApplicability::Yes,
                            )
                            .context("resolve qualified trait associated item candidates")?,
                    );
                }
            }
        }

        Ok(Self::project_associated_candidates(candidates))
    }

    /// Return declarations from the trait used by a signature-owned associated binding.
    ///
    /// This is the declaration-signature form of `Iterator<It$0 = u8>`. The signature scope
    /// supplies module names, impl `Self`, and generic parameters while candidate lookup remains
    /// limited to the resolved trait and its supertraits.
    pub fn trait_associated_item_candidates_for_signature(
        &self,
        scope: IndexedSignatureTypeScope,
        trait_ref: &rg_item_tree::TypeRef,
    ) -> anyhow::Result<Vec<MemberAssociatedItemCandidate>> {
        let use_site = scope.context().module.origin.origin_crate();
        let item_lookup_query = self
            .db
            .item_lookup_query(use_site)
            .context("assemble trait binding item lookup")?;
        let ty_context = TyContext::new(
            self.db,
            self.db,
            item_lookup_query,
            self.db.trait_selection(use_site),
        );
        let query = AssociatedItemQuery::new(ty_context);
        let item_paths = ItemPathQuery::new(self.db, self.db);
        let lowering = TypeLoweringQuery::new(&item_paths, &item_paths);
        let env = TypeLoweringEnv::new(
            scope.generic_owner(),
            TypeLoweringAnchor::Context(scope.context()),
        );
        let mut session = lowering
            .session(env)
            .context("create trait binding lowering session")?;
        let Some(trait_ref) = session
            .lower_trait_ref(trait_ref, Ty::Unknown)
            .context("lower trait binding qualifier")?
        else {
            return Ok(Vec::new());
        };
        Ok(Self::project_associated_candidates(
            query
                .candidates_for_trait_applications([trait_ref.application], TraitApplicability::Yes)
                .context("resolve trait binding associated item candidates")?,
        ))
    }

    /// Resolve one associated candidate into label, kind, and documentation facts.
    pub fn associated_item_definition(
        &self,
        candidate: MemberAssociatedItemCandidate,
    ) -> anyhow::Result<Option<MemberAssociatedItemDefinition>> {
        let (label, kind, documentation) = match candidate.item() {
            MemberAssociatedItem::Function(function) => {
                let Some(function) = self
                    .function(function)
                    .context("read associated function definition")?
                else {
                    return Ok(None);
                };
                (
                    function.name().to_string(),
                    function.symbol_kind(),
                    function.docs_text(),
                )
            }
            MemberAssociatedItem::TypeAlias(alias) => {
                let Some(data) = ItemStoreQuery::new(self.db)
                    .type_alias_data(alias)
                    .context("read associated type alias definition")?
                else {
                    return Ok(None);
                };
                (
                    data.name.to_string(),
                    SymbolKind::TypeAlias,
                    data.docs.as_ref().map(Documentation::text),
                )
            }
            MemberAssociatedItem::Const(konst) => {
                let Some(data) = ItemStoreQuery::new(self.db)
                    .const_data(konst)
                    .context("read associated const definition")?
                else {
                    return Ok(None);
                };
                (
                    data.name.to_string(),
                    SymbolKind::Const,
                    data.docs.as_ref().map(Documentation::text),
                )
            }
            MemberAssociatedItem::EnumVariant(variant) => {
                let Some(variant) = self
                    .enum_variant(variant)
                    .context("read associated enum variant definition")?
                else {
                    return Ok(None);
                };
                (
                    variant.label().to_string(),
                    SymbolKind::EnumVariant,
                    variant.docs_text(),
                )
            }
        };

        Ok(Some(MemberAssociatedItemDefinition {
            candidate,
            label,
            kind,
            documentation,
        }))
    }

    /// Return only enum variants permitted after a type-shaped import qualifier.
    ///
    /// `use Option::So$0;` may import `Some`, but a `use` path cannot import inherent or
    /// trait-provided associated items through `Option`. Type aliases are followed to their
    /// nominal enum before variants are collected.
    pub fn associated_enum_variants_for_import(
        &self,
        importing_module: rg_ir_model::ModuleRef,
        qualifier: &Path,
    ) -> anyhow::Result<Vec<MemberAssociatedItemCandidate>> {
        let context = rg_semantic_ir::TypePathContext::module(importing_module);
        let resolution = ItemPathQuery::new(self.db, self.db)
            .resolve_type_path(context, qualifier)
            .context("resolve import associated item qualifier")?;
        let ty = match resolution {
            TypePathResolution::SelfType(def) | TypePathResolution::TypeDef(def) => {
                Some(Ty::adt(rg_ty::AdtTy::bare(def)))
            }
            TypePathResolution::TypeAlias(alias) => {
                SemanticSignatureQuery::with_resolver(self.db, self.db, self.db)
                    .type_alias_ty(alias)
                    .context("resolve import type alias target")?
            }
            TypePathResolution::Trait(_) | TypePathResolution::Unknown => None,
        };
        let Some(ty) = ty else {
            return Ok(Vec::new());
        };

        let mut candidates = Vec::new();
        for nominal in ty.as_adts() {
            let TypeDefId::Enum(enum_id) = nominal.def.id else {
                continue;
            };
            let Some(data) = ItemStoreQuery::new(self.db)
                .enum_data_for_type_def(nominal.def)
                .context("read import enum variant candidates")?
            else {
                continue;
            };
            candidates.extend((0..data.variants.len()).map(|index| {
                MemberAssociatedItemCandidate {
                    item: MemberAssociatedItem::EnumVariant(EnumVariantRef {
                        origin: nominal.def.origin,
                        enum_id,
                        index,
                    }),
                    applicability: TraitApplicability::Yes,
                }
            }));
        }
        Ok(candidates)
    }

    fn body_associated_prefix(
        qualifier: &IndexedAssociatedPathQualifier,
    ) -> BodyAssociatedPathPrefix {
        match qualifier {
            IndexedAssociatedPathQualifier::Type(ty) => BodyAssociatedPathPrefix::Type(ty.clone()),
            IndexedAssociatedPathQualifier::QualifiedTrait { self_ty, trait_ref } => {
                BodyAssociatedPathPrefix::QualifiedTrait {
                    self_ty: self_ty.clone(),
                    trait_ref: trait_ref.clone(),
                }
            }
        }
    }

    /// Convert compiler refs once and merge overlapping impl evidence by declaration.
    fn project_associated_candidates(
        candidates: Vec<AssociatedItemCandidateRef>,
    ) -> Vec<MemberAssociatedItemCandidate> {
        let mut projected: Vec<MemberAssociatedItemCandidate> = Vec::new();
        for candidate in candidates {
            let item = match candidate.item() {
                AssociatedItemRef::Function(function) => MemberAssociatedItem::Function(function),
                AssociatedItemRef::TypeAlias(alias) => MemberAssociatedItem::TypeAlias(alias),
                AssociatedItemRef::Const(konst) => MemberAssociatedItem::Const(konst),
                AssociatedItemRef::EnumVariant(variant) => {
                    MemberAssociatedItem::EnumVariant(variant)
                }
            };
            if let Some(existing) = projected.iter_mut().find(|existing| existing.item == item) {
                existing.applicability = existing.applicability.or(candidate.applicability());
                continue;
            }
            projected.push(MemberAssociatedItemCandidate {
                item,
                applicability: candidate.applicability(),
            });
        }
        projected
    }
}
