//! Declaration access at the boundary between semantic storage and the compiler solver.
//!
//! The provider reuses Glancer's lowering and lookup. Solver callbacks are infallible, so the
//! provider retains source errors for the enclosing operation to return to its caller.

mod stored;

use std::{
    cell::{Cell, RefCell},
    sync::{Arc, Mutex},
};

use rg_def_map::DefMapSource;
use rg_ir_model::{
    AssocItemId, DefMapRef, GenericDefRef, GenericParamRef, ImplRef, ItemOwner, TraitDefRef,
    TraitImplRef, TypeAliasRef, TypeDefId, TypeDefRef,
};
use rg_semantic_ir::{ItemStoreSource, TypePathContext};
use rg_std::Cancelable;
use rustc_type_ir::{
    ClauseKind,
    data_structures::HashMap,
    inherent::IntoKind as _,
    lang_items::{SolverAdtLangItem, SolverProjectionLangItem, SolverTraitLangItem},
};

use self::stored::StoredDeclaration;
use super::{
    CallableSignature, Clause, DefId, ImplHeader, InferenceSubstitution, List, ProjectionTy,
    Solver, SolverInterner, SolverStorage, Ty, profile::SolverProfile, types::AdtDef,
};
use crate::{
    TyContext,
    lookup::{ItemPathQuery, TraitImplFilter},
    lowering::{TypeLoweringAnchor, TypeLoweringEnv, TypeLoweringQuery, TypePathResolver},
};

/// A declaration template in this operation's working types. Its parameters are replaced only
/// when a caller instantiates it; the template itself never contains call-owned variables.
pub(crate) struct Declaration<'s> {
    pub name: String,
    pub generics: Vec<GenericParamRef>,
    pub parent_count: usize,
    pub parent: Option<DefId>,
    // Requirements on using the item, such as `T: Clone` on `fn copy<T: Clone>(...)`.
    pub predicates: Vec<Clause<'s>>,
    // Promises about an associated or opaque type, such as `type Item: Clone`.
    pub bounds: Vec<Clause<'s>>,
    pub lang_item: Option<LangItem>,
    pub kind: DeclarationKind<'s>,
}

pub(crate) enum DeclarationKind<'s> {
    Adt {
        data: AdtDef,
        fields: Vec<Ty<'s>>,
    },
    Trait {
        is_auto: bool,
        is_unsafe: bool,
        associated_types: Vec<DefId>,
    },
    Impl {
        header: ImplHeader<'s>,
        associated_types: Vec<(String, DefId)>,
    },
    Function(CallableSignature<'s>),
    Alias(Option<Ty<'s>>),
    Opaque,
    Unavailable,
}

pub(crate) struct DeclarationGenerics {
    pub params: Vec<GenericParamRef>,
    pub parent_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum LangItem {
    Trait(SolverTraitLangItem),
    Projection(SolverProjectionLangItem),
    Adt(SolverAdtLangItem),
}

pub(crate) trait DeclarationProvider {
    fn declaration<'s>(&self, cx: SolverInterner<'s>, id: DefId) -> Option<Declaration<'s>>;
    fn generics(&self, id: DefId) -> Option<DeclarationGenerics>;
    fn adt_def(&self, id: TypeDefRef) -> Option<AdtDef>;
    fn impls(&self, trait_id: TraitDefRef, filter: TraitImplFilter) -> Option<Vec<ImplRef>>;
    fn lang_item(&self, item: LangItem) -> Option<DefId>;
    fn is_cancelled(&self) -> bool;
}

/// Supplies the lexical context needed by solver declarations and standalone editor queries.
/// Saved item queries have only crate declarations; body queries also expose their local impls
/// and the generic owner's assumptions.
/// For example, solving inside `fn f<T: Clone>()` must see `T: Clone` and any impls declared in
/// a surrounding block, even though neither comes from a crate-wide impl search.
pub trait SolverScope: TypePathResolver {
    fn local_trait_impls(&self, _trait_ref: TraitDefRef) -> Result<Vec<TraitImplRef>, Self::Error> {
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
    fn local_trait_impls(&self, trait_ref: TraitDefRef) -> Result<Vec<TraitImplRef>, Self::Error> {
        R::local_trait_impls(*self, trait_ref)
    }

    fn generic_owner(&self) -> Option<GenericDefRef> {
        R::generic_owner(*self)
    }

    fn declaration_cache(&self) -> Option<&DeclarationCache> {
        R::declaration_cache(*self)
    }
}

impl<'query, D, I> SolverScope for ItemPathQuery<'query, D, I>
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
    entries: Arc<Mutex<HashMap<DefId, Arc<StoredDeclaration>>>>,
}

/// Declaration lowering remains tied to the same semantic snapshot and use-site lookup as its
/// caller. Working templates belong to the interner; this provider bridges source reads and the
/// optional owned cache shared by separate operations.
///
/// Compiler callbacks ask for declarations as they explore a goal. This provider loads those
/// declarations on demand, using the same path resolver as the body or editor query. If a read
/// fails, it saves the error: the compiler's callback API cannot return our storage errors, but
/// the enclosing `with_solver` call can.
pub struct SemanticDeclarations<'a, 'query, D, I: ItemStoreSource<'query>> {
    paths: &'a ItemPathQuery<'query, D, I>,
    resolver: &'a dyn TypePathResolver<Error = I::Error>,
    // Only operations that evaluate goals need impl discovery and a use-site solver scope.
    #[allow(clippy::type_complexity)]
    solving: Option<(
        &'a TyContext<'query, D, I>,
        &'a dyn SolverScope<Error = I::Error>,
    )>,
    shared: Option<&'a DeclarationCache>,
    error: RefCell<Option<I::Error>>,
    cancelled: Cell<bool>,
    profile: RefCell<SolverProfile>,
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
            paths: context.item_paths(),
            resolver,
            solving: Some((context, resolver)),
            shared: resolver.declaration_cache(),
            error: RefCell::new(None),
            cancelled: Cell::new(false),
            profile: RefCell::default(),
        }
    }

    /// Independent source queries need declaration identities and generic metadata, but do
    /// not search for impls or evaluate goals. They use the same provider and lowering code with
    /// no use-site solver context. This keeps display queries from building an impl index.
    pub(crate) fn for_lowering(
        paths: &'a ItemPathQuery<'query, D, I>,
        resolver: &'a dyn TypePathResolver<Error = I::Error>,
    ) -> Self {
        Self {
            paths,
            resolver,
            solving: None,
            shared: None,
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
    pub fn with_solver<R>(&self, run: impl for<'s> FnOnce(Solver<'s>) -> R) -> Result<R, I::Error> {
        self.with_storage(|cx| run(Solver::new(cx)))
    }

    /// Source-only queries need working type storage without an inference context. Both kinds
    /// of operation collect callback errors and release their entire working graph here.
    pub(crate) fn with_storage<R>(
        &self,
        run: impl for<'s> FnOnce(SolverInterner<'s>) -> R,
    ) -> Result<R, I::Error> {
        let result = {
            let storage = SolverStorage::new(self);
            let result = run(storage.interner());
            storage.record_profile();
            result
        };
        match self.take_error() {
            Some(error) => Err(error),
            None => Ok(result),
        }
    }

    fn load<'s>(
        &self,
        cx: SolverInterner<'s>,
        id: DefId,
    ) -> Result<Option<Declaration<'s>>, I::Error> {
        let paths = self.paths;
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
                let Some(header) =
                    TypeLoweringQuery::new(paths, &self.resolver).trait_header(cx, id)?
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
                let Some(header) =
                    TypeLoweringQuery::new(paths, &self.resolver).impl_header(cx, id)?
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
                    TypeLoweringQuery::new(paths, &self.resolver).function(cx, id)?
                else {
                    return Ok(None);
                };
                result.name = data.name.to_string();
                result.parent = Self::parent(id.origin, data.owner);
                result.predicates = signature.clauses.to_vec();
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
                let mut lower = lowering.session(
                    cx,
                    TypeLoweringEnv::new(owner, TypeLoweringAnchor::Context(context)),
                )?;
                result.predicates = lower.lower_clauses()?;
                if let Some(DefId::Trait(_)) = result.parent {
                    let subject = cx.projection(ProjectionTy {
                        associated_ty: id,
                        args: InferenceSubstitution::identity(
                            cx,
                            generics.iter().map(|p| p.param()),
                        )
                        .args_for(cx, generics.iter().map(|p| p.param())),
                    });
                    for bound in data.signature.bounds() {
                        if let Some(ty) = bound.required_trait_ty()
                            && let Some(bound) = lower.lower_trait_ref(ty, subject)?
                        {
                            result.bounds.extend(bound.clauses(cx));
                        }
                    }
                }
                result.kind = DeclarationKind::Alias(if data.signature.aliased_ty().is_some() {
                    lowering.type_alias_ty(cx, id)?
                } else {
                    None
                });
            }
            DefId::Opaque(id) => {
                let opaque_bounds = TypeLoweringQuery::new(paths, &self.resolver)
                    .opaque_bounds_for_owner(cx, id.owner)?;
                let Some((_, bounds)) =
                    opaque_bounds.iter().find(|(opaque, _)| opaque.opaque == id)
                else {
                    return Ok(None);
                };
                result.bounds = bounds.iter().flat_map(|b| b.clauses(cx)).collect();
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
                let mut lower = lowering.session(
                    cx,
                    TypeLoweringEnv::new(
                        owner,
                        TypeLoweringAnchor::Context(TypePathContext::module(module)),
                    ),
                )?;
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
                        .session(
                            cx,
                            TypeLoweringEnv::new(owner, TypeLoweringAnchor::Context(context)),
                        )?
                        .lower_clauses()?;
                }
            }
            DefId::Static(_) => {}
            DefId::Closure(_) | DefId::Unavailable => unreachable!(),
        }

        // PointeeSized is a tautology in the compiler type system and must not reach its solver
        // assembly. In particular, core uses it on pointer impls without declaring source impls.
        let pointee_sized = self.lang_item(LangItem::Trait(SolverTraitLangItem::PointeeSized));
        let retained = |clause: &Clause<'s>| {
            !matches!(
                clause.kind().skip_binder(),
                ClauseKind::Trait(tr) if Some(tr.trait_ref.def_id) == pointee_sized
            )
        };
        result.predicates.retain(retained);
        result.bounds.retain(retained);
        if let DeclarationKind::Function(signature) = &mut result.kind {
            signature.clauses = List::new(
                cx,
                &signature
                    .clauses
                    .iter()
                    .filter(retained)
                    .collect::<Vec<_>>(),
            );
        }
        Ok(Some(result))
    }

    fn parent(origin: DefMapRef, owner: ItemOwner) -> Option<DefId> {
        match owner {
            ItemOwner::Trait(id) => Some(DefId::Trait(TraitDefRef { origin, id })),
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
        let owner = id.generic_owner()?;
        // Parameter identity and order come from syntax-shaped declarations. Reading them must
        // not lower a function's signature or an alias's target through the solver again.
        let paths = self.paths;
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
            .paths
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

    fn declaration<'s>(&self, cx: SolverInterner<'s>, id: DefId) -> Option<Declaration<'s>> {
        if self.is_cancelled() {
            return None;
        }
        // Copy the cache handle before importing types. Import can read declaration metadata,
        // so it must run after the shared-cache lock has been released.
        let cached = self.shared.and_then(|shared| {
            shared
                .entries
                .lock()
                .expect("declaration cache lock should not be poisoned")
                .get(&id)
                .cloned()
        });
        if let Some(data) = cached {
            self.profile.borrow_mut().shared_declaration_hits += 1;
            return Some(data.lower(cx));
        }

        // Never hold the shared lock across lowering: a declaration can name other declarations.
        // The active interner caches the working result; only the reusable template is exported.
        self.profile.borrow_mut().declaration_loads += 1;
        match self.load(cx, id) {
            Ok(Some(data)) => {
                if let Some(shared) = self.shared
                    && !cx.has_unavailable()
                {
                    let stored = Arc::new(StoredDeclaration::raise(cx, &data));
                    shared
                        .entries
                        .lock()
                        .expect("declaration cache lock should not be poisoned")
                        .insert(id, stored);
                }
                Some(data)
            }
            Ok(None) => None,
            Err(error) => {
                *self.error.borrow_mut() = Some(error);
                None
            }
        }
    }

    fn impls(&self, trait_id: TraitDefRef, filter: TraitImplFilter) -> Option<Vec<ImplRef>> {
        // Keep the source index's outer-shape rejection before lowering impl headers. For
        // example, an unsupported `&dyn Trait` header can lower to Unknown, but its reference
        // syntax still tells us it cannot implement a trait for Vec<T>.
        let (context, scope) = self.solving?;
        let impls = filter.candidates(context, trait_id);
        match impls {
            Some(mut impls) => {
                // Keep the index's ordered set while merging the local overlay. For this one
                // trait, projecting to ImplRef preserves uniqueness and needs no second set.
                match scope.local_trait_impls(trait_id) {
                    Ok(local) => impls.extend(local),
                    Err(error) => {
                        *self.error.borrow_mut() = Some(error);
                        return None;
                    }
                }
                Some(
                    impls
                        .into_iter()
                        .map(|candidate| candidate.impl_ref)
                        .collect(),
                )
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
        let (context, _) = self.solving?;
        context.item_lookup().lang_trait(source).map(DefId::Trait)
    }

    fn is_cancelled(&self) -> bool {
        self.cancelled.get()
            || self.solving.is_some_and(|(context, _)| {
                Cancelable::check_cancelled(context, "solver declarations").is_err()
            })
    }
}
