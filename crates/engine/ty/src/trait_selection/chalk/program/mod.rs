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
use std::time::Instant;

use chalk_ir::Variances;
use chalk_solve::rust_ir::{
    AssociatedTyDatum, AssociatedTyValue, ImplDatum, OpaqueTyDatum, TraitDatum,
};
use rg_def_map::DefMapSource;
use rg_ir_model::{GenericDefRef, ImplRef, OpaqueTyRef, TraitDefRef, TypeAliasRef, TypeDefRef};
use rg_semantic_ir::{CrateItemQuery, ItemStoreSource};
use rg_std::UniqueVec;
use rg_text::Name;

use super::interner::RgChalkInterner;
use crate::inference::InferenceTable;
use crate::trait_selection::{TraitGoal, TraitSelectionCache};
use crate::{Clause, ItemPathQuery, TraitRefLowering};

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
}

/// The complete semantic input discovered from a set of new roots.
///
/// This is temporary build data. It keeps canonical headers beside their identities so the build
/// phase does not repeat semantic lowering while it turns the discovered closure into Chalk
/// datums.
#[derive(Default)]
struct ChalkProgramScope {
    definitions: ChalkProgramRoots,
    trait_headers: HashMap<TraitDefRef, crate::TraitHeader>,
    impls: UniqueVec<ImplRef>,
    impl_headers: HashMap<ImplRef, crate::ImplHeader>,
    opaque_bounds: HashMap<OpaqueTyRef, (crate::OpaqueTy, Vec<TraitRefLowering>)>,
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
    traits: HashMap<TraitDefRef, Arc<TraitDatum<RgChalkInterner>>>,
    trait_arities: HashMap<TraitDefRef, usize>,
    associated_tys: HashMap<TypeAliasRef, Arc<AssociatedTyDatum<RgChalkInterner>>>,
    associated_ty_by_trait_name: HashMap<(TraitDefRef, Name), TypeAliasRef>,
    associated_ty_values: HashMap<TypeAliasRef, Arc<AssociatedTyValue<RgChalkInterner>>>,
    associated_ty_value_by_impl: HashMap<(ImplRef, TypeAliasRef), TypeAliasRef>,
    opaque_tys: HashMap<OpaqueTyRef, Arc<OpaqueTyDatum<RgChalkInterner>>>,
    adts: HashMap<TypeDefRef, Arc<chalk_solve::rust_ir::AdtDatum<RgChalkInterner>>>,
    adt_variances: HashMap<TypeDefRef, Variances<RgChalkInterner>>,
    impls: HashMap<ImplRef, Arc<ImplDatum<RgChalkInterner>>>,
    impls_by_trait: HashMap<TraitDefRef, Vec<ImplRef>>,
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
        cache: &TraitSelectionCache,
        clauses: &[Clause],
        table: Option<&InferenceTable>,
    ) -> Result<(), I::Error>
    where
        D: DefMapSource<Error = I::Error>,
        I: ItemStoreSource<'query>,
    {
        let mut roots = ChalkProgramRoots::default();
        roots.collect_clauses(item_paths, clauses, table)?;
        self.ensure_roots(item_paths, crate_items, cache, &roots)
    }

    /// Make every definition reachable from this projection goal available to Chalk.
    pub(super) fn ensure_for_goal<'query, D, I>(
        &mut self,
        item_paths: &ItemPathQuery<'query, D, I>,
        crate_items: &CrateItemQuery<'query, D, I>,
        cache: &TraitSelectionCache,
        goal: &TraitGoal,
        table: &InferenceTable,
    ) -> Result<(), I::Error>
    where
        D: DefMapSource<Error = I::Error>,
        I: ItemStoreSource<'query>,
    {
        let mut roots = ChalkProgramRoots::default();
        roots.collect_goal(item_paths, goal, table)?;
        self.ensure_roots(item_paths, crate_items, cache, &roots)
    }

    fn ensure_roots<'query, D, I>(
        &mut self,
        item_paths: &ItemPathQuery<'query, D, I>,
        crate_items: &CrateItemQuery<'query, D, I>,
        cache: &TraitSelectionCache,
        roots: &ChalkProgramRoots,
    ) -> Result<(), I::Error>
    where
        D: DefMapSource<Error = I::Error>,
        I: ItemStoreSource<'query>,
    {
        let pending_roots = roots.new_since(&self.roots);
        if pending_roots.is_empty() {
            return Ok(());
        }

        crate::profile::metric::PROGRAM_BUILDS.inc();
        let started = Instant::now();
        let result = self
            .program
            .extend(item_paths, crate_items, cache, &pending_roots);
        crate::profile::metric::PROGRAM_BUILD_TIME.record(started.elapsed());
        result?;
        self.roots.merge(&pending_roots);

        // A trait is materialized with every visible impl before its first query, and an opaque is
        // materialized before its identity enters a goal. Later extensions therefore cannot
        // invalidate answers retained by the solver forests; they only add new goal identities.
        Ok(())
    }

    pub(super) fn database(&self) -> &ChalkProgram {
        &self.program
    }

    pub(super) fn associated_tys(
        &self,
    ) -> &HashMap<TypeAliasRef, Arc<AssociatedTyDatum<RgChalkInterner>>> {
        &self.program.associated_tys
    }

    pub(super) fn associated_ty_ref(
        &self,
        trait_ref: TraitDefRef,
        assoc_name: &str,
    ) -> Option<TypeAliasRef> {
        self.program
            .associated_ty_by_trait_name
            .get(&(trait_ref, Name::new(assoc_name)))
            .copied()
    }
}
