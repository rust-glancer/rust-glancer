//! Temporary type storage and the compiler solver's view of source declarations.
//!
//! The compiler algorithms ask their interner both to construct types and to read declarations.
//! This module connects those requests to one operation's storage and declaration provider.
//! Interning gives repeated type shapes the same address, so the solver can compare them cheaply.

mod availability;
mod declarations;
mod lists;

use std::{cell::RefCell, fmt};

use rustc_type_ir::{
    self as ir, TypeFoldable, TypeVisitableExt, VisitorResult,
    data_structures::HashMap,
    inherent::{GenericArgs as _, Ty as _},
    lang_items::{SolverAdtLangItem, SolverProjectionLangItem, SolverTraitLangItem},
};

use self::{
    availability::CallbackAvailability, declarations::WorkingDeclarations, lists::ListInterners,
};
pub use self::{availability::CallbackScope, lists::ListElement};
use super::{
    DeclarationKind, DeclarationProvider, Solver,
    declarations::LangItem,
    profile::SolverProfile,
    types::{
        AdtDef, Clause, Const, ConstExpr, DefId, ErrorGuaranteed, ExternalConstraints, GenericArg,
        GenericArgs, Generics, List, Param, ParamEnv, Pattern, Predicate, Region, Safety, Symbol,
        Term, Ty, TyKind, ValTree, ValueConst,
    },
};
use crate::lookup::TraitImplFilter;

/// The owner of every temporary solver value in a body or standalone query.
///
/// All candidate probes in that operation share these type nodes and declaration conversions.
/// Their variable assignments live separately in inference contexts, so rejecting a probe does
/// not require removing its allocated types. Everything is released when the operation ends.
///
/// Interned type nodes have no destructors. External constraints do own vectors, so they are
/// boxed separately and dropped along with the solver caches when this storage leaves scope.
pub struct SolverStorage<'s> {
    arena: bumpalo::Bump,
    provider: &'s dyn DeclarationProvider,
    // Each node already owns its kind. Borrow that kind for structural lookup instead of
    // copying it into the table; only the returned solver handle uses pointer equality.
    tys: RefCell<HashMap<&'s TyKind<'s>, Ty<'s>>>,
    consts: RefCell<HashMap<&'s ir::ConstKind<SolverInterner<'s>>, Const<'s>>>,
    predicates: RefCell<
        HashMap<
            &'s ir::Binder<SolverInterner<'s>, ir::PredicateKind<SolverInterner<'s>>>,
            Predicate<'s>,
        >,
    >,
    lists: ListInterners<'s>,
    // Boxes keep published addresses stable when the owner vector grows.
    #[allow(clippy::vec_box)]
    external: RefCell<Vec<Box<ir::solve::ExternalConstraintsData<SolverInterner<'s>>>>>,
    cache: RefCell<ir::search_graph::GlobalCache<SolverInterner<'s>>>,
    declarations: RefCell<WorkingDeclarations<'s>>,
    callbacks: CallbackAvailability,
    profile: RefCell<SolverProfile>,
}

impl<'s> SolverStorage<'s> {
    pub(crate) fn new(provider: &'s dyn DeclarationProvider) -> Self {
        crate::profile::metric::SOLVER_OPERATIONS.add(1);
        Self {
            arena: bumpalo::Bump::new(),
            provider,
            tys: RefCell::default(),
            consts: RefCell::default(),
            predicates: RefCell::default(),
            lists: ListInterners::default(),
            external: RefCell::default(),
            cache: RefCell::default(),
            declarations: RefCell::default(),
            callbacks: CallbackAvailability::default(),
            profile: RefCell::default(),
        }
    }

    pub fn interner(&'s self) -> SolverInterner<'s> {
        SolverInterner(self)
    }

    /// Sample storage before releasing the operation. These totals describe allocation work
    /// across bodies and queries; their sum is not the amount that was alive at the same time.
    pub(crate) fn record_profile(&self) {
        let mut profile = self.profile.borrow_mut();
        profile.arena_reserved_bytes = self.arena.allocated_bytes_including_metadata() as u64;
        profile.record_node_table(&self.tys.borrow());
        profile.record_node_table(&self.consts.borrow());
        profile.record_node_table(&self.predicates.borrow());
        self.lists.record_profile(&mut profile);
    }
}

// These are solver diagnostics, not user-facing type rendering. Avoid recursively invoking the
// same IrPrint implementation through Debug for types whose Debug delegates back to this trait.
macro_rules! debug_print {
    ($($kind:ident),+ $(,)?) => {
        $(
            impl<'s> ir::ir_print::IrPrint<ir::$kind<SolverInterner<'s>>> for SolverInterner<'s> {
                fn print(value: &ir::$kind<Self>, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                    write!(f, "{value:?}")
                }

                fn print_debug(value: &ir::$kind<Self>, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                    Self::print(value, f)
                }
            }
        )+
    };
}

/// A copyable handle to the operation's storage, usually named `cx` in compiler callbacks.
/// Copying this handle shares allocations and caches; it does not copy inference assignments.
///
/// Declaration reads through these handles also share callback reporting. A `CallbackScope`
/// identifies which reads belong to one candidate or query before its answer is accepted.
#[derive(Clone, Copy)]
pub struct SolverInterner<'s>(&'s SolverStorage<'s>);

impl fmt::Debug for SolverInterner<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SolverInterner")
    }
}

impl<'s> SolverInterner<'s> {
    pub(crate) fn profile(self, record: impl FnOnce(&mut SolverProfile)) {
        record(&mut self.0.profile.borrow_mut());
    }

    pub(crate) fn is_cancelled(self) -> bool {
        self.0.provider.is_cancelled()
    }

    /// A named item can be known before any arguments are inferred. Preserve every parameter's
    /// kind and position without importing the declaration's own parameters into that use site.
    pub fn unknown_args(self, owner: DefId) -> GenericArgs<'s> {
        self.complete_args(owner, List::default())
    }

    pub(crate) fn complete_args(self, id: DefId, args: GenericArgs<'s>) -> GenericArgs<'s> {
        // A source name can identify `Wrapper` before its arguments are known. Compiler IR still
        // requires a slot for every parameter: field and predicate instantiation indexes them.
        // Missing source information becomes an error slot, never a made-up generic parameter.
        let generics = self.generics(id);
        if generics.params.len() == args.len() {
            return args;
        }
        List::new(
            self,
            &generics
                .params
                .iter()
                .enumerate()
                .map(|(index, param)| {
                    args.get(index).copied().unwrap_or_else(|| match param {
                        rg_ir_model::GenericParamRef::Type(_) => self.unknown().into(),
                        rg_ir_model::GenericParamRef::Const(_) => {
                            Const::new(self, ir::ConstKind::Error(ErrorGuaranteed)).into()
                        }
                        rg_ir_model::GenericParamRef::Lifetime(_) => Region(ir::ReErased).into(),
                    })
                })
                .collect::<Vec<_>>(),
        )
    }

    pub(crate) fn intern_ty(self, kind: TyKind<'s>) -> Ty<'s> {
        if let Some(value) = self.0.tys.borrow().get(&kind) {
            return *value;
        }
        let flags = ir::FlagComputation::for_kind(&kind);
        let value = Ty(self.0.arena.alloc(ir::WithCachedTypeInfo {
            internee: kind,
            flags: flags.flags,
            outer_exclusive_binder: flags.outer_exclusive_binder,
        }));
        self.0.tys.borrow_mut().insert(&value.0.internee, value);
        value
    }

    pub(crate) fn intern_const(self, kind: ir::ConstKind<Self>) -> Const<'s> {
        if let Some(value) = self.0.consts.borrow().get(&kind) {
            return *value;
        }
        let flags = ir::FlagComputation::for_const_kind(&kind);
        let value = Const(self.0.arena.alloc(ir::WithCachedTypeInfo {
            internee: kind,
            flags: flags.flags,
            outer_exclusive_binder: flags.outer_exclusive_binder,
        }));
        self.0.consts.borrow_mut().insert(&value.0.internee, value);
        value
    }

    pub(crate) fn intern_predicate(
        self,
        kind: ir::Binder<Self, ir::PredicateKind<Self>>,
    ) -> Predicate<'s> {
        if let Some(value) = self.0.predicates.borrow().get(&kind) {
            return *value;
        }
        let flags = ir::FlagComputation::for_predicate(kind);
        let value = Predicate(self.0.arena.alloc(ir::WithCachedTypeInfo {
            internee: kind,
            flags: flags.flags,
            outer_exclusive_binder: flags.outer_exclusive_binder,
        }));
        self.0
            .predicates
            .borrow_mut()
            .insert(&value.0.internee, value);
        value
    }

    fn require_lang_item(self, item: LangItem) -> DefId {
        self.0.provider.lang_item(item).unwrap_or_else(|| {
            self.unavailable("missing language item");
            DefId::Unavailable
        })
    }
}

impl<'s> ir::Interner for SolverInterner<'s> {
    type DefId = DefId;
    type LocalDefId = DefId;
    type TraitId = DefId;
    type ForeignId = DefId;
    type FunctionId = DefId;
    type ClosureId = DefId;
    type CoroutineClosureId = DefId;
    type CoroutineId = DefId;
    type AdtId = DefId;
    type ImplId = DefId;
    type UnevaluatedConstId = DefId;
    type TraitAssocTyId = DefId;
    type TraitAssocConstId = DefId;
    type TraitAssocTermId = DefId;
    type OpaqueTyId = DefId;
    type LocalOpaqueTyId = DefId;
    type FreeTyAliasId = DefId;
    type FreeConstAliasId = DefId;
    type FreeTermAliasId = DefId;
    type ImplOrTraitAssocTyId = DefId;
    type ImplOrTraitAssocConstId = DefId;
    type ImplOrTraitAssocTermId = DefId;
    type InherentAssocTyId = DefId;
    type InherentAssocConstId = DefId;
    type InherentAssocTermId = DefId;
    type Span = ();
    type GenericArgs = GenericArgs<'s>;
    type GenericArgsSlice = GenericArgs<'s>;
    type GenericArg = GenericArg<'s>;
    type Term = Term<'s>;
    type BoundVarKinds = List<'s, ir::BoundVariableKind<Self>>;
    type PredefinedOpaques = List<'s, (ir::OpaqueTypeKey<Self>, Ty<'s>)>;
    fn mk_predefined_opaques_in_body(
        self,
        data: &[(ir::OpaqueTypeKey<Self>, Ty<'s>)],
    ) -> Self::PredefinedOpaques {
        List::new(self, data)
    }

    type LocalDefIds = List<'s, DefId>;
    type CanonicalVarKinds = List<'s, ir::CanonicalVarKind<Self>>;
    fn mk_canonical_var_kinds(
        self,
        kinds: &[ir::CanonicalVarKind<Self>],
    ) -> Self::CanonicalVarKinds {
        List::new(self, kinds)
    }

    type ExternalConstraints = ExternalConstraints<'s>;
    fn mk_external_constraints(
        self,
        data: ir::solve::ExternalConstraintsData<Self>,
    ) -> Self::ExternalConstraints {
        let data = Box::new(data);
        let ptr: *const ir::solve::ExternalConstraintsData<Self> = &*data;
        self.0.external.borrow_mut().push(data);
        // SAFETY: the box is never removed or mutated after publication. Its allocation is
        // stable across vector growth and lives for the storage borrow carried by this interner.
        ExternalConstraints(unsafe { &*ptr })
    }

    type DepNodeIndex = ();
    type Tracked<T: fmt::Debug + Clone> = T;
    fn mk_tracked<T: fmt::Debug + Clone>(self, data: T, _: ()) -> T {
        data
    }

    fn get_tracked<T: fmt::Debug + Clone>(self, tracked: &T) -> T {
        tracked.clone()
    }

    fn with_cached_task<T>(self, task: impl FnOnce() -> T) -> (T, ()) {
        (task(), ())
    }

    type Ty = Ty<'s>;
    type Tys = List<'s, Ty<'s>>;
    type FnInputTys = Self::Tys;
    type ParamTy = Param;
    type Symbol = Symbol<'s>;
    type ErrorGuaranteed = ErrorGuaranteed;
    type BoundExistentialPredicates = List<'s, ir::Binder<Self, ir::ExistentialPredicate<Self>>>;
    type AllocId = ();
    type Pat = Pattern<'s>;
    type PatList = List<'s, Pattern<'s>>;
    type Safety = Safety;
    type Const = Const<'s>;
    type Consts = List<'s, Const<'s>>;
    type ParamConst = Param;
    type ValueConst = ValueConst<'s>;
    type ExprConst = ConstExpr;
    type ValTree = ValTree<'s>;
    type ScalarInt = u128;
    type Region = Region<'s>;
    type EarlyParamRegion = Param;
    type LateParamRegion = Param;
    type RegionAssumptions = List<'s, ir::OutlivesPredicate<Self, GenericArg<'s>>>;
    type ParamEnv = ParamEnv<'s>;
    type Predicate = Predicate<'s>;
    type Clause = Clause<'s>;
    type Clauses = List<'s, Clause<'s>>;
    fn with_global_cache<R>(
        self,
        f: impl FnOnce(&mut ir::search_graph::GlobalCache<Self>) -> R,
    ) -> R {
        f(&mut self.0.cache.borrow_mut())
    }

    fn canonical_param_env_cache_get_or_insert<R>(
        self,
        _: ParamEnv<'s>,
        f: impl FnOnce() -> ir::CanonicalParamEnvCacheEntry<Self>,
        from: impl FnOnce(&ir::CanonicalParamEnvCacheEntry<Self>) -> R,
    ) -> R {
        from(&f())
    }

    fn assert_evaluation_is_concurrent(&self) {
        panic!("solver cache entry changed in a single-threaded operation")
    }

    fn expand_abstract_consts<T: TypeFoldable<Self>>(self, value: T) -> T {
        value
    }

    type GenericsOf = Generics<'s>;
    fn generics_of(self, id: DefId) -> Self::GenericsOf {
        self.generics(id)
    }

    type VariancesOf = List<'s, ir::Variance>;
    fn variances_of(self, id: DefId) -> Self::VariancesOf {
        // Equality inference is invariant. TODO: compute declaration variance before exposing
        // additional subtype/coercion relationships through these declarations.
        List::new(self, &vec![ir::Invariant; self.generics(id).params.len()])
    }

    fn opt_alias_variances(
        self,
        _: impl Into<ir::AliasTermKind<Self>>,
    ) -> Option<Self::VariancesOf> {
        None
    }

    fn type_of(self, id: DefId) -> ir::EarlyBinder<Self, Ty<'s>> {
        let ty = match id {
            DefId::TypeAlias(alias) => self.alias_value(alias),
            DefId::Impl(implementation) => self
                .impl_header(implementation)
                .map(|header| header.self_ty)
                .unwrap_or_else(|| self.unknown()),
            DefId::Adt(_) => Ty::new_adt(
                self,
                self.adt_def(id),
                GenericArgs::identity_for_item(self, id),
            ),
            _ => {
                self.unavailable("definition type");
                self.unknown()
            }
        };
        ir::EarlyBinder::bind(ty)
    }

    fn type_of_opaque_hir_typeck(self, _: DefId) -> ir::EarlyBinder<Self, Ty<'s>> {
        self.unavailable("opaque hidden type");
        ir::EarlyBinder::bind(Ty::new_error(self, ErrorGuaranteed))
    }

    fn is_type_const(self, _: DefId) -> bool {
        false
    }

    fn const_of_item(self, _: DefId) -> ir::EarlyBinder<Self, Const<'s>> {
        self.unavailable("constant evaluation");
        ir::EarlyBinder::bind(Const::new(self, ir::ConstKind::Error(ErrorGuaranteed)))
    }

    fn anon_const_kind(self, _: DefId) -> ir::AnonConstKind {
        self.unavailable("anonymous constant evaluation");
        ir::AnonConstKind::MCG
    }

    type AdtDef = AdtDef;
    fn adt_def(self, id: DefId) -> AdtDef {
        if let DeclarationKind::Adt(data) = self.metadata(id).kind {
            return data;
        }
        self.unavailable("ADT definition");
        AdtDef {
            id,
            is_struct: false,
            is_packed: false,
            is_phantom_data: false,
            is_manually_drop: false,
            is_fundamental: false,
        }
    }

    fn alias_ty_kind_from_def_id(self, id: DefId) -> ir::AliasTyKind<Self> {
        match id {
            DefId::Opaque(_) => ir::AliasTyKind::Opaque { def_id: id },
            DefId::TypeAlias(_) => match self.metadata(id).parent {
                Some(DefId::Trait(_)) => ir::AliasTyKind::Projection { def_id: id },
                Some(DefId::Impl(_)) => ir::AliasTyKind::Inherent { def_id: id },
                _ => ir::AliasTyKind::Free { def_id: id },
            },
            _ => {
                self.unavailable("alias definition");
                ir::AliasTyKind::Free { def_id: id }
            }
        }
    }

    fn alias_term_kind_from_def_id(self, id: DefId) -> ir::AliasTermKind<Self> {
        self.alias_ty_kind_from_def_id(id).into()
    }

    fn trait_ref_and_own_args_for_alias(
        self,
        id: DefId,
        args: GenericArgs<'s>,
    ) -> (ir::TraitRef<Self>, GenericArgs<'s>) {
        let parent = self.projection_parent(id);
        let count = self.generics(parent).params.len();
        (
            ir::TraitRef::new_from_args(self, parent, List(&args.0[..count])),
            List(&args.0[count..]),
        )
    }

    fn mk_args(self, args: &[GenericArg<'s>]) -> GenericArgs<'s> {
        List::new(self, args)
    }

    fn mk_args_from_iter<I, T>(self, args: I) -> T::Output
    where
        I: Iterator<Item = T>,
        T: ir::CollectAndApply<GenericArg<'s>, GenericArgs<'s>>,
    {
        T::collect_and_apply(args, |args| self.mk_args(args))
    }

    fn check_args_compatible(self, id: DefId, args: GenericArgs<'s>) -> bool {
        // A missing mandatory language item already marks this operation unavailable. Let the
        // compiler solver unwind to that boundary without asserting about a fictitious arity.
        id == DefId::Unavailable || self.generics(id).params.len() == args.len()
    }

    fn debug_assert_args_compatible(self, id: DefId, args: GenericArgs<'s>) {
        debug_assert!(
            self.check_args_compatible(id, args),
            "argument count for {id:?}"
        );
    }

    fn debug_assert_existential_args_compatible(self, id: DefId, args: GenericArgs<'s>) {
        debug_assert_eq!(self.generics(id).params.len(), args.len() + 1);
    }

    fn mk_type_list_from_iter<I, T>(self, args: I) -> T::Output
    where
        I: Iterator<Item = T>,
        T: ir::CollectAndApply<Ty<'s>, Self::Tys>,
    {
        T::collect_and_apply(args, |args| List::new(self, args))
    }

    fn projection_parent(self, id: DefId) -> DefId {
        self.metadata(id).parent.unwrap_or_else(|| {
            self.unavailable("projection parent");
            DefId::Unavailable
        })
    }

    fn impl_or_trait_assoc_term_parent(self, id: DefId) -> DefId {
        self.projection_parent(id)
    }

    fn inherent_alias_term_parent(self, id: DefId) -> DefId {
        self.projection_parent(id)
    }

    fn recursion_limit(self) -> usize {
        64
    }

    type Features = Features;
    fn features(self) -> Features {
        Features
    }

    fn coroutine_hidden_types(
        self,
        _: DefId,
    ) -> ir::EarlyBinder<Self, ir::Binder<Self, ir::CoroutineWitnessTypes<Self>>> {
        self.unavailable("coroutine hidden types");
        ir::EarlyBinder::bind(ir::Binder::dummy(ir::CoroutineWitnessTypes {
            types: List::default(),
            assumptions: List::default(),
        }))
    }

    fn fn_sig(self, id: DefId) -> ir::EarlyBinder<Self, ir::Binder<Self, ir::FnSig<Self>>> {
        let DefId::Function(function) = id else {
            self.unavailable("function identity");
            return ir::EarlyBinder::bind(ir::Binder::dummy(ir::FnSig::dummy()));
        };
        let Some(sig) = self.function_signature(function) else {
            return ir::EarlyBinder::bind(ir::Binder::dummy(ir::FnSig::dummy()));
        };
        let tys = sig.params.iter().chain([sig.ret]).collect::<Vec<_>>();
        ir::EarlyBinder::bind(ir::Binder::dummy(ir::FnSig {
            inputs_and_output: List::new(self, &tys),
            fn_sig_kind: ir::FnSigKind::new(
                rustc_abi::ExternAbi::Rust,
                Safety(!sig.qualifiers.is_unsafe),
                false,
            ),
        }))
    }

    fn coroutine_movability(self, _: DefId) -> ir::Movability {
        self.unavailable("coroutine movability");
        ir::Movability::Static
    }

    fn coroutine_for_closure(self, _: DefId) -> DefId {
        self.unavailable("coroutine closure");
        DefId::Unavailable
    }

    fn generics_require_sized_self(self, _: DefId) -> bool {
        false
    }

    fn item_bounds(self, id: DefId) -> ir::EarlyBinder<Self, impl IntoIterator<Item = Clause<'s>>> {
        ir::EarlyBinder::bind(ir::elaborate::elaborate(self, self.bounds(id)).collect::<Vec<_>>())
    }

    fn item_self_bounds(
        self,
        id: DefId,
    ) -> ir::EarlyBinder<Self, impl IntoIterator<Item = Clause<'s>>> {
        ir::EarlyBinder::bind(
            ir::elaborate::elaborate(self, self.bounds(id))
                .filter_only_self()
                .collect::<Vec<_>>(),
        )
    }

    fn item_non_self_bounds(
        self,
        id: DefId,
    ) -> ir::EarlyBinder<Self, impl IntoIterator<Item = Clause<'s>>> {
        let own = self
            .item_self_bounds(id)
            .skip_binder()
            .into_iter()
            .collect::<Vec<_>>();
        ir::EarlyBinder::bind(
            self.item_bounds(id)
                .skip_binder()
                .into_iter()
                .filter(|c| !own.contains(c))
                .collect::<Vec<_>>(),
        )
    }

    fn predicates_of(
        self,
        id: DefId,
    ) -> ir::EarlyBinder<Self, impl IntoIterator<Item = Clause<'s>>> {
        ir::EarlyBinder::bind(self.predicates(id))
    }

    fn own_predicates_of(
        self,
        id: DefId,
    ) -> ir::EarlyBinder<Self, impl IntoIterator<Item = Clause<'s>>> {
        self.predicates_of(id)
    }

    fn explicit_super_predicates_of(
        self,
        id: DefId,
    ) -> ir::EarlyBinder<Self, impl IntoIterator<Item = (Clause<'s>, ())>> {
        ir::EarlyBinder::bind(
            self.predicates(id)
                .into_iter()
                .map(|c| (c, ()))
                .collect::<Vec<_>>(),
        )
    }

    fn explicit_implied_predicates_of(
        self,
        id: DefId,
    ) -> ir::EarlyBinder<Self, impl IntoIterator<Item = (Clause<'s>, ())>> {
        self.explicit_super_predicates_of(id)
    }

    fn impl_super_outlives(
        self,
        _: DefId,
    ) -> ir::EarlyBinder<Self, impl IntoIterator<Item = Clause<'s>>> {
        ir::EarlyBinder::bind(Vec::new())
    }

    fn impl_is_const(self, _: DefId) -> bool {
        false
    }

    fn fn_is_const(self, _: DefId) -> bool {
        false
    }

    fn closure_is_const(self, _: DefId) -> bool {
        false
    }

    fn alias_has_const_conditions(self, _: DefId) -> bool {
        false
    }

    fn const_conditions(
        self,
        _: DefId,
    ) -> ir::EarlyBinder<Self, impl IntoIterator<Item = ir::Binder<Self, ir::TraitRef<Self>>>> {
        ir::EarlyBinder::bind(Vec::new())
    }

    fn explicit_implied_const_bounds(
        self,
        _: DefId,
    ) -> ir::EarlyBinder<Self, impl IntoIterator<Item = ir::Binder<Self, ir::TraitRef<Self>>>> {
        ir::EarlyBinder::bind(Vec::new())
    }

    fn impl_self_is_guaranteed_unsized(self, id: DefId) -> bool {
        self.type_of(id).skip_binder().is_guaranteed_unsized_raw()
    }

    fn has_target_features(self, _: DefId) -> bool {
        false
    }

    fn require_projection_lang_item(self, item: SolverProjectionLangItem) -> DefId {
        self.require_lang_item(LangItem::Projection(item))
    }

    fn require_trait_lang_item(self, item: SolverTraitLangItem) -> DefId {
        self.require_lang_item(LangItem::Trait(item))
    }

    fn require_adt_lang_item(self, item: SolverAdtLangItem) -> DefId {
        self.require_lang_item(LangItem::Adt(item))
    }

    fn is_projection_lang_item(self, id: DefId, item: SolverProjectionLangItem) -> bool {
        self.as_projection_lang_item(id) == Some(item)
    }

    fn is_trait_lang_item(self, id: DefId, item: SolverTraitLangItem) -> bool {
        self.as_trait_lang_item(id) == Some(item)
    }

    fn is_adt_lang_item(self, id: DefId, item: SolverAdtLangItem) -> bool {
        self.as_adt_lang_item(id) == Some(item)
    }

    fn is_default_trait(self, _: DefId) -> bool {
        false
    }

    fn is_sizedness_trait(self, id: DefId) -> bool {
        matches!(
            self.as_trait_lang_item(id),
            Some(
                SolverTraitLangItem::Sized
                    | SolverTraitLangItem::MetaSized
                    | SolverTraitLangItem::PointeeSized
            )
        )
    }

    fn as_projection_lang_item(self, id: DefId) -> Option<SolverProjectionLangItem> {
        match self.metadata(id).lang_item {
            Some(LangItem::Projection(item)) => Some(item),
            _ => None,
        }
    }

    fn as_trait_lang_item(self, id: DefId) -> Option<SolverTraitLangItem> {
        match self.metadata(id).lang_item {
            Some(LangItem::Trait(item)) => Some(item),
            _ => None,
        }
    }

    fn as_adt_lang_item(self, id: DefId) -> Option<SolverAdtLangItem> {
        match self.metadata(id).lang_item {
            Some(LangItem::Adt(item)) => Some(item),
            _ => None,
        }
    }

    fn associated_type_def_ids(self, id: DefId) -> impl IntoIterator<Item = DefId> {
        match &self.metadata(id).kind {
            DeclarationKind::Trait {
                associated_types, ..
            } => associated_types.clone(),
            _ => Vec::new(),
        }
    }

    fn for_each_relevant_impl<R: VisitorResult>(
        self,
        id: DefId,
        ty: Ty<'s>,
        mut f: impl FnMut(DefId) -> R,
    ) -> R {
        // The enclosing root will roll back after an unsupported callback. More candidates
        // cannot repair that evaluation, and an error type could make each of them seem viable.
        if self.has_unavailable() {
            return R::output();
        }
        let DefId::Trait(id) = id else {
            self.unavailable("trait identity");
            return R::output();
        };
        let Some(impls) = self.0.provider.impls(id, TraitImplFilter::from(ty)) else {
            self.unavailable("impl enumeration");
            return R::output();
        };
        for id in impls {
            if self.has_unavailable() {
                break;
            }
            self.profile(|p| p.impl_candidates += 1);
            if self.is_cancelled() {
                self.unavailable("cancelled");
                break;
            }
            if let std::ops::ControlFlow::Break(residual) = f(DefId::Impl(id)).branch() {
                return R::from_residual(residual);
            }
        }
        R::output()
    }

    fn for_each_blanket_impl<R: VisitorResult>(
        self,
        id: DefId,
        mut f: impl FnMut(DefId) -> R,
    ) -> R {
        if self.has_unavailable() {
            return R::output();
        }
        let DefId::Trait(id) = id else {
            self.unavailable("trait identity");
            return R::output();
        };
        let Some(impls) = self.0.provider.impls(id, TraitImplFilter::All) else {
            self.unavailable("impl enumeration");
            return R::output();
        };
        for id in impls {
            if self.has_unavailable() {
                break;
            }
            self.profile(|p| p.impl_candidates += 1);
            if let std::ops::ControlFlow::Break(residual) = f(DefId::Impl(id)).branch() {
                return R::from_residual(residual);
            }
        }
        R::output()
    }

    fn has_item_definition(self, id: DefId) -> bool {
        matches!(
            self.metadata(id).kind,
            DeclarationKind::Alias { has_value: true }
        )
    }

    fn impl_specializes(self, _: DefId, _: DefId) -> bool {
        false
    }

    fn impl_is_default(self, _: DefId) -> bool {
        false
    }

    fn impl_trait_ref(self, id: DefId) -> ir::EarlyBinder<Self, ir::TraitRef<Self>> {
        if let DefId::Impl(implementation) = id
            && let Some(header) = self.impl_header(implementation)
            && let Some(tr) = header.trait_ref
        {
            let reference = ir::TraitRef::new_from_args(
                self,
                DefId::Trait(tr.def),
                self.complete_args(DefId::Trait(tr.def), tr.args),
            );
            // Compiler error types relate to anything for recovery. An incomplete header must
            // not turn that recovery rule into proof of an impl.
            if reference.references_error() {
                self.unavailable("incomplete impl header");
            }
            return ir::EarlyBinder::bind(reference);
        }
        self.unavailable("impl trait reference");
        ir::EarlyBinder::bind(ir::TraitRef::new_from_args(
            self,
            DefId::Unavailable,
            List::default(),
        ))
    }

    fn impl_polarity(self, _: DefId) -> ir::ImplPolarity {
        // TODO: Retain impl polarity in source declarations before supporting negative impls.
        ir::ImplPolarity::Positive
    }

    fn trait_is_auto(self, id: DefId) -> bool {
        matches!(
            self.metadata(id).kind,
            DeclarationKind::Trait { is_auto: true, .. }
        )
    }

    fn trait_is_coinductive(self, id: DefId) -> bool {
        self.trait_is_auto(id) || self.is_sizedness_trait(id)
    }

    fn trait_is_alias(self, _: DefId) -> bool {
        false
    }

    fn trait_is_dyn_compatible(self, _: DefId) -> bool {
        self.unavailable("dyn compatibility");
        false
    }

    fn trait_is_fundamental(self, _: DefId) -> bool {
        false
    }

    fn trait_is_unsafe(self, id: DefId) -> bool {
        matches!(
            self.metadata(id).kind,
            DeclarationKind::Trait {
                is_unsafe: true,
                ..
            }
        )
    }

    fn is_impl_trait_in_trait(self, _: DefId) -> bool {
        false
    }

    fn delay_bug(self, _: impl ToString) -> ErrorGuaranteed {
        self.unavailable("solver delayed error");
        ErrorGuaranteed
    }

    fn is_general_coroutine(self, _: DefId) -> bool {
        false
    }

    fn coroutine_is_async(self, _: DefId) -> bool {
        false
    }

    fn coroutine_is_gen(self, _: DefId) -> bool {
        false
    }

    fn coroutine_is_async_gen(self, _: DefId) -> bool {
        false
    }

    type UnsizingParams = Box<rustc_index::bit_set::DenseBitSet<u32>>;
    fn unsizing_params_for_adt(self, _: DefId) -> Self::UnsizingParams {
        self.unavailable("ADT unsizing parameters");
        Box::new(rustc_index::bit_set::DenseBitSet::new_empty(0))
    }

    fn anonymize_bound_vars<T: TypeFoldable<Self>>(
        self,
        binder: ir::Binder<Self, T>,
    ) -> ir::Binder<Self, T> {
        self.anonymize(binder)
    }

    fn opaque_types_defined_by(self, _: DefId) -> Self::LocalDefIds {
        List::default()
    }

    fn opaque_types_and_coroutines_defined_by(self, _: DefId) -> Self::LocalDefIds {
        List::default()
    }

    type Probe = Box<ir::solve::inspect::Probe<Self>>;
    fn mk_probe(self, probe: ir::solve::inspect::Probe<Self>) -> Self::Probe {
        Box::new(probe)
    }

    fn evaluate_root_goal_for_proof_tree_raw(
        self,
        input: ir::solve::CanonicalInput<Self>,
    ) -> (ir::solve::QueryResult<Self>, Self::Probe) {
        rustc_next_trait_solver::solve::evaluate_root_goal_for_proof_tree_raw_provider::<
            Solver<'s>,
            Self,
        >(self, input)
    }

    fn item_name(self, id: DefId) -> Symbol<'s> {
        Symbol(self.0.arena.alloc_str(&self.metadata(id).name))
    }
}

impl<'s> ir::ir_print::IrPrint<ir::TraitRef<SolverInterner<'s>>> for SolverInterner<'s> {
    fn print(t: &ir::TraitRef<Self>, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("TraitRef")
            .field(&t.def_id)
            .field(&t.args)
            .finish()
    }

    fn print_debug(t: &ir::TraitRef<Self>, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Self::print(t, f)
    }
}

impl<'s> ir::ir_print::IrPrint<ir::ExistentialTraitRef<SolverInterner<'s>>> for SolverInterner<'s> {
    fn print(t: &ir::ExistentialTraitRef<Self>, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("ExistentialTraitRef")
            .field(&t.def_id)
            .field(&t.args)
            .finish()
    }

    fn print_debug(t: &ir::ExistentialTraitRef<Self>, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Self::print(t, f)
    }
}

impl<'s> ir::ir_print::IrPrint<ir::PatternKind<SolverInterner<'s>>> for SolverInterner<'s> {
    fn print(_: &ir::PatternKind<Self>, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("unsupported pattern type")
    }

    fn print_debug(t: &ir::PatternKind<Self>, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Self::print(t, f)
    }
}

debug_print!(
    AliasTy,
    AliasTerm,
    TraitPredicate,
    HostEffectPredicate,
    ExistentialProjection,
    ProjectionPredicate,
    NormalizesTo,
    SubtypePredicate,
    CoercePredicate,
    FnSig
);

#[derive(Clone, Copy)]
pub struct Features;

impl<'s> ir::inherent::Features<SolverInterner<'s>> for Features {
    fn generic_const_exprs(self) -> bool {
        false
    }

    fn generic_const_args(self) -> bool {
        false
    }

    fn coroutine_clone(self) -> bool {
        false
    }

    fn feature_bound_holds_in_crate(self, _: Symbol<'s>) -> bool {
        false
    }
}
