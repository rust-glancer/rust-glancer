//! Trait-backed `Deref` target lookup for autoderef.
//!
//! This module deliberately stays narrow: it recognizes the canonical `Deref` language item and
//! asks the shared trait-selection projection path for `Target`. No adjustment code reads an impl
//! alias body directly or assumes that the trait is reachable through the path `core::ops::Deref`.

use rg_def_map::DefMapSource;
use rg_ir_model::{ItemOwner, TraitDefRef};
use rg_item_tree::LangItem;
use rg_semantic_ir::ItemStoreSource;
use rg_std::UniqueVec;

use crate::{
    AdtTy, GenericArgs, TraitGoal, TraitSelectionQuery, Ty, TyContext, inference::InferenceTable,
};

/// Resolves the associated `Target` type for applicable canonical `Deref` impls.
#[derive(Clone)]
pub(crate) struct DerefResolver<'query, D, I> {
    context: TyContext<'query, D, I>,
}

impl<'query, D, I> DerefResolver<'query, D, I>
where
    D: DefMapSource + Clone,
    I: ItemStoreSource<'query, Error = D::Error> + Clone,
{
    pub(crate) fn new(context: TyContext<'query, D, I>) -> Self {
        Self { context }
    }

    /// Returns all one-step `Deref::Target` types for a known type.
    pub(crate) fn targets_for_ty(&self, ty: &Ty) -> Result<UniqueVec<Ty>, D::Error> {
        // TODO: Add `DerefMut` once receiver contexts carry enough mutability information to
        // distinguish mutable adjustment from shared `Deref`.
        let mut targets = UniqueVec::new();
        for receiver_ty in ty.as_adts() {
            for target in self.targets_for_nominal(receiver_ty)? {
                targets.push(target);
            }
        }
        Ok(targets)
    }

    /// Returns one-step `Deref::Target` types for a nominal receiver.
    ///
    /// For `impl<T> core::ops::Deref for Wrapper<T> { type Target = T; }` and receiver
    /// `Wrapper<User>`, this resolves the target as `User`.
    fn targets_for_nominal(&self, receiver_ty: &AdtTy) -> Result<UniqueVec<Ty>, D::Error> {
        let mut targets = UniqueVec::new();
        let Some((deref_trait, target_name)) = self.canonical_deref_items()? else {
            return Ok(targets);
        };
        let table = InferenceTable::new();
        let query = TraitSelectionQuery::new(self.context.clone());
        let goal = TraitGoal::new(
            Ty::adt(receiver_ty.clone()),
            deref_trait,
            GenericArgs::empty(),
        );
        let Some(projection) = query.normalize_assoc_type(&goal, target_name.as_str(), &table)?
        else {
            return Ok(targets);
        };
        if projection.applicability != rg_ir_model::TraitApplicability::Yes {
            return Ok(targets);
        }
        let target = projection.table.finalize(&projection.ty);
        if target.is_projectable() {
            targets.push(target);
        }

        Ok(targets)
    }

    /// Find the visible `Deref` and `Deref::Target` identities and verify they belong together.
    ///
    /// The two attributes are indexed independently. Checking the associated type's owner prevents
    /// malformed declarations from combining an otherwise valid `Deref` trait with an unrelated
    /// `#[lang = "deref_target"]` alias.
    fn canonical_deref_items(&self) -> Result<Option<(TraitDefRef, rg_text::Name)>, D::Error> {
        let Some(deref_trait) = self.context.item_lookup().lang_trait(LangItem::Deref) else {
            return Ok(None);
        };
        let Some(target_alias) = self
            .context
            .item_lookup()
            .lang_type_alias(LangItem::DerefTarget)
        else {
            return Ok(None);
        };
        let Some(target_data) = self
            .context
            .item_paths()
            .items()
            .type_alias_data(target_alias)?
        else {
            return Ok(None);
        };
        let ItemOwner::Trait(owner) = target_data.owner else {
            return Ok(None);
        };
        if target_alias.origin != deref_trait.origin || owner != deref_trait.id {
            return Ok(None);
        }
        Ok(Some((deref_trait, target_data.name.clone())))
    }
}
