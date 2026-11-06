//! Builds analysis data for the code inside Rust bodies.
//!
//! [`BodyIrBuilder`] analyzes saved source using declarations already collected by DefMap and
//! Semantic IR. [`CurrentSourceBuilder`] handles editor text for one request, preparing the
//! declarations and bodies needed by that request against the saved project.

mod const_eval;
mod current;
mod local_items;
mod lower;
mod materialization;
mod pattern_binding;
mod policy;
mod query_source;
mod resolve;
mod state;

use anyhow::Context as _;
use rg_def_map::{DefMapDb, DefMapLoader};
use rg_ir_model::{CrateRef, PackageSlot};
use rg_package_store::PackageSubset;
use rg_parse::ParseDb;
use rg_semantic_ir::{SemanticIrDb, SemanticIrLoader};
use rg_std::{Shrink, UniqueVec};
use rg_text::PackageNameInterners;

use self::materialization::BodyIrMaterializationPlan;
pub use self::{
    current::{
        CurrentSourceBuildCheckpoint, CurrentSourceBuildSummary, CurrentSourceBuilder,
        CurrentSourceSelection, CurrentSourceUnavailable,
    },
    policy::BodyIrBuildPolicy,
};
use crate::CrateBodies;

/// Builds [`CrateBodies`] from saved source and the declarations in
/// [`DefMapDb`](rg_def_map::DefMapDb) and [`SemanticIrDb`](rg_semantic_ir::SemanticIrDb).
///
/// Select bodies by build policy or by explicit crates, then collect the results with
/// [`Self::build`].
/// Installing these results in a [`BodyIrDb`](crate::BodyIrDb) is the caller's responsibility,
/// so body analysis can run independently of changes to the project's existing bodies.
///
/// `evaluate_initializers` returns saved declaration values even for targets excluded by body policy. The
/// caller installs these in Semantic IR separately from the body products.
pub struct BodyIrBuilder<'db, 'names> {
    parse: &'db ParseDb,
    def_map: &'db DefMapDb,
    semantic_ir: &'db SemanticIrDb,
    materialization: Option<BodyIrMaterializationPlan>,
    packages: &'db [PackageSlot],
    interners: &'names mut PackageNameInterners,
    def_map_loader: DefMapLoader<'db>,
    semantic_ir_loader: SemanticIrLoader<'db>,
    subset: &'db PackageSubset,
    cancellation: rg_std::CancellationToken,
    reallocate_payloads: bool,
}

impl rg_std::Cancelable for BodyIrBuilder<'_, '_> {
    fn check_cancelled(&self, checkpoint: &'static str) -> Result<(), rg_std::Cancelled> {
        rg_std::Cancelable::check_cancelled(&self.cancellation, checkpoint)
    }
}

impl<'db, 'names> BodyIrBuilder<'db, 'names> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        parse: &'db ParseDb,
        def_map: &'db DefMapDb,
        semantic_ir: &'db SemanticIrDb,
        packages: &'db [PackageSlot],
        reallocate_payloads: bool,
        interners: &'names mut PackageNameInterners,
        def_map_loader: DefMapLoader<'db>,
        semantic_ir_loader: SemanticIrLoader<'db>,
        subset: &'db PackageSubset,
    ) -> Self {
        Self {
            parse,
            def_map,
            semantic_ir,
            materialization: None,
            packages,
            reallocate_payloads,
            interners,
            def_map_loader,
            semantic_ir_loader,
            subset,
            cancellation: rg_std::CancellationToken::new(),
        }
    }

    /// Select the bodies that [`BodyIrBuildPolicy`] allows for each package and Cargo target.
    pub fn configured_bodies(mut self, policy: BodyIrBuildPolicy) -> Self {
        self.materialization = Some(BodyIrMaterializationPlan::ConfiguredBodies(policy));
        self
    }

    /// Complete selected targets, including targets omitted by the startup policy.
    pub fn selected_crates(mut self, crates: UniqueVec<CrateRef>) -> Self {
        self.materialization = Some(BodyIrMaterializationPlan::SelectedCrates(crates));
        self
    }

    /// Bind this build and its package workers to the operation that requested it.
    pub fn cancellation(mut self, cancellation: rg_std::CancellationToken) -> Self {
        self.cancellation = cancellation;
        self
    }

    /// Analyze the selected bodies before returning any results to the project.
    #[rg_std::cancelable("start body build")]
    pub fn build(self) -> anyhow::Result<Vec<(CrateRef, CrateBodies)>> {
        let packages = Self::normalized_package_slots(self.packages);
        let materialization = self
            .materialization
            .as_ref()
            .context("body build requires a materialization selection")?
            .lowering();
        // Dependency loads and solver caches belong to this build. Only finished body data
        // escapes into the saved project after every worker has completed successfully.
        let semantic_ir = self
            .semantic_ir
            .read_txn_for_subset(self.semantic_ir_loader, self.subset);
        let def_map = self
            .def_map
            .read_txn_for_subset(self.def_map_loader, self.subset);
        let lowered = lower::build_selected_packages(
            self.parse,
            &def_map,
            &semantic_ir,
            materialization,
            &packages,
            self.interners,
            &self.cancellation,
        )?;
        let mut products = resolve::resolve_selected_packages(
            lowered,
            self.parse,
            self.interners,
            &def_map,
            &semantic_ir,
            &self.cancellation,
        )?;
        // Resident payloads are copied out of allocator pages shared with build temporaries.
        // Offloadable payloads will be encoded and dropped, so they do not need that copy.
        if self.reallocate_payloads {
            for (_, bodies) in &mut products {
                let mut reallocated = bodies.clone();
                Shrink::shrink_to_fit(&mut reallocated);
                *bodies = reallocated;
            }
        }
        rg_std::check_cancel!(self.cancellation, "finish body build");
        products.sort_by_key(|(crate_ref, _)| (crate_ref.package.0, crate_ref.crate_id.0));
        Ok(products)
    }

    fn normalized_package_slots(packages: &[PackageSlot]) -> Vec<PackageSlot> {
        let mut slots = packages.to_vec();
        slots.sort_by_key(|slot| slot.0);
        slots.dedup();
        slots
    }
}

fn local_thread_pool(thread_name_prefix: &'static str) -> anyhow::Result<rayon::ThreadPool> {
    rayon::ThreadPoolBuilder::new()
        .thread_name(move |index| format!("{thread_name_prefix}-{index}"))
        .build()
        .with_context(|| format!("while attempting to create {thread_name_prefix} thread pool"))
}
