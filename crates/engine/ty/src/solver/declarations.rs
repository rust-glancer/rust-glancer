//! Declaration access at the boundary between semantic storage and the compiler solver.
//!
//! The provider reuses Glancer's lowering and lookup. Solver callbacks are infallible, so the
//! provider retains source errors for the enclosing operation to return to its caller.

use std::sync::{Arc, Mutex};

use rg_ir_model::{GenericParamRef, ImplRef, TraitDefRef, TypeDefRef};
use rg_std::UniqueVec;
use rustc_type_ir::{
    data_structures::HashMap,
    lang_items::{SolverAdtLangItem, SolverProjectionLangItem, SolverTraitLangItem},
};

use super::{DefId, types::AdtDef};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum LangItem {
    Trait(SolverTraitLangItem),
    Projection(SolverProjectionLangItem),
    Adt(SolverAdtLangItem),
}

/// The parts of a source declaration that the solver can ask about.
///
/// These types still use Glancer's owned representation. For example, a function's `T` is a
/// generic parameter here; each call gets its own inference variable when it is instantiated.
/// That lets several solver operations share a declaration without sharing their answers.
pub(crate) struct Declaration {
    pub name: String,
    pub generics: Vec<GenericParamRef>,
    pub parent_count: usize,
    pub parent: Option<DefId>,
    // Requirements on using the item, such as `T: Clone` on `fn copy<T: Clone>(...)`.
    pub predicates: Vec<crate::Clause>,
    // Promises about an associated or opaque type, such as `type Item: Clone`.
    pub bounds: Vec<crate::Clause>,
    pub lang_item: Option<LangItem>,
    pub kind: DeclarationKind,
}

pub(crate) enum DeclarationKind {
    Adt {
        data: AdtDef,
        fields: Vec<crate::Ty>,
    },
    Trait {
        is_auto: bool,
        is_unsafe: bool,
        associated_types: Vec<DefId>,
    },
    Impl {
        header: crate::lowering::ImplHeader,
        associated_types: Vec<(String, DefId)>,
    },
    Function(crate::lowering::CallableSignature),
    Alias(Option<crate::Ty>),
    Opaque,
    Unavailable,
}

pub(crate) struct DeclarationGenerics {
    pub params: Vec<GenericParamRef>,
    pub parent_count: usize,
}

pub(crate) trait DeclarationProvider {
    fn declaration(&self, id: DefId) -> Option<Arc<Declaration>>;
    fn generics(&self, id: DefId) -> Option<DeclarationGenerics>;
    fn adt_def(&self, id: TypeDefRef) -> Option<AdtDef>;
    fn impls(&self, trait_id: TraitDefRef, self_ty: Option<crate::Ty>) -> Option<Vec<ImplRef>>;
    fn lang_item(&self, item: LangItem) -> Option<DefId>;
    fn is_cancelled(&self) -> bool;
}

use std::cell::{Cell, RefCell};

use rg_def_map::DefMapSource;
use rg_ir_model::{
    AssocItemId, GenericDefRef, ItemOwner, TraitDefRef as TraitRef, TypeAliasRef, TypeDefId,
};
use rg_semantic_ir::{ItemStoreSource, TypePathContext};

use crate::{
    TyContext,
    lowering::{TypeLoweringAnchor, TypeLoweringEnv, TypeLoweringQuery},
};

/// Supplies the lexical context needed by solver declarations and standalone editor queries.
/// Saved item queries have only crate declarations; body queries also expose their local impls
/// and the generic owner's assumptions.
/// For example, solving inside `fn f<T: Clone>()` must see `T: Clone` and any impls declared in
/// a surrounding block, even though neither comes from a crate-wide impl search.
pub trait SolverScope: crate::lowering::TypePathResolver {
    fn local_trait_impls(&self, _trait_ref: TraitDefRef) -> Result<Vec<ImplRef>, Self::Error> {
        Ok(Vec::new())
    }

    fn generic_owner(&self) -> Option<GenericDefRef> {
        None
    }

    /// Reuse declarations only while the source snapshot and lexical resolver stay the same.
    /// A body or edited overlay must receive a fresh cache when its context is constructed.
    fn declaration_cache(&self) -> Option<&DeclarationCache> {
        None
    }
}

impl<R: SolverScope + ?Sized> SolverScope for &R {
    fn local_trait_impls(&self, trait_ref: TraitDefRef) -> Result<Vec<ImplRef>, Self::Error> {
        R::local_trait_impls(*self, trait_ref)
    }

    fn generic_owner(&self) -> Option<GenericDefRef> {
        R::generic_owner(*self)
    }

    fn declaration_cache(&self) -> Option<&DeclarationCache> {
        R::declaration_cache(*self)
    }
}

impl<'query, D, I> SolverScope for crate::lookup::ItemPathQuery<'query, D, I>
where
    D: DefMapSource<Error = I::Error>,
    I: ItemStoreSource<'query>,
{
}

/// Owned declaration types shared by operations in one immutable lexical context.
///
/// A declaration such as `Self::Item` is lowered through the caller's resolver. Even a saved
/// declaration identity may see an edited impl, so this cache belongs to the body context rather
/// than a crate or a process. Compiler types and inference answers never enter this cache.
#[derive(Clone, Default)]
pub struct DeclarationCache {
    entries: Arc<Mutex<HashMap<DefId, Arc<Declaration>>>>,
}

/// Declaration lowering remains tied to the same semantic snapshot and use-site lookup as its
/// caller. The local map avoids locking on repeated callbacks within one solver operation.
///
/// Compiler callbacks ask for declarations as they explore a goal. This provider loads those
/// declarations on demand, using the same path resolver as the body or editor query. If a read
/// fails, it saves the error: the compiler's callback API cannot return our storage errors, but
/// the enclosing `with_solver` call can.
pub struct SemanticDeclarations<'a, 'query, D, I: ItemStoreSource<'query>> {
    context: &'a TyContext<'query, D, I>,
    resolver: &'a dyn SolverScope<Error = I::Error>,
    cache: RefCell<HashMap<DefId, Arc<Declaration>>>,
    shared: Option<&'a DeclarationCache>,
    error: RefCell<Option<I::Error>>,
    cancelled: Cell<bool>,
    profile: RefCell<super::profile::SolverProfile>,
}

impl<'a, 'query, D, I> SemanticDeclarations<'a, 'query, D, I>
where
    D: DefMapSource<Error = I::Error>,
    I: ItemStoreSource<'query>,
{
    pub fn new(
        context: &'a TyContext<'query, D, I>,
        resolver: &'a dyn SolverScope<Error = I::Error>,
    ) -> Self {
        Self {
            context,
            resolver,
            cache: RefCell::default(),
            shared: resolver.declaration_cache(),
            error: RefCell::new(None),
            cancelled: Cell::new(false),
            profile: RefCell::default(),
        }
    }

    pub fn take_error(&self) -> Option<I::Error> {
        self.error.borrow_mut().take()
    }

    /// Run a body or query with temporary type storage, then release that storage together.
    ///
    /// Work inside the closure can keep `Vec<?T>` alive while more evidence arrives. Before
    /// returning, it must turn the answer into owned types or other owned facts. The closure's
    /// lifetime prevents a solver type from escaping into the saved body or an editor result.
    /// Source errors collected by callbacks are returned after the temporary storage is dropped.
    pub fn with_solver<R>(
        &self,
        run: impl for<'s> FnOnce(super::Solver<'s>) -> R,
    ) -> Result<R, I::Error> {
        let result = {
            let storage = super::SolverStorage::new(self);
            let solver = super::Solver::new(storage.interner());
            run(solver)
        };
        match self.take_error() {
            Some(error) => Err(error),
            None => Ok(result),
        }
    }

    fn load(&self, id: DefId) -> Result<Option<Declaration>, I::Error> {
        let paths = self.context.item_paths();
        let items = paths.items();
        let Some(owner) = id.generic_owner() else {
            return Ok(None);
        };
        let generics = paths.generics().generics(owner)?;
        let mut result = Declaration {
            name: String::new(),
            generics: generics.iter().map(|p| p.param()).collect(),
            parent_count: generics.parent_len(),
            parent: None,
            predicates: Vec::new(),
            bounds: Vec::new(),
            lang_item: None,
            kind: DeclarationKind::Unavailable,
        };
        match id {
            DefId::Trait(id) => {
                let Some(data) = items.trait_data(id)? else {
                    return Ok(None);
                };
                let Some(header) = crate::lowering::SemanticSignatureQuery::trait_header_with(
                    paths,
                    &self.resolver,
                    id,
                )?
                else {
                    return Ok(None);
                };
                result.name = data.name.to_string();
                result.predicates = header.clauses;
                // TODO: Retain `auto` in the source declaration model before enabling automatic
                // structural impls. Ordinary trait impls use exactly their declared predicates.
                result.kind = DeclarationKind::Trait {
                    is_auto: false,
                    is_unsafe: data.is_unsafe,
                    associated_types: data
                        .items
                        .iter()
                        .filter_map(|item| match item {
                            AssocItemId::TypeAlias(alias) => Some(DefId::TypeAlias(TypeAliasRef {
                                origin: id.origin,
                                id: *alias,
                            })),
                            _ => None,
                        })
                        .collect(),
                };
                for item in [
                    SolverTraitLangItem::Fn,
                    SolverTraitLangItem::FnMut,
                    SolverTraitLangItem::FnOnce,
                    SolverTraitLangItem::PointeeSized,
                    SolverTraitLangItem::Sized,
                    SolverTraitLangItem::MetaSized,
                    SolverTraitLangItem::Tuple,
                    SolverTraitLangItem::Destruct,
                ] {
                    let lang = LangItem::Trait(item);
                    if self.lang_item(lang) == Some(DefId::Trait(id)) {
                        result.lang_item = Some(lang);
                    }
                }
            }
            DefId::Impl(id) => {
                let Some(data) = items.impl_data(id)? else {
                    return Ok(None);
                };
                let Some(header) = crate::lowering::impl_header_with(paths, &self.resolver, id)?
                else {
                    return Ok(None);
                };
                result.predicates = header.clauses.clone();
                let mut associated_types = Vec::new();
                for item in &data.items {
                    if let AssocItemId::TypeAlias(alias) = item {
                        let alias = TypeAliasRef {
                            origin: id.origin,
                            id: *alias,
                        };
                        let Some(data) = items.type_alias_data(alias)? else {
                            return Ok(None);
                        };
                        associated_types.push((data.name.to_string(), DefId::TypeAlias(alias)));
                    }
                }
                result.kind = DeclarationKind::Impl {
                    header,
                    associated_types,
                };
            }
            DefId::Function(id) => {
                let Some(data) = items.function_data(id)? else {
                    return Ok(None);
                };
                // Declaration anchors still determine ownership; the caller's resolver also
                // knows the lexical/body overlay needed for paths such as an inherent Self::Id.
                let Some(signature) =
                    crate::lowering::CallableSignature::lower_with(paths, &self.resolver, id)?
                else {
                    return Ok(None);
                };
                result.name = data.name.to_string();
                result.parent = Self::parent(id.origin, data.owner);
                result.predicates = signature.clauses.clone();
                result.kind = DeclarationKind::Function(signature);
            }
            DefId::TypeAlias(id) => {
                let Some(data) = items.type_alias_data(id)? else {
                    return Ok(None);
                };
                let Some(context) = items.type_path_context_for_owner(id.origin, data.owner)?
                else {
                    return Ok(None);
                };
                result.name = data.name.to_string();
                result.parent = Self::parent(id.origin, data.owner);
                let lowering = TypeLoweringQuery::new(paths, &self.resolver);
                let mut lower = lowering.session(TypeLoweringEnv::new(
                    owner,
                    TypeLoweringAnchor::Context(context),
                ))?;
                result.predicates = lower.lower_clauses()?;
                if let Some(DefId::Trait(_)) = result.parent {
                    let subject =
                        crate::Ty::Alias(crate::AliasTy::Projection(crate::ProjectionTy {
                            associated_ty: id,
                            args: crate::Substitution::identity(&generics).args_for(&generics),
                        }));
                    for bound in data.signature.bounds() {
                        if let Some(ty) = bound.required_trait_ty()
                            && let Some(bound) = lower.lower_trait_ref(ty, subject.clone())?
                        {
                            result.bounds.extend(bound.into_clauses());
                        }
                    }
                }
                result.kind = DeclarationKind::Alias(if data.signature.aliased_ty().is_some() {
                    crate::lowering::SemanticSignatureQuery::type_alias_ty_with(
                        paths,
                        &self.resolver,
                        id,
                    )?
                } else {
                    None
                });
            }
            DefId::Opaque(id) => {
                let opaque_bounds =
                    crate::lowering::SemanticSignatureQuery::opaque_bounds_for_owner_with(
                        paths,
                        &self.resolver,
                        id.owner,
                    )?;
                let Some((_, bounds)) =
                    opaque_bounds.iter().find(|(opaque, _)| opaque.opaque == id)
                else {
                    return Ok(None);
                };
                result.bounds = bounds
                    .iter()
                    .flat_map(|b| b.clone().into_clauses())
                    .collect();
                result.kind = DeclarationKind::Opaque;
            }
            DefId::Adt(id) => {
                let Some(store) = items.item_store_for_origin(id.origin)? else {
                    return Ok(None);
                };
                let (name, module, fields) = match id.id {
                    TypeDefId::Struct(struct_id) => {
                        let Some(data) = store.struct_data(struct_id) else {
                            return Ok(None);
                        };
                        (
                            data.name.clone(),
                            data.owner,
                            data.fields.fields().iter().collect::<Vec<_>>(),
                        )
                    }
                    TypeDefId::Enum(enum_id) => {
                        let Some(data) = store.enum_data(enum_id) else {
                            return Ok(None);
                        };
                        (
                            data.name.clone(),
                            data.owner,
                            data.variants
                                .iter()
                                .flat_map(|v| v.fields.fields())
                                .collect(),
                        )
                    }
                    TypeDefId::Union(union_id) => {
                        let Some(data) = store.union_data(union_id) else {
                            return Ok(None);
                        };
                        (data.name.clone(), data.owner, data.fields.iter().collect())
                    }
                };
                result.name = name.to_string();
                let lowering = TypeLoweringQuery::new(paths, &self.resolver);
                let mut lower = lowering.session(TypeLoweringEnv::new(
                    owner,
                    TypeLoweringAnchor::Context(TypePathContext::module(module)),
                ))?;
                result.predicates = lower.lower_clauses()?;
                let fields = fields
                    .into_iter()
                    .map(|f| lower.lower_type_ref(&f.ty))
                    .collect::<Result<Vec<_>, _>>()?;
                result.kind = DeclarationKind::Adt {
                    data: AdtDef::new(id),
                    fields,
                };
            }
            DefId::Const(id) => {
                let Some(data) = items.const_data(id)? else {
                    return Ok(None);
                };
                result.parent = Self::parent(id.origin, data.owner);
                if let Some(context) = items.type_path_context_for_owner(id.origin, data.owner)? {
                    result.predicates = TypeLoweringQuery::new(paths, &self.resolver)
                        .session(TypeLoweringEnv::new(
                            owner,
                            TypeLoweringAnchor::Context(context),
                        ))?
                        .lower_clauses()?;
                }
            }
            DefId::Static(_) => {}
            DefId::Closure(_) | DefId::Unavailable => unreachable!(),
        }

        // PointeeSized is a tautology in the compiler type system and must not reach its solver
        // assembly. In particular, core uses it on pointer impls without declaring source impls.
        let pointee_sized = self
            .context
            .item_lookup()
            .lang_trait(rg_item_tree::LangItem::PointeeSized);
        let retained = |clause: &crate::Clause| !matches!(clause, crate::Clause::Implemented(tr) if Some(tr.def) == pointee_sized);
        result.predicates.retain(retained);
        result.bounds.retain(retained);
        if let DeclarationKind::Function(signature) = &mut result.kind {
            signature.clauses.retain(retained);
        }
        Ok(Some(result))
    }

    fn parent(origin: rg_ir_model::DefMapRef, owner: ItemOwner) -> Option<DefId> {
        match owner {
            ItemOwner::Trait(id) => Some(DefId::Trait(TraitRef { origin, id })),
            ItemOwner::Impl(id) => Some(DefId::Impl(ImplRef { origin, id })),
            ItemOwner::Module(_) => None,
        }
    }
}

impl<'query, D, I> DeclarationProvider for SemanticDeclarations<'_, 'query, D, I>
where
    D: DefMapSource<Error = I::Error>,
    I: ItemStoreSource<'query>,
{
    fn generics(&self, id: DefId) -> Option<DeclarationGenerics> {
        if self.is_cancelled() {
            return None;
        }
        if let Some(data) = self.cache.borrow().get(&id) {
            return Some(DeclarationGenerics {
                params: data.generics.clone(),
                parent_count: data.parent_count,
            });
        }
        let owner = id.generic_owner()?;
        // Parameter identity and order come from syntax-shaped declarations. Reading them must
        // not lower a function's signature or an alias's target through the solver again.
        let paths = self.context.item_paths();
        let result = (|| {
            if paths.items().semantic_item_view(owner.into())?.is_none() {
                return Ok(None);
            }
            let generics = paths.generics().generics(owner)?;
            Ok(Some(DeclarationGenerics {
                params: generics.iter().map(|p| p.param()).collect(),
                parent_count: generics.parent_len(),
            }))
        })();
        match result {
            Ok(data) => data,
            Err(error) => {
                *self.error.borrow_mut() = Some(error);
                None
            }
        }
    }

    fn adt_def(&self, id: TypeDefRef) -> Option<AdtDef> {
        if self.is_cancelled() {
            return None;
        }

        // Merely naming `Vec<T>` does not require its fields or bounds. Check that the source
        // declaration exists, then leave field lowering to the callbacks that need field types.
        match self
            .context
            .item_paths()
            .items()
            .semantic_item_view(GenericDefRef::TypeDef(id).into())
        {
            Ok(Some(_)) => Some(AdtDef::new(id)),
            Ok(None) => None,
            Err(error) => {
                *self.error.borrow_mut() = Some(error);
                None
            }
        }
    }

    fn declaration(&self, id: DefId) -> Option<Arc<Declaration>> {
        if self.is_cancelled() {
            return None;
        }
        if let Some(data) = self.cache.borrow().get(&id) {
            self.profile.borrow_mut().declaration_hits += 1;
            return Some(data.clone());
        }
        if let Some(shared) = self.shared
            && let Some(data) = shared
                .entries
                .lock()
                .expect("declaration cache lock should not be poisoned")
                .get(&id)
                .cloned()
        {
            self.profile.borrow_mut().shared_declaration_hits += 1;
            self.cache.borrow_mut().insert(id, data.clone());
            return Some(data);
        }

        // Lowering may itself select an associated declaration. Keep the shared lock out of
        // that recursive work, and publish only a complete successful source read.
        self.profile.borrow_mut().declaration_loads += 1;
        match self.load(id) {
            Ok(Some(data)) => {
                let data = Arc::new(data);
                if let Some(shared) = self.shared {
                    shared
                        .entries
                        .lock()
                        .expect("declaration cache lock should not be poisoned")
                        .insert(id, data.clone());
                }
                self.cache.borrow_mut().insert(id, data.clone());
                Some(data)
            }
            Ok(None) => None,
            Err(error) => {
                *self.error.borrow_mut() = Some(error);
                None
            }
        }
    }

    fn impls(&self, trait_id: TraitRef, self_ty: Option<crate::Ty>) -> Option<Vec<ImplRef>> {
        // Keep the source index's outer-shape rejection before lowering impl headers. For
        // example, an unsupported `&dyn Trait` header can lower to Unknown, but its reference
        // syntax still tells us it cannot implement a trait for Vec<T>.
        let lookup = self.context.item_lookup();
        let impls = match self_ty {
            Some(ty) => crate::lookup::trait_impl_candidates(self.context, trait_id, &ty),
            None => lookup
                .trait_impls_for_trait(trait_id)
                .ok()
                .map(Option::unwrap_or_default),
        };
        match impls {
            Some(impls) => {
                let mut result = impls
                    .into_iter()
                    .map(|i| i.impl_ref)
                    .collect::<UniqueVec<_>>();
                match self.resolver.local_trait_impls(trait_id) {
                    Ok(local) => result.extend(local),
                    Err(error) => {
                        *self.error.borrow_mut() = Some(error);
                        return None;
                    }
                }
                Some(result.into_vec())
            }
            None => {
                self.cancelled.set(true);
                None
            }
        }
    }

    fn lang_item(&self, item: LangItem) -> Option<DefId> {
        let source = match item {
            LangItem::Trait(SolverTraitLangItem::Sized) => rg_item_tree::LangItem::Sized,
            LangItem::Trait(SolverTraitLangItem::MetaSized) => rg_item_tree::LangItem::MetaSized,
            LangItem::Trait(SolverTraitLangItem::Tuple) => rg_item_tree::LangItem::Tuple,
            LangItem::Trait(SolverTraitLangItem::Destruct) => rg_item_tree::LangItem::Destruct,
            LangItem::Trait(SolverTraitLangItem::Fn) => rg_item_tree::LangItem::Fn,
            LangItem::Trait(SolverTraitLangItem::FnMut) => rg_item_tree::LangItem::FnMut,
            LangItem::Trait(SolverTraitLangItem::FnOnce) => rg_item_tree::LangItem::FnOnce,
            LangItem::Trait(SolverTraitLangItem::PointeeSized) => {
                rg_item_tree::LangItem::PointeeSized
            }
            _ => return None,
        };
        self.context
            .item_lookup()
            .lang_trait(source)
            .map(DefId::Trait)
    }

    fn is_cancelled(&self) -> bool {
        self.cancelled.get()
            || rg_std::Cancelable::check_cancelled(self.context, "solver declarations").is_err()
    }
}
