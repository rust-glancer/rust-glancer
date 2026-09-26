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
    AssocItemId, DefMapRef, FunctionRef, GenericDefRef, GenericParamRef, ImplRef, ItemOwner,
    TraitDefRef, TraitImplRef, TypeAliasRef, TypeDefId, TypeDefRef,
};
use rg_semantic_ir::{ItemStoreSource, TypePathContext};
use rg_std::Cancelable;
use rustc_type_ir::{
    ClauseKind,
    inherent::IntoKind as _,
    lang_items::{SolverAdtLangItem, SolverProjectionLangItem, SolverTraitLangItem},
};

use self::stored::StoredDeclarations;
use super::{
    CallableSignature, Clause, DefId, ImplHeader, InferenceSubstitution, InferenceTable, List,
    ProjectionTy, Solver, SolverInterner, SolverStorage, TraitApplication, Ty,
    profile::SolverProfile, types::AdtDef,
};
use crate::{
    TyContext,
    lookup::{ItemPathQuery, TraitImplFilter},
    lowering::{TypeLoweringAnchor, TypeLoweringEnv, TypeLoweringQuery, TypePathResolver},
};

/// Facts that can be read without resolving a type or a bound. For example, identifying the
/// parent trait of `Iterator::Item` must not lower `Item`'s default type or the trait's predicates.
/// Generic parameter identity and order also come from syntax, not from instantiated types.
pub(crate) struct DeclarationMetadata {
    pub name: String,
    pub generics: Vec<GenericParamRef>,
    pub parent_count: usize,
    pub parent: Option<DefId>,
    pub lang_item: Option<LangItem>,
    pub kind: DeclarationKind,
}

pub(crate) enum DeclarationKind {
    Adt(AdtDef),
    Trait {
        is_auto: bool,
        is_unsafe: bool,
        associated_types: Vec<DefId>,
    },
    Impl {
        associated_types: Vec<(String, DefId)>,
    },
    Alias {
        has_value: bool,
    },
    Other,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum LangItem {
    Trait(SolverTraitLangItem),
    Projection(SolverProjectionLangItem),
    Adt(SolverAdtLangItem),
    Deref,
    DerefTarget,
}

/// Supply declaration metadata, types, and bounds to the solver as separate reads.
///
/// For `impl Widget where Self::Item: Clone`, lowering the bound needs to find `Item` in the
/// impl. That lookup needs the impl's receiver, so reading a header must work without reading
/// the predicates whose lowering requested it.
pub(crate) trait DeclarationProvider {
    fn metadata(&self, id: DefId) -> Option<Arc<DeclarationMetadata>>;
    fn impl_header<'s>(&self, cx: SolverInterner<'s>, id: ImplRef) -> Option<ImplHeader<'s>>;
    fn function_signature<'s>(
        &self,
        cx: SolverInterner<'s>,
        id: FunctionRef,
    ) -> Option<CallableSignature<'s>>;
    fn predicates<'s>(&self, cx: SolverInterner<'s>, id: DefId) -> Option<List<'s, Clause<'s>>>;
    fn bounds<'s>(&self, cx: SolverInterner<'s>, id: DefId) -> Option<List<'s, Clause<'s>>>;
    fn alias_value<'s>(&self, cx: SolverInterner<'s>, id: TypeAliasRef) -> Option<Ty<'s>>;
    fn field_tys<'s>(&self, cx: SolverInterner<'s>, id: TypeDefRef) -> Option<List<'s, Ty<'s>>>;
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

/// Owned declaration data shared by operations in one immutable lexical context.
///
/// A type path such as `Self::Item` is lowered through the caller's resolver. Even a saved
/// declaration identity may see an edited impl, so this cache belongs to the body context rather
/// than a crate or a process. Compiler types and inference answers never enter this cache.
#[derive(Clone, Default)]
pub struct DeclarationCache {
    entries: Arc<Mutex<StoredDeclarations>>,
}

/// Load and lower declaration data for a body or editor query using its semantic snapshot
/// and path resolver.
///
/// Separate operations in the same lexical context can reuse owned data from `DeclarationCache`.
/// Each operation imports the requested parts into its own interner, where they live until that
/// operation ends.
///
/// Compiler callbacks ask for declarations as they explore a goal. This provider loads those
/// declarations on demand, using the same path resolver as the body or editor query. If a read
/// fails, it saves the error: the compiler's callback API cannot return our storage errors, but
/// the enclosing `with_solver` call can.
/// Callback availability tracking prevents accepting a solver answer based on that failed read.
/// Keeping the underlying error here also lets the operation report the storage failure itself.
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

    /// Give a standalone query the same assumptions as inference in its enclosing declaration.
    /// The table and all its types belong to this operation; return owned results from the closure.
    pub fn with_table<R>(
        &self,
        run: impl for<'s> FnOnce(InferenceTable<'s>, &'s [GenericParamRef]) -> R,
    ) -> Result<R, I::Error> {
        self.with_solver(|solver| {
            let cx = solver.interner();
            let (params, env) = match self.solving.and_then(|(_, scope)| scope.generic_owner()) {
                Some(owner) => (
                    cx.params(owner.into()),
                    cx.parameter_environment(owner.into()),
                ),
                None => (&[][..], Default::default()),
            };
            run(InferenceTable::new(solver, env), params)
        })
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

    fn load_metadata(&self, id: DefId) -> Result<Option<DeclarationMetadata>, I::Error> {
        let Some(owner) = id.generic_owner() else {
            return Ok(None);
        };
        let items = self.paths.items();
        let Some(item) = items.semantic_item_view(owner.into())? else {
            return Ok(None);
        };
        let generics = self.paths.generics().generics(owner)?;
        let mut result = DeclarationMetadata {
            name: item.name().map(ToString::to_string).unwrap_or_default(),
            generics: generics.iter().map(|p| p.param()).collect(),
            parent_count: generics.parent_len(),
            parent: item
                .item_owner()
                .and_then(|parent| Self::parent(owner.origin(), parent)),
            lang_item: None,
            kind: DeclarationKind::Other,
        };
        match id {
            DefId::Trait(id) => {
                let Some(data) = items.trait_data(id)? else {
                    return Ok(None);
                };
                // TODO: Retain `auto` in source declarations before enabling structural impls.
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
                let mut associated_types = Vec::new();
                for item in item.assoc_items().unwrap_or_default() {
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
                result.kind = DeclarationKind::Impl { associated_types };
            }
            DefId::TypeAlias(id) => {
                let Some(data) = items.type_alias_data(id)? else {
                    return Ok(None);
                };
                result.kind = DeclarationKind::Alias {
                    has_value: data.signature.aliased_ty().is_some(),
                };
            }
            DefId::Adt(id) => result.kind = DeclarationKind::Adt(AdtDef::new(id)),
            // An opaque occurrence inherits its owner's generics, but querying its bounds is
            // what checks the occurrence itself. No signature walk is needed for metadata.
            DefId::Opaque(_) => {
                result.parent = Some(owner.into());
            }
            DefId::Function(_) | DefId::Const(_) | DefId::Static(_) => {}
            DefId::Closure(_) | DefId::Unavailable => unreachable!(),
        }
        Ok(Some(result))
    }

    /// Prepare lowered predicates or type bounds for solver callbacks, then intern the list.
    fn solver_clauses<'s>(
        &self,
        cx: SolverInterner<'s>,
        mut clauses: Vec<Clause<'s>>,
    ) -> List<'s, Clause<'s>> {
        // PointeeSized is a tautology in compiler IR, not a source impl to prove. Keep it out of
        // both requirements and associated-type promises before handing clauses to the solver.
        let pointee_sized = self.lang_item(LangItem::Trait(SolverTraitLangItem::PointeeSized));
        clauses.retain(|clause| {
            !matches!(
                clause.kind().skip_binder(),
                ClauseKind::Trait(tr) if Some(tr.trait_ref.def_id) == pointee_sized
            )
        });
        List::new(cx, &clauses)
    }

    /// Read an owned cache entry and release the lock before the caller imports its types.
    /// Import and lowering can request another declaration, which needs to lock the cache again.
    fn cached<T>(&self, read: impl FnOnce(&StoredDeclarations) -> Option<T>) -> Option<T> {
        let value = self.shared.and_then(|shared| {
            read(
                &shared
                    .entries
                    .lock()
                    .expect("declaration cache lock should not be poisoned"),
            )
        });
        if value.is_some() {
            self.profile.borrow_mut().shared_declaration_hits += 1;
        }
        value
    }

    fn cache(&self, write: impl FnOnce(&mut StoredDeclarations)) {
        if let Some(shared) = self.shared {
            write(
                &mut shared
                    .entries
                    .lock()
                    .expect("declaration cache lock should not be poisoned"),
            );
        }
    }

    /// Run a declaration read unless the operation was cancelled, retaining any storage error
    /// for the enclosing operation to return. Solver callbacks receive `None` on failure because
    /// their API cannot carry our storage errors.
    fn load<T>(&self, read: impl FnOnce() -> Result<Option<T>, I::Error>) -> Option<T> {
        if self.is_cancelled() {
            return None;
        }
        self.profile.borrow_mut().declaration_loads += 1;
        match read() {
            Ok(value) => value,
            Err(error) => {
                *self.error.borrow_mut() = Some(error);
                None
            }
        }
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
    fn metadata(&self, id: DefId) -> Option<Arc<DeclarationMetadata>> {
        if let Some(data) = self.cached(|cache| cache.metadata.get(&id).cloned()) {
            return Some(data);
        }
        let data = Arc::new(self.load(|| self.load_metadata(id))?);
        self.cache(|cache| {
            cache.metadata.insert(id, data.clone());
        });
        Some(data)
    }

    fn impl_header<'s>(&self, cx: SolverInterner<'s>, id: ImplRef) -> Option<ImplHeader<'s>> {
        if let Some(data) = self.cached(|cache| cache.impl_headers.get(&id).cloned()) {
            let params = cx.params(DefId::Impl(id));
            return Some(ImplHeader {
                owner: id,
                self_ty: cx.lower_ty(&data.self_ty, params),
                trait_ref: data.trait_ref.as_ref().map(|tr| TraitApplication {
                    def: tr.def,
                    args: cx.lower_args(&tr.args, params),
                }),
            });
        }
        let data =
            self.load(|| TypeLoweringQuery::new(self.paths, &self.resolver).impl_header(cx, id))?;
        if self.shared.is_some() && !cx.has_unavailable() {
            let stored = Arc::new(data.raise(cx));
            self.cache(|cache| {
                cache.impl_headers.insert(id, stored);
            });
        }
        Some(data)
    }

    fn function_signature<'s>(
        &self,
        cx: SolverInterner<'s>,
        id: FunctionRef,
    ) -> Option<CallableSignature<'s>> {
        if let Some(data) = self.cached(|cache| cache.functions.get(&id).cloned()) {
            let params = cx.params(DefId::Function(id));
            return Some(CallableSignature {
                params: List::new(
                    cx,
                    &data
                        .params
                        .iter()
                        .map(|ty| cx.lower_ty(ty, params))
                        .collect::<Vec<_>>(),
                ),
                ret: cx.lower_ty(&data.ret, params),
                qualifiers: data.qualifiers,
            });
        }
        let data =
            self.load(|| TypeLoweringQuery::new(self.paths, &self.resolver).function(cx, id))?;
        if self.shared.is_some() && !cx.has_unavailable() {
            let stored = Arc::new(data.raise(cx));
            self.cache(|cache| {
                cache.functions.insert(id, stored);
            });
        }
        Some(data)
    }

    fn predicates<'s>(&self, cx: SolverInterner<'s>, id: DefId) -> Option<List<'s, Clause<'s>>> {
        if let Some(data) = self.cached(|cache| cache.predicates.get(&id).cloned()) {
            let params = cx.params(id);
            return Some(List::new(
                cx,
                &data
                    .iter()
                    .map(|clause| cx.lower_clause(clause, params))
                    .collect::<Vec<_>>(),
            ));
        }
        let owner = id.generic_owner()?;
        let clauses =
            self.load(|| TypeLoweringQuery::new(self.paths, &self.resolver).predicates(cx, owner))?;
        let clauses = self.solver_clauses(cx, clauses);
        if self.shared.is_some() && !cx.has_unavailable() {
            let stored = clauses
                .iter()
                .map(|clause| cx.raise_clause(clause))
                .collect();
            self.cache(|cache| {
                cache.predicates.insert(id, stored);
            });
        }
        Some(clauses)
    }

    fn bounds<'s>(&self, cx: SolverInterner<'s>, id: DefId) -> Option<List<'s, Clause<'s>>> {
        if let Some(data) = self.cached(|cache| cache.bounds.get(&id).cloned()) {
            let params = cx.params(id);
            return Some(List::new(
                cx,
                &data
                    .iter()
                    .map(|clause| cx.lower_clause(clause, params))
                    .collect::<Vec<_>>(),
            ));
        }
        let clauses = self.load(|| {
            let lowering = TypeLoweringQuery::new(self.paths, &self.resolver);
            let mut clauses = Vec::new();
            match id {
                DefId::TypeAlias(alias) => {
                    let Some(data) = self.paths.items().type_alias_data(alias)? else {
                        return Ok(None);
                    };
                    if matches!(data.owner, ItemOwner::Trait(_)) {
                        let owner = GenericDefRef::TypeAlias(alias);
                        let Some(context) = self
                            .paths
                            .items()
                            .type_path_context_for_generic_def(owner)?
                        else {
                            return Ok(None);
                        };
                        let params = cx.params(id);
                        let subject = cx.projection(ProjectionTy {
                            associated_ty: alias,
                            args: InferenceSubstitution::identity(cx, params.iter().copied())
                                .args_for(cx, params.iter().copied()),
                        });
                        let mut lower = lowering.session(
                            cx,
                            TypeLoweringEnv::new(owner, TypeLoweringAnchor::Context(context)),
                        )?;
                        for bound in data.signature.bounds() {
                            if let Some(ty) = bound.required_trait_ty()
                                && let Some(bound) = lower.lower_trait_ref(ty, subject)?
                            {
                                clauses.extend(bound.clauses(cx));
                            }
                        }
                    }
                }
                DefId::Opaque(opaque) => {
                    let bounds = lowering.opaque_bounds_for_owner(cx, opaque.owner)?;
                    let Some((_, bounds)) = bounds
                        .iter()
                        .find(|(candidate, _)| candidate.opaque == opaque)
                    else {
                        return Ok(None);
                    };
                    clauses.extend(bounds.iter().flat_map(|bound| bound.clauses(cx)));
                }
                _ => {
                    // No bounds is a valid answer for other declarations, but a missing
                    // declaration must still make this callback unavailable.
                    if matches!(cx.metadata(id).kind, DeclarationKind::Unavailable) {
                        return Ok(None);
                    }
                }
            }
            Ok(Some(clauses))
        })?;
        let clauses = self.solver_clauses(cx, clauses);
        if self.shared.is_some() && !cx.has_unavailable() {
            let stored = clauses
                .iter()
                .map(|clause| cx.raise_clause(clause))
                .collect();
            self.cache(|cache| {
                cache.bounds.insert(id, stored);
            });
        }
        Some(clauses)
    }

    fn alias_value<'s>(&self, cx: SolverInterner<'s>, id: TypeAliasRef) -> Option<Ty<'s>> {
        if let Some(data) = self.cached(|cache| cache.alias_values.get(&id).cloned()) {
            return Some(cx.lower_ty(&data, cx.params(DefId::TypeAlias(id))));
        }
        // An associated declaration without a default names a projection, but does not supply
        // a value for type_of. Keep that distinct from lowering the alias's name in source code.
        if !matches!(
            cx.metadata(DefId::TypeAlias(id)).kind,
            DeclarationKind::Alias { has_value: true }
        ) {
            return None;
        }
        let ty =
            self.load(|| TypeLoweringQuery::new(self.paths, &self.resolver).type_alias_ty(cx, id))?;
        if self.shared.is_some() && !cx.has_unavailable() {
            let stored = Arc::new(cx.raise_ty(ty).unwrap_or(crate::Ty::Unknown));
            self.cache(|cache| {
                cache.alias_values.insert(id, stored);
            });
        }
        Some(ty)
    }

    fn field_tys<'s>(&self, cx: SolverInterner<'s>, id: TypeDefRef) -> Option<List<'s, Ty<'s>>> {
        if let Some(data) = self.cached(|cache| cache.fields.get(&id).cloned()) {
            let params = cx.params(DefId::Adt(id));
            return Some(List::new(
                cx,
                &data
                    .iter()
                    .map(|ty| cx.lower_ty(ty, params))
                    .collect::<Vec<_>>(),
            ));
        }
        let fields = self.load(|| {
            let items = self.paths.items();
            let Some(store) = items.item_store_for_origin(id.origin)? else {
                return Ok(None);
            };
            let (module, fields) = match id.id {
                TypeDefId::Struct(id) => {
                    let Some(data) = store.struct_data(id) else {
                        return Ok(None);
                    };
                    (data.owner, data.fields.fields().iter().collect::<Vec<_>>())
                }
                TypeDefId::Enum(id) => {
                    let Some(data) = store.enum_data(id) else {
                        return Ok(None);
                    };
                    (
                        data.owner,
                        data.variants
                            .iter()
                            .flat_map(|v| v.fields.fields())
                            .collect(),
                    )
                }
                TypeDefId::Union(id) => {
                    let Some(data) = store.union_data(id) else {
                        return Ok(None);
                    };
                    (data.owner, data.fields.iter().collect())
                }
            };
            let lowering = TypeLoweringQuery::new(self.paths, &self.resolver);
            let mut lower = lowering.session(
                cx,
                TypeLoweringEnv::new(
                    id.into(),
                    TypeLoweringAnchor::Context(TypePathContext::module(module)),
                ),
            )?;
            fields
                .into_iter()
                .map(|field| lower.lower_type_ref(&field.ty))
                .collect::<Result<Vec<_>, _>>()
                .map(Some)
        })?;
        let fields = List::new(cx, &fields);
        if self.shared.is_some() && !cx.has_unavailable() {
            let stored = fields
                .iter()
                .map(|ty| cx.raise_ty(ty).unwrap_or(crate::Ty::Unknown))
                .collect();
            self.cache(|cache| {
                cache.fields.insert(id, stored);
            });
        }
        Some(fields)
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
        let (context, _) = self.solving?;
        if item == LangItem::DerefTarget {
            return context
                .item_lookup()
                .lang_type_alias(rg_item_tree::LangItem::DerefTarget)
                .map(DefId::TypeAlias);
        }
        let source = match item {
            LangItem::Deref => rg_item_tree::LangItem::Deref,
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
        context.item_lookup().lang_trait(source).map(DefId::Trait)
    }

    fn is_cancelled(&self) -> bool {
        self.cancelled.get()
            || self.solving.is_some_and(|(context, _)| {
                Cancelable::check_cancelled(context, "solver declarations").is_err()
            })
    }
}
