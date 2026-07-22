//! Goal-directed construction of the Rust program exposed to Chalk.
//!
//! A trait query starts with a small set of semantic roots, such as `Iterator` and the opaque
//! types appearing in its arguments. This module expands those roots into every declaration that
//! Chalk may ask about, lowers that closed set into Chalk datums, and then keeps the resulting
//! database for later queries.
//!
//! Read the module in three steps:
//!
//! 1. `roots` discovers the semantic dependency closure of a goal.
//! 2. `build` materializes traits, impls, associated types, opaque types, and referenced ADTs.
//! 3. `database` answers Chalk's callbacks from that materialized data.

mod build;
mod database;
mod roots;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use chalk_ir::{
    AliasTy as ChalkAliasTy, GenericArgData, Substitution as ChalkSubstitution, TyKind, Variances,
    WhereClause,
};
use chalk_solve::rust_ir::{
    AssociatedTyDatum, AssociatedTyValue, FnDefDatum, ImplDatum, OpaqueTyDatum, TraitDatum,
    WellKnownTrait,
};
use rg_def_map::DefMapSource;
use rg_ir_model::{
    FunctionRef, GenericDefRef, ImplRef, OpaqueTyRef, TraitDefRef, TypeAliasRef, TypeDefRef,
};
use rg_item_tree::LangItem;
use rg_semantic_ir::{CrateItemQuery, ItemLookupIndex, ItemStoreSource};
use rg_std::UniqueVec;
use rg_text::Name;

use super::interner::RgChalkInterner;
use crate::inference::InferenceTable;
use crate::trait_selection::{TraitGoal, TraitSelectionSession};
use crate::{Clause, ItemPathQuery, TraitRefLowering};

const INTER: RgChalkInterner = RgChalkInterner;
// Program extensions are relatively rare; a subsecond threshold still filters ordinary root
// discovery while exposing extensions that materially contribute to a slow body.
const SLOW_PROGRAM_EXTENSION: Duration = Duration::from_millis(100);

/// The growing Chalk database owned by one solver instance.
///
/// Roots record which query entry points have already been loaded. The program contains their
/// full dependency closure. Adding a new root extends the same program instead of rebuilding the
/// traits and impls needed by earlier queries.
pub(super) struct ChalkProgramState {
    roots: ChalkProgramRoots,
    program: ChalkProgram,
}

/// Semantic definitions that can become entry points into one Chalk program.
///
/// A goal normally contributes one or two traits. Opaque types are roots too because their bounds
/// can introduce traits that are not written directly in the outer goal.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ChalkProgramRoots {
    traits: UniqueVec<TraitDefRef>,
    opaque_tys: UniqueVec<OpaqueTyRef>,
    functions: UniqueVec<FunctionRef>,
}

/// The complete semantic input discovered from a set of new roots.
///
/// This is temporary build data. It keeps canonical headers beside their identities so the build
/// phase does not repeat semantic lowering while it turns the discovered closure into Chalk
/// datums.
#[derive(Default)]
struct ChalkProgramScope {
    definitions: ChalkProgramRoots,
    trait_headers: HashMap<TraitDefRef, crate::signature::TraitHeader>,
    impls: UniqueVec<ImplRef>,
    impl_headers: HashMap<ImplRef, crate::ImplHeader>,
    opaque_bounds: HashMap<OpaqueTyRef, (crate::OpaqueTy, Vec<TraitRefLowering>)>,
    function_signatures: HashMap<FunctionRef, crate::CallableSignature>,
    loaded_opaque_owners: UniqueVec<GenericDefRef>,
}

/// Materialized Chalk datums and the lookup indexes needed by Chalk's database callbacks.
///
/// The program only contains definitions reachable from goals seen by its solver. Once a trait is
/// materialized, however, all of its visible impls are added together so extending the program
/// later cannot invalidate answers already retained in Chalk's solver forests.
#[derive(Debug)]
pub(super) struct ChalkProgram {
    materialized_traits: UniqueVec<TraitDefRef>,
    materialized_opaque_owners: UniqueVec<GenericDefRef>,
    known_items: ChalkKnownItems,
    traits: HashMap<TraitDefRef, Arc<TraitDatum<RgChalkInterner>>>,
    trait_arities: HashMap<TraitDefRef, usize>,
    associated_tys: HashMap<TypeAliasRef, Arc<AssociatedTyDatum<RgChalkInterner>>>,
    associated_ty_by_trait_name: HashMap<(TraitDefRef, Name), TypeAliasRef>,
    associated_ty_values: HashMap<TypeAliasRef, Arc<AssociatedTyValue<RgChalkInterner>>>,
    associated_ty_value_by_impl: HashMap<(ImplRef, TypeAliasRef), TypeAliasRef>,
    opaque_tys: HashMap<OpaqueTyRef, Arc<OpaqueTyDatum<RgChalkInterner>>>,
    functions: HashMap<FunctionRef, Arc<FnDefDatum<RgChalkInterner>>>,
    adts: HashMap<TypeDefRef, Arc<chalk_solve::rust_ir::AdtDatum<RgChalkInterner>>>,
    adt_variances: HashMap<TypeDefRef, Variances<RgChalkInterner>>,
    impls: HashMap<ImplRef, Arc<ImplDatum<RgChalkInterner>>>,
    impls_by_trait: HashMap<TraitDefRef, Vec<ImplRef>>,
}

/// Exact compiler-known identities used by Chalk's callable built-ins.
#[derive(Debug, Default)]
struct ChalkKnownItems {
    fn_trait: Option<TraitDefRef>,
    fn_mut_trait: Option<TraitDefRef>,
    fn_once_trait: Option<TraitDefRef>,
    fn_once_output: Option<TypeAliasRef>,
}

impl ChalkKnownItems {
    fn from_index(index: &ItemLookupIndex) -> Self {
        Self {
            fn_trait: index.lang_trait(LangItem::Fn),
            fn_mut_trait: index.lang_trait(LangItem::FnMut),
            fn_once_trait: index.lang_trait(LangItem::FnOnce),
            fn_once_output: index.lang_type_alias(LangItem::FnOnceOutput),
        }
    }

    fn well_known_trait(
        &self,
        trait_ref: TraitDefRef,
        associated_ty_ids: &[chalk_ir::AssocTypeId<RgChalkInterner>],
    ) -> Option<WellKnownTrait> {
        if self.fn_trait == Some(trait_ref) {
            return Some(WellKnownTrait::Fn);
        }
        if self.fn_mut_trait == Some(trait_ref) {
            return Some(WellKnownTrait::FnMut);
        }
        if self.fn_once_trait == Some(trait_ref)
            && self.fn_once_output.is_some_and(|output| {
                associated_ty_ids == [super::lower::chalk_assoc_type_id(output)]
            })
        {
            return Some(WellKnownTrait::FnOnce);
        }
        None
    }

    fn trait_ref(&self, well_known: WellKnownTrait) -> Option<TraitDefRef> {
        match well_known {
            WellKnownTrait::Fn => self.fn_trait,
            WellKnownTrait::FnMut => self.fn_mut_trait,
            WellKnownTrait::FnOnce => self.fn_once_trait,
            _ => None,
        }
    }
}

impl ChalkProgramState {
    pub(super) fn new() -> Self {
        Self {
            roots: ChalkProgramRoots::default(),
            program: ChalkProgram::empty(),
        }
    }

    /// Make every definition reachable from these clauses available to Chalk.
    pub(super) fn ensure_for_clauses<'query, D, I>(
        &mut self,
        item_paths: &ItemPathQuery<'query, D, I>,
        crate_items: &CrateItemQuery<'query, D, I>,
        lookup_index: &ItemLookupIndex,
        session: &TraitSelectionSession,
        clauses: &[Clause],
        table: Option<&InferenceTable>,
    ) -> Result<bool, I::Error>
    where
        D: DefMapSource<Error = I::Error>,
        I: ItemStoreSource<'query>,
    {
        let mut roots = ChalkProgramRoots::default();
        roots.collect_clauses(item_paths, clauses, table)?;
        self.ensure_roots(item_paths, crate_items, lookup_index, session, &roots)
    }

    /// Make every definition reachable from this projection goal available to Chalk.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn ensure_for_goal<'query, D, I>(
        &mut self,
        item_paths: &ItemPathQuery<'query, D, I>,
        crate_items: &CrateItemQuery<'query, D, I>,
        lookup_index: &ItemLookupIndex,
        session: &TraitSelectionSession,
        goal: &TraitGoal,
        associated_ty: TypeAliasRef,
        table: &InferenceTable,
    ) -> Result<bool, I::Error>
    where
        D: DefMapSource<Error = I::Error>,
        I: ItemStoreSource<'query>,
    {
        let mut roots = ChalkProgramRoots::default();
        roots.collect_goal(item_paths, goal, table)?;
        roots.collect_associated_ty(item_paths, associated_ty)?;
        self.ensure_roots(item_paths, crate_items, lookup_index, session, &roots)
    }

    fn ensure_roots<'query, D, I>(
        &mut self,
        item_paths: &ItemPathQuery<'query, D, I>,
        crate_items: &CrateItemQuery<'query, D, I>,
        lookup_index: &ItemLookupIndex,
        session: &TraitSelectionSession,
        roots: &ChalkProgramRoots,
    ) -> Result<bool, I::Error>
    where
        D: DefMapSource<Error = I::Error>,
        I: ItemStoreSource<'query>,
    {
        let pending_roots = roots.new_since(&self.roots);
        if !pending_roots.is_empty() {
            crate::profile::metric::PROGRAM_BUILDS.inc();
            let started = Instant::now();
            let result = self.program.extend(
                item_paths,
                crate_items,
                lookup_index,
                session,
                &pending_roots,
            );
            let elapsed = started.elapsed();
            crate::profile::metric::PROGRAM_BUILD_TIME.record(elapsed);
            if elapsed >= SLOW_PROGRAM_EXTENSION {
                tracing::debug!(
                    elapsed_ms = elapsed.as_millis(),
                    trait_roots = pending_roots.traits.len(),
                    trait_root_ids = ?pending_roots.traits.as_slice(),
                    opaque_type_roots = pending_roots.opaque_tys.len(),
                    function_roots = pending_roots.functions.len(),
                    succeeded = result.is_ok(),
                    "slow Chalk program extension"
                );
            }
            result?;
            self.roots.merge(&pending_roots);
        }

        // A trait is materialized with every visible impl before its first query, and an opaque is
        // materialized before its identity enters a goal. Later extensions therefore cannot
        // invalidate answers retained by the solver forests; they only add new goal identities.
        // Function callbacks have a stronger invariant: every function type in this query must
        // have real datum. Unsupported signatures are omitted by the builder and stop the query
        // before Chalk can ask its mandatory callback for fabricated data.
        Ok(roots
            .functions
            .iter()
            .all(|function| self.program.functions.contains_key(function)))
    }

    pub(super) fn database(&self) -> &ChalkProgram {
        &self.program
    }

    pub(super) fn associated_tys(
        &self,
    ) -> &HashMap<TypeAliasRef, Arc<AssociatedTyDatum<RgChalkInterner>>> {
        &self.program.associated_tys
    }

    pub(super) fn functions(&self) -> &HashMap<FunctionRef, Arc<FnDefDatum<RgChalkInterner>>> {
        &self.program.functions
    }

    /// Instantiate the associated value owned by one already-selected Chalk impl datum.
    ///
    /// Trait selection has already proved the impl's predicates before this is called. Reading the
    /// materialized datum here completes that selection inside the Chalk adapter; it does not
    /// lower the source alias again or create a second project-side projection engine.
    pub(super) fn selected_associated_ty_value(
        &self,
        impl_ref: ImplRef,
        associated_ty_ref: TypeAliasRef,
        args: &chalk_ir::Substitution<RgChalkInterner>,
    ) -> Option<chalk_ir::Ty<RgChalkInterner>> {
        let value_ref = self
            .program
            .associated_ty_value_by_impl
            .get(&(impl_ref, associated_ty_ref))?;
        let value = self.program.associated_ty_values.get(value_ref)?;
        (value.value.len(INTER) == args.len(INTER))
            .then(|| value.value.clone().substitute(INTER, args).ty)
    }

    /// Read an associated equality carried by the receiver's materialized opaque datum.
    ///
    /// Opaque bounds are environment evidence rather than impl candidates. Substitute both the
    /// opaque owner's arguments and Chalk's dedicated `Self` binder, then require an exact match
    /// with the projection being normalized.
    pub(super) fn opaque_associated_ty_value(
        &self,
        alias: &ChalkAliasTy<RgChalkInterner>,
    ) -> Option<chalk_ir::Ty<RgChalkInterner>> {
        let ChalkAliasTy::Projection(projection) = alias else {
            return None;
        };
        let self_arg = projection.substitution.iter(INTER).next()?;
        let GenericArgData::Ty(self_ty) = self_arg.data(INTER) else {
            return None;
        };
        let TyKind::OpaqueType(opaque_id, opaque_args) = self_ty.kind(INTER) else {
            return None;
        };
        let super::interner::ChalkDefId::Opaque(opaque_ref) = opaque_id.0 else {
            return None;
        };
        let datum = self.program.opaque_tys.get(&opaque_ref)?;
        if datum.bound.len(INTER) != opaque_args.len(INTER) {
            return None;
        }
        let bound = datum.bound.clone().substitute(INTER, opaque_args);
        let self_subst = ChalkSubstitution::from_iter(INTER, [self_arg.clone()]);
        let clauses = bound.bounds.substitute(INTER, &self_subst);

        clauses.into_iter().find_map(|clause| {
            if clause.len(INTER) != 0 {
                return None;
            }
            let WhereClause::AliasEq(equality) = clause.skip_binders() else {
                return None;
            };
            (equality.alias == *alias).then(|| equality.ty.clone())
        })
    }
}
