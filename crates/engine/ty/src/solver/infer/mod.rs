//! The inference state shared by body relations and trait solving.
//!
//! Ena snapshots cover variable assignments. The same guard also rolls back universes, opaque
//! definitions, and newly allocated region variables, so a rejected candidate leaves no evidence.

mod generalize;
mod variables;

use std::cell::{Cell, RefCell};

use ena::unify::{InPlace, InPlaceUnificationTable, Snapshot as EnaSnapshot, UnifyKey};
use rustc_type_ir::{
    self as ir, InferCtxtLike, TypeFoldable, TypeFolder, TypeSuperFoldable, TypeVisitableExt,
    data_structures::HashMap,
    inherent::{Const as _, IntoKind, Region as _, Ty as _},
    relate::{RelateResult, combine::PredicateEmittingRelation},
};

use self::variables::{Key, Value};
use super::{Const, DefId, GenericArg, GenericArgs, List, Region, SolverInterner, Ty};

/// Variable assignments used by the compiler's type relations and goal evaluation.
///
/// A type such as `Vec<?T>` only carries a variable id. This context knows whether `?T` is still
/// open, equal to another variable, or assigned to `u8`. Both body constraints and solver answers
/// update these same tables. Assumptions and the queue of goals to solve belong to `InferenceTable`.
#[derive(Clone)]
pub struct InferCtxt<'s> {
    pub(crate) interner: SolverInterner<'s>,
    mode: ir::TypingMode<SolverInterner<'s>>,
    universe: Cell<ir::UniverseIndex>,
    state: RefCell<State<'s>>,
    tainted: Cell<bool>,
}

impl<'s> InferCtxt<'s> {
    pub(crate) fn new(
        interner: SolverInterner<'s>,
        mode: ir::TypingMode<SolverInterner<'s>>,
    ) -> Self {
        Self {
            interner,
            mode,
            universe: Cell::new(ir::UniverseIndex::ROOT),
            state: RefCell::default(),
            tainted: Cell::new(false),
        }
    }

    pub fn snapshot(&self) -> Snapshot<'_, 's> {
        let mut s = self.state.borrow_mut();
        Snapshot {
            infcx: self,
            universe: self.universe(),
            regions: s.regions.len(),
            opaques: s.opaques.len(),
            duplicates: s.duplicates.len(),
            tainted: self.tainted.get(),
            tables: Some(VariableSnapshots {
                types: s.types.snapshot(),
                subtypes: s.subtypes.snapshot(),
                consts: s.consts.snapshot(),
                ints: s.ints.snapshot(),
                floats: s.floats.snapshot(),
            }),
        }
    }

    pub(crate) fn fallback_numeric(&self) {
        let mut state = self.state.borrow_mut();
        for index in 0..state.ints.len() {
            let id = ir::IntVid::from_usize(index);
            if state.ints.probe_value(id) == ir::IntVarValue::Unknown {
                state
                    .ints
                    .unify_var_value(id, ir::IntVarValue::IntType(ir::IntTy::I32))
                    .expect("unbound numeric variable");
            }
        }
        for index in 0..state.floats.len() {
            let id = ir::FloatVid::from_usize(index);
            if state.floats.probe_value(id) == ir::FloatVarValue::Unknown {
                state
                    .floats
                    .unify_var_value(id, ir::FloatVarValue::Known(ir::FloatTy::F64))
                    .expect("unbound numeric variable");
            }
        }
    }

    pub fn next_int_var(&self) -> Ty<'s> {
        Ty::new_infer(
            self.interner,
            ir::IntVar(
                self.state
                    .borrow_mut()
                    .ints
                    .new_key(ir::IntVarValue::Unknown),
            ),
        )
    }

    pub fn next_float_var(&self) -> Ty<'s> {
        Ty::new_infer(
            self.interner,
            ir::FloatVar(
                self.state
                    .borrow_mut()
                    .floats
                    .new_key(ir::FloatVarValue::Unknown),
            ),
        )
    }

    pub(crate) fn next_ty_var_in_universe(&self, universe: ir::UniverseIndex) -> Ty<'s> {
        let mut s = self.state.borrow_mut();
        let key = s.types.new_key(Value::Unknown(universe));
        let subkey = s.subtypes.new_key(Value::Unknown(universe));
        debug_assert_eq!(key.index(), subkey.index());
        Ty::new_var(self.interner, ir::TyVid::from_u32(key.index()))
    }

    pub(crate) fn next_const_var_in_universe(&self, universe: ir::UniverseIndex) -> Const<'s> {
        let key = self
            .state
            .borrow_mut()
            .consts
            .new_key(Value::Unknown(universe));
        Const::new_var(self.interner, ir::ConstVid::from_u32(key.index()))
    }

    pub(crate) fn next_region_var_in_universe(&self, universe: ir::UniverseIndex) -> Region<'s> {
        let mut s = self.state.borrow_mut();
        let vid = ir::RegionVid::from_usize(s.regions.len());
        s.regions.push(universe);
        Region(ir::ReVar(vid))
    }

    fn ty_value(&self, vid: ir::TyVid) -> Value<Ty<'s>> {
        self.state
            .borrow_mut()
            .types
            .probe_value(Key::new(vid.as_u32()))
    }

    fn const_value(&self, vid: ir::ConstVid) -> Value<Const<'s>> {
        self.state
            .borrow_mut()
            .consts
            .probe_value(Key::new(vid.as_u32()))
    }

    pub(crate) fn assign_ty(&self, vid: ir::TyVid, ty: Ty<'s>) {
        self.state
            .borrow_mut()
            .types
            .union_value(Key::new(vid.as_u32()), Value::Known(ty));
    }

    pub(crate) fn assign_const(&self, vid: ir::ConstVid, ct: Const<'s>) {
        self.state
            .borrow_mut()
            .consts
            .union_value(Key::new(vid.as_u32()), Value::Known(ct));
    }

    pub(crate) fn universe_of_region(&self, region: Region<'s>) -> ir::UniverseIndex {
        match region.kind() {
            ir::ReVar(v) => self.state.borrow().regions[v.as_usize()],
            ir::RePlaceholder(p) => p.universe,
            _ => ir::UniverseIndex::ROOT,
        }
    }

    pub(crate) fn is_tainted(&self) -> bool {
        self.tainted.get()
    }
}

impl<'s> InferCtxtLike for InferCtxt<'s> {
    type Interner = SolverInterner<'s>;
    fn cx(&self) -> Self::Interner {
        self.interner
    }

    fn disable_trait_solver_fast_paths(&self) -> bool {
        false
    }

    fn typing_mode_raw(&self) -> ir::TypingMode<Self::Interner> {
        self.mode
    }

    fn universe(&self) -> ir::UniverseIndex {
        self.universe.get()
    }

    fn create_next_universe(&self) -> ir::UniverseIndex {
        let u = self.universe().next_universe();
        self.universe.set(u);
        u
    }

    fn universe_of_ty(&self, v: ir::TyVid) -> Option<ir::UniverseIndex> {
        match self.ty_value(v) {
            Value::Unknown(u) => Some(u),
            Value::Known(_) => None,
        }
    }

    fn universe_of_ct(&self, v: ir::ConstVid) -> Option<ir::UniverseIndex> {
        match self.const_value(v) {
            Value::Unknown(u) => Some(u),
            Value::Known(_) => None,
        }
    }

    fn universe_of_lt(&self, v: ir::RegionVid) -> Option<ir::UniverseIndex> {
        self.state.borrow().regions.get(v.as_usize()).copied()
    }

    fn root_ty_var(&self, v: ir::TyVid) -> ir::TyVid {
        ir::TyVid::from_u32(
            self.state
                .borrow_mut()
                .types
                .find(Key::new(v.as_u32()))
                .index(),
        )
    }

    fn sub_unification_table_root_var(&self, v: ir::TyVid) -> ir::TyVid {
        ir::TyVid::from_u32(
            self.state
                .borrow_mut()
                .subtypes
                .find(Key::new(v.as_u32()))
                .index(),
        )
    }

    fn root_const_var(&self, v: ir::ConstVid) -> ir::ConstVid {
        ir::ConstVid::from_u32(
            self.state
                .borrow_mut()
                .consts
                .find(Key::new(v.as_u32()))
                .index(),
        )
    }

    fn opportunistic_resolve_ty_var(&self, v: ir::TyVid) -> Ty<'s> {
        match self.ty_value(v) {
            Value::Known(ty) => ty,
            Value::Unknown(_) => Ty::new_var(self.interner, self.root_ty_var(v)),
        }
    }

    fn opportunistic_resolve_ct_var(&self, v: ir::ConstVid) -> Const<'s> {
        match self.const_value(v) {
            Value::Known(ct) => ct,
            Value::Unknown(_) => Const::new_var(self.interner, self.root_const_var(v)),
        }
    }

    fn opportunistic_resolve_int_var(&self, v: ir::IntVid) -> Ty<'s> {
        let mut s = self.state.borrow_mut();
        match s.ints.probe_value(v) {
            ir::IntVarValue::IntType(t) => Ty::new(self.interner, ir::Int(t)),
            ir::IntVarValue::UintType(t) => Ty::new(self.interner, ir::Uint(t)),
            ir::IntVarValue::Unknown => Ty::new_infer(self.interner, ir::IntVar(s.ints.find(v))),
        }
    }

    fn opportunistic_resolve_float_var(&self, v: ir::FloatVid) -> Ty<'s> {
        let mut s = self.state.borrow_mut();
        match s.floats.probe_value(v) {
            ir::FloatVarValue::Known(t) => Ty::new(self.interner, ir::Float(t)),
            ir::FloatVarValue::Unknown => {
                Ty::new_infer(self.interner, ir::FloatVar(s.floats.find(v)))
            }
        }
    }

    fn opportunistic_resolve_lt_var(&self, v: ir::RegionVid) -> Region<'s> {
        Region(ir::ReVar(v))
    }

    fn is_changed_arg(&self, arg: GenericArg<'s>) -> bool {
        match arg.kind() {
            ir::GenericArgKind::Type(t) => self.shallow_resolve(t) != t,
            ir::GenericArgKind::Const(c) => self.shallow_resolve_const(c) != c,
            ir::GenericArgKind::Lifetime(_) => false,
        }
    }

    fn next_ty_infer(&self) -> Ty<'s> {
        self.next_ty_var_in_universe(self.universe())
    }

    fn next_const_infer(&self) -> Const<'s> {
        self.next_const_var_in_universe(self.universe())
    }

    fn next_region_infer(&self) -> Region<'s> {
        self.next_region_var_in_universe(self.universe())
    }

    fn fresh_args_for_item(&self, id: DefId) -> GenericArgs<'s> {
        let args = self
            .interner
            .generics(id)
            .params
            .iter()
            .map(|p| match p {
                rg_ir_model::GenericParamRef::Type(_) => self.next_ty_infer().into(),
                rg_ir_model::GenericParamRef::Const(_) => self.next_const_infer().into(),
                rg_ir_model::GenericParamRef::Lifetime(_) => self.next_region_infer().into(),
            })
            .collect::<Vec<_>>();
        List::new(self.interner, &args)
    }

    fn instantiate_binder_with_infer<T: TypeFoldable<Self::Interner> + Copy>(
        &self,
        value: ir::Binder<Self::Interner, T>,
    ) -> T {
        let mut vars = HashMap::default();
        self.interner.replace_bound_vars(value, |var, kind| {
            *vars.entry(var).or_insert_with(|| match kind {
                ir::BoundVariableKind::Ty(_) => self.next_ty_infer().into(),
                ir::BoundVariableKind::Region(_) => self.next_region_infer().into(),
                ir::BoundVariableKind::Const => self.next_const_infer().into(),
            })
        })
    }

    fn enter_forall<T: TypeFoldable<Self::Interner>, U>(
        &self,
        value: ir::Binder<Self::Interner, T>,
        f: impl FnOnce(T) -> U,
    ) -> U {
        // A binder's placeholders share one fresh universe. Generalization prevents them from
        // escaping into a type variable which was created outside this binder.
        if value.bound_vars().is_empty() {
            return f(value.skip_binder());
        }
        let universe = self.create_next_universe();
        let result = self
            .interner
            .replace_bound_vars(value, |var, kind| match kind {
                ir::BoundVariableKind::Ty(kind) => Ty::new_placeholder(
                    self.interner,
                    ir::PlaceholderType::new(universe, ir::BoundTy { var, kind }),
                )
                .into(),
                ir::BoundVariableKind::Region(kind) => Region::new_placeholder(
                    self.interner,
                    ir::PlaceholderRegion::new(universe, ir::BoundRegion { var, kind }),
                )
                .into(),
                ir::BoundVariableKind::Const => Const::new_placeholder(
                    self.interner,
                    ir::PlaceholderConst::new(universe, ir::BoundConst::new(var)),
                )
                .into(),
            });
        f(result)
    }

    fn equate_ty_vids_raw(&self, a: ir::TyVid, b: ir::TyVid) {
        let mut s = self.state.borrow_mut();
        s.types.union(Key::new(a.as_u32()), Key::new(b.as_u32()));
        s.subtypes.union(Key::new(a.as_u32()), Key::new(b.as_u32()));
    }

    fn sub_unify_ty_vids_raw(&self, a: ir::TyVid, b: ir::TyVid) {
        self.state
            .borrow_mut()
            .subtypes
            .union(Key::new(a.as_u32()), Key::new(b.as_u32()));
    }

    fn equate_const_vids_raw(&self, a: ir::ConstVid, b: ir::ConstVid) {
        self.state
            .borrow_mut()
            .consts
            .union(Key::new(a.as_u32()), Key::new(b.as_u32()));
    }

    fn equate_int_vids_raw(&self, a: ir::IntVid, b: ir::IntVid) {
        self.state
            .borrow_mut()
            .ints
            .unify_var_var(a, b)
            .expect("raw integer variables are unresolved");
    }

    fn equate_float_vids_raw(&self, a: ir::FloatVid, b: ir::FloatVid) {
        self.state
            .borrow_mut()
            .floats
            .unify_var_var(a, b)
            .expect("raw float variables are unresolved");
    }

    fn instantiate_int_var_raw(&self, v: ir::IntVid, value: ir::IntVarValue) {
        self.state
            .borrow_mut()
            .ints
            .unify_var_value(v, value)
            .expect("raw integer variable is unresolved");
    }

    fn instantiate_float_var_raw(&self, v: ir::FloatVid, value: ir::FloatVarValue) {
        self.state
            .borrow_mut()
            .floats
            .unify_var_value(v, value)
            .expect("raw float variable is unresolved");
    }

    fn instantiate_ty_var_raw<R: PredicateEmittingRelation<Self>>(
        &self,
        relation: &mut R,
        expected: bool,
        target: ir::TyVid,
        variance: ir::Variance,
        source: Ty<'s>,
    ) -> RelateResult<Self::Interner, ()> {
        self.instantiate_ty_var(relation, expected, target, variance, source)
    }

    fn instantiate_const_var_raw<R: PredicateEmittingRelation<Self>>(
        &self,
        relation: &mut R,
        expected: bool,
        target: ir::ConstVid,
        source: Const<'s>,
    ) -> RelateResult<Self::Interner, ()> {
        self.instantiate_const_var(relation, expected, target, source)
    }

    fn set_tainted_by_errors(&self, _: super::types::ErrorGuaranteed) {
        self.tainted.set(true);
    }

    fn shallow_resolve(&self, ty: Ty<'s>) -> Ty<'s> {
        match ty.kind() {
            ir::Infer(ir::TyVar(v)) => match self.ty_value(v) {
                // Type variables can resolve to numeric variables; their numeric assignment is
                // another root lookup, not a structural child visited by a type folder.
                Value::Known(known) => self.shallow_resolve(known),
                Value::Unknown(_) => Ty::new_var(self.interner, self.root_ty_var(v)),
            },
            ir::Infer(ir::IntVar(v)) => self.opportunistic_resolve_int_var(v),
            ir::Infer(ir::FloatVar(v)) => self.opportunistic_resolve_float_var(v),
            _ => ty,
        }
    }

    fn shallow_resolve_const(&self, ct: Const<'s>) -> Const<'s> {
        match ct.kind() {
            ir::ConstKind::Infer(ir::InferConst::Var(v)) => self.opportunistic_resolve_ct_var(v),
            _ => ct,
        }
    }

    fn resolve_vars_if_possible<T: TypeFoldable<Self::Interner>>(&self, value: T) -> T {
        // Most declaration types contain only parameters and concrete types. Their cached flags
        // tell us that resolving inference variables cannot change any part of the value.
        if !value.has_non_region_infer() {
            return value;
        }

        struct Resolver<'a, 's>(&'a InferCtxt<'s>);

        impl<'s> TypeFolder<SolverInterner<'s>> for Resolver<'_, 's> {
            fn cx(&self) -> SolverInterner<'s> {
                self.0.interner
            }

            fn fold_ty(&mut self, t: Ty<'s>) -> Ty<'s> {
                if t.has_non_region_infer() {
                    self.0.shallow_resolve(t).super_fold_with(self)
                } else {
                    t
                }
            }

            fn fold_const(&mut self, c: Const<'s>) -> Const<'s> {
                if c.has_non_region_infer() {
                    self.0.shallow_resolve_const(c).super_fold_with(self)
                } else {
                    c
                }
            }
        }
        value.fold_with(&mut Resolver(self))
    }

    fn probe<T>(&self, f: impl FnOnce() -> T) -> T {
        let _snapshot = self.snapshot();
        f()
    }

    // Region identities participate in binders, but region validity is deliberately outside
    // Glancer's inference. As in rust-analyzer, these constraints do not invoke a borrow checker.
    fn sub_regions(&self, _: Region<'s>, _: Region<'s>, _: ir::solve::VisibleForLeakCheck, _: ()) {}

    fn equate_regions(
        &self,
        _: Region<'s>,
        _: Region<'s>,
        _: ir::solve::VisibleForLeakCheck,
        _: (),
    ) {
    }

    fn register_ty_outlives(&self, _: Ty<'s>, _: Region<'s>, _: ()) {}

    type OpaqueTypeStorageEntries = OpaqueEntries;
    fn opaque_types_storage_num_entries(&self) -> OpaqueEntries {
        let s = self.state.borrow();
        OpaqueEntries {
            unique: s.opaques.len(),
            duplicates: s.duplicates.len(),
        }
    }

    fn clone_opaque_types_lookup_table(&self) -> Vec<(ir::OpaqueTypeKey<Self::Interner>, Ty<'s>)> {
        self.state.borrow().opaques.clone()
    }

    fn clone_duplicate_opaque_types(&self) -> Vec<(ir::OpaqueTypeKey<Self::Interner>, Ty<'s>)> {
        self.state.borrow().duplicates.clone()
    }

    fn clone_opaque_types_added_since(
        &self,
        prev: OpaqueEntries,
    ) -> Vec<(ir::OpaqueTypeKey<Self::Interner>, Ty<'s>)> {
        let s = self.state.borrow();
        s.opaques[prev.unique..]
            .iter()
            .chain(&s.duplicates[prev.duplicates..])
            .copied()
            .collect()
    }

    fn opaques_with_sub_unified_hidden_type(
        &self,
        vid: ir::TyVid,
    ) -> Vec<ir::AliasTy<Self::Interner>> {
        let root = self.sub_unification_table_root_var(vid);
        self.clone_opaque_types_lookup_table()
            .into_iter()
            .filter_map(|(key, ty)| {
                if let ir::Infer(ir::TyVar(v)) = self.shallow_resolve(ty).kind()
                    && self.sub_unification_table_root_var(v) == root
                {
                    Some(ir::AliasTy::new_from_args(
                        self.interner,
                        ir::AliasTyKind::Opaque { def_id: key.def_id },
                        key.args,
                    ))
                } else {
                    None
                }
            })
            .collect()
    }

    fn register_hidden_type_in_storage(
        &self,
        key: ir::OpaqueTypeKey<Self::Interner>,
        ty: Ty<'s>,
        _: (),
    ) -> Option<Ty<'s>> {
        let mut s = self.state.borrow_mut();
        if let Some((_, prev)) = s.opaques.iter().find(|(k, _)| *k == key) {
            Some(*prev)
        } else {
            s.opaques.push((key, ty));
            None
        }
    }

    fn add_duplicate_opaque_type(&self, key: ir::OpaqueTypeKey<Self::Interner>, ty: Ty<'s>, _: ()) {
        self.state.borrow_mut().duplicates.push((key, ty));
    }

    fn reset_opaque_types(&self) {
        let mut s = self.state.borrow_mut();
        s.opaques.clear();
        s.duplicates.clear();
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OpaqueEntries {
    unique: usize,
    duplicates: usize,
}

impl ir::inherent::OpaqueTypeStorageEntries for OpaqueEntries {
    fn needs_reevaluation(self, canonicalized: usize) -> bool {
        self.unique != canonicalized
    }
}

#[derive(Clone, Default)]
struct State<'s> {
    types: InPlaceUnificationTable<Key<Ty<'s>>>,
    // Subtype-related variables may need to be reconsidered together without being equal.
    // Keep those connections separately from the table that assigns actual types.
    subtypes: InPlaceUnificationTable<Key<()>>,
    consts: InPlaceUnificationTable<Key<Const<'s>>>,
    ints: InPlaceUnificationTable<ir::IntVid>,
    floats: InPlaceUnificationTable<ir::FloatVid>,
    regions: Vec<ir::UniverseIndex>,
    opaques: Vec<(ir::OpaqueTypeKey<SolverInterner<'s>>, Ty<'s>)>,
    duplicates: Vec<(ir::OpaqueTypeKey<SolverInterner<'s>>, Ty<'s>)>,
}

struct VariableSnapshots<'s> {
    types: EnaSnapshot<InPlace<Key<Ty<'s>>>>,
    subtypes: EnaSnapshot<InPlace<Key<()>>>,
    consts: EnaSnapshot<InPlace<Key<Const<'s>>>>,
    ints: EnaSnapshot<InPlace<ir::IntVid>>,
    floats: EnaSnapshot<InPlace<ir::FloatVid>>,
}

/// A trial within one inference context. Dropping it undoes the trial; `commit` keeps it.
///
/// Restoring only assignments would leave behind other evidence from a rejected candidate,
/// such as a proposed hidden type for `impl Trait`. The guard also restores that evidence and
/// the universe in which new variables are created. Allocated type nodes can remain shared.
pub struct Snapshot<'a, 's> {
    infcx: &'a InferCtxt<'s>,
    tables: Option<VariableSnapshots<'s>>,
    universe: ir::UniverseIndex,
    regions: usize,
    opaques: usize,
    duplicates: usize,
    tainted: bool,
}

impl Snapshot<'_, '_> {
    pub fn commit(mut self) {
        let t = self.tables.take().expect("snapshot is active");
        let mut s = self.infcx.state.borrow_mut();
        s.types.commit(t.types);
        s.subtypes.commit(t.subtypes);
        s.consts.commit(t.consts);
        s.ints.commit(t.ints);
        s.floats.commit(t.floats);
    }
}

impl Drop for Snapshot<'_, '_> {
    fn drop(&mut self) {
        if let Some(t) = self.tables.take() {
            let mut s = self.infcx.state.borrow_mut();
            s.types.rollback_to(t.types);
            s.subtypes.rollback_to(t.subtypes);
            s.consts.rollback_to(t.consts);
            s.ints.rollback_to(t.ints);
            s.floats.rollback_to(t.floats);
            s.regions.truncate(self.regions);
            s.opaques.truncate(self.opaques);
            s.duplicates.truncate(self.duplicates);
            self.infcx.universe.set(self.universe);
            self.infcx.tainted.set(self.tainted);
        }
    }
}
