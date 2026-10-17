//! Semantic decisions used by type rendering.
//!
//! These queries choose which type information and names to show. They do not manage output
//! buffers; the renderer uses the returned facts and name spellings during its shared walk.

use rg_ir_model::{
    CrateRef, GenericDefRef, ItemOwner, TraitApplicability, TraitDefRef, TypeAliasRef, TypeDefId,
};
use rg_item_tree::{LangItem, VisibilityLevel};
use rg_semantic_ir::{GenericParamSource, GenericsQuery, ItemStoreQuery};
use rg_text::RustEdition;
use rg_ty::{
    AdtTy, ConstValue, GenericArg, ProjectionTy, Substitution, TraitApplication, Ty, TyContext,
    signature::SemanticSignatureQuery,
    trait_selection::{TraitGoal, TraitSelectionQuery},
};

use crate::{IndexedViewDb, display::syntax::SyntaxRenderer, item::path::PathView};

/// Supplies display choices that need indexed declarations or trait solving.
/// Iterator shortening opts into a type context for projecting `<Adapter as Iterator>::Item`;
/// ordinary name and default-argument queries do not need that context.
#[derive(Clone)]
pub(super) struct TypeLabelQuery<'a, 'db> {
    db: &'a IndexedViewDb<'db>,
    syntax: SyntaxRenderer,
    iterator_context: Option<TyContext<'a, &'a IndexedViewDb<'db>, &'a IndexedViewDb<'db>>>,
}

impl<'a, 'db> TypeLabelQuery<'a, 'db> {
    pub(super) fn new(db: &'a IndexedViewDb<'db>, edition: RustEdition) -> Self {
        Self {
            db,
            syntax: SyntaxRenderer::new(edition),
            iterator_context: None,
        }
    }

    pub(super) fn enable_iterator_simplification(
        &mut self,
        use_site: CrateRef,
    ) -> anyhow::Result<()> {
        self.iterator_context = Some(TyContext::new(
            self.db,
            self.db,
            self.db.item_lookup_query(use_site)?,
            use_site,
            self.db.cancellation().clone(),
        ));
        Ok(())
    }

    /// Recognize public adapters declared under `core::iter`, including its private modules.
    /// Keep named user iterators and types such as `slice::Iter` recognizable in their hints.
    pub(super) fn iterator_item(&self, ty: &AdtTy) -> anyhow::Result<Option<Ty>> {
        let Some(context) = &self.iterator_context else {
            return Ok(None);
        };
        if let Some(iterator) = context.item_lookup().lang_trait(LangItem::Iterator)
            && ty.def.origin == iterator.origin
            && let TypeDefId::Struct(id) = ty.def.id
            && let Some(data) = ItemStoreQuery::new(self.db)
                .item_store_for_origin(ty.def.origin)?
                .and_then(|items| items.struct_data(id))
            && data.visibility == VisibilityLevel::Public
        {
            // The compiler identity gives us the defining crate without relying on its import
            // name. Walk to the root's child to distinguish `iter::adapters` from `slice::iter`.
            let Some(modules) = self.db.def_map_for_origin(ty.def.origin)? else {
                return Ok(None);
            };
            let mut module = modules.module(data.owner.module);
            loop {
                let Some(current) = module else {
                    return Ok(None);
                };
                let Some(parent) = current.parent.and_then(|parent| modules.module(parent)) else {
                    return Ok(None);
                };
                if parent.parent.is_none() {
                    if current.name.as_ref().map(|name| name.as_str()) != Some("iter") {
                        return Ok(None);
                    }
                    break;
                }
                module = Some(parent);
            }

            // Normalize through the real impl rather than guessing from adapter arguments:
            // `Map<I, F>` yields F's output, while `Enumerate<I>` yields `(usize, I::Item)`.
            // Keep the concrete label if the available facts cannot establish that item type.
            // TODO: Supply enclosing generic bounds to simplify adapters over `I: Iterator` too.
            let goal = TraitGoal::new(Ty::adt(ty.clone()), iterator, Vec::new());
            return Ok(TraitSelectionQuery::new(context.clone())
                .normalize_assoc_type(&goal, "Item")?
                .filter(|result| result.applicability == TraitApplicability::Yes)
                .map(|result| result.ty));
        }
        Ok(None)
    }

    /// Choose how to spell the trait named by `def`.
    ///
    /// `application` describes the impl receiving code from our completion or code action.
    /// When rendering a `dyn Object` parameter in `impl Service<u8> for Worker`, `def` identifies
    /// `Object` and `application` describes `Service<u8>`.
    ///
    /// Inlay hints and hover pass `None` and use the short name for readability. When we insert
    /// member code, callers pass `Some` and keep the full path so `Object` resolves even if the
    /// user has not imported it in the impl's module.
    pub(super) fn trait_name(
        &self,
        def: TraitDefRef,
        application: Option<&TraitApplication>,
    ) -> anyhow::Result<Option<String>> {
        if application.is_some() {
            return PathView::new(self.db, self.syntax.edition()).trait_path(def);
        }
        Ok(ItemStoreQuery::new(self.db)
            .trait_data(def)?
            .map(|data| self.syntax.identifier(&data.name).to_string()))
    }

    /// Leave out trailing arguments that equal their declared defaults.
    ///
    /// For `struct Pair<T = u8, U = u16>`, `Pair<u8, u16>` can be shown as `Pair`, but
    /// `Pair<u8, Custom>` needs both arguments to keep `Custom` in the second position.
    pub(super) fn non_default_generic_args<'ty>(
        &self,
        owner: GenericDefRef,
        args: &'ty [GenericArg],
    ) -> anyhow::Result<&'ty [GenericArg]> {
        if args.is_empty() {
            return Ok(args);
        }
        let generics = GenericsQuery::new(self.db).generics(owner)?;
        // `trait Service<T>` has parameters `[Self, T]`, but `dyn Service<User>` supplies
        // only `[User]` here. Skip `Self` so the parameter and argument positions line up.
        let params = generics
            .iter()
            .filter(|param| !matches!(param.source(), GenericParamSource::TraitSelf))
            .collect::<Vec<_>>();
        if params.len() != args.len() {
            return Ok(args);
        }
        // A default like `U = T` needs the actual `T` before comparison. For `Pair<User, User>`,
        // substitute `T = User` so the default for `U` becomes `User` too.
        let mut substitution = Substitution::new();
        for (param, arg) in params.iter().zip(args) {
            substitution.push(param.param(), arg.clone());
        }
        let signatures = SemanticSignatureQuery::with_resolver(self.db, self.db, self.db);
        let mut keep = args.len();
        // Work backwards: once one argument must stay, all earlier slots must stay too.
        // Compare resolved values so an alias and the type it names can count as equal.
        for (param, arg) in params.iter().zip(args).rev() {
            let Some(default) = signatures.generic_param_default(param.param())? else {
                break;
            };
            let default = substitution.apply_arg(&default);
            // Two unknown values are not evidence that an argument matches its default.
            if default.has_unknown()
                || matches!(default, GenericArg::Const(ConstValue::Unknown))
                || &default != arg
            {
                break;
            }
            keep -= 1;
        }
        Ok(&args[..keep])
    }

    /// Return the associated name only for projections owned by the exact trait application.
    /// An unrelated projection stays unknown, rather than becoming the wrong `Self::Assoc` in
    /// a member we insert into the user's impl.
    pub(super) fn trait_impl_projection_name(
        &self,
        projection: &ProjectionTy,
        application: &TraitApplication,
    ) -> anyhow::Result<Option<&'a str>> {
        if projection.args != application.args {
            return Ok(None);
        }
        let Some(data) = ItemStoreQuery::new(self.db).type_alias_data(projection.associated_ty)?
        else {
            return Ok(None);
        };
        let ItemOwner::Trait(trait_id) = data.owner else {
            return Ok(None);
        };
        if (TraitDefRef {
            origin: projection.associated_ty.origin,
            id: trait_id,
        }) != application.def
        {
            return Ok(None);
        }
        Ok(Some(data.name.as_str()))
    }

    /// Callable bounds store their inputs as one tuple argument and their return type as `Output`.
    /// For example, `Fn<(u8, u16), Output = bool>` supplies `[u8, u16]` and `bool` so the renderer
    /// can write `Fn(u8, u16) -> bool`. Both pieces must be present to use callable notation.
    pub(super) fn callable_signature<'params, 'ret>(
        &self,
        def: TraitDefRef,
        args: &'params [GenericArg],
        bindings: impl Iterator<Item = (TypeAliasRef, &'ret Ty)>,
    ) -> anyhow::Result<Option<(&'a str, &'params [Ty], &'ret Ty)>> {
        let items = ItemStoreQuery::new(self.db);
        let Some(trait_data) = items.trait_data(def)? else {
            return Ok(None);
        };
        if !matches!(trait_data.name.as_str(), "Fn" | "FnMut" | "FnOnce") {
            return Ok(None);
        }

        let mut positional = args.iter();
        let Some(GenericArg::Type(input)) = positional.next() else {
            return Ok(None);
        };
        if positional.next().is_some() {
            return Ok(None);
        }
        let Ty::Tuple(params) = input.as_ref() else {
            return Ok(None);
        };
        let mut ret = None;
        for (associated_ty, ty) in bindings {
            if items
                .type_alias_data(associated_ty)?
                .is_some_and(|data| data.name.as_str() == "Output")
            {
                ret = Some(ty);
                break;
            }
        }
        let Some(ret) = ret else {
            return Ok(None);
        };

        Ok(Some((trait_data.name.as_str(), params, ret)))
    }
}
