//! Builds and rebuilds Body IR snapshots.

mod body_def_map;
mod body_item_store;
mod lower;
mod materialization;
mod pattern_binding;
mod query_source;
mod resolve;
mod state;

use std::{collections::HashMap, time::Instant};

use anyhow::Context as _;
use rayon::prelude::*;

use rg_def_map::PackageDefMaps as DefMapPackage;
use rg_def_map::PackageSlot;
use rg_ir_model::{CrateId, CrateRef};
use rg_package_store::{PackageLoader, PackageSubset};
use rg_semantic_ir::PackageIr;
use rg_std::Shrink;
use rg_text::PackageNameInterners;

use crate::{BodyIrBuildPolicy, BodyIrDb, BodyIrFile, BodyIrLoader, PackageBodies};

use self::materialization::BodyIrMaterializationPlan;

/// Builder for a fresh Body IR snapshot.
pub struct BodyIrDbBuilder<'db, 'names> {
    parse: &'db rg_parse::ParseDb,
    def_map: &'db rg_def_map::DefMapDb,
    semantic_ir: &'db rg_semantic_ir::SemanticIrDb,
    policy: BodyIrBuildPolicy,
    interners: NameInternerSource<'names>,
}

impl<'db> BodyIrDbBuilder<'db, 'static> {
    pub(crate) fn new(
        parse: &'db rg_parse::ParseDb,
        def_map: &'db rg_def_map::DefMapDb,
        semantic_ir: &'db rg_semantic_ir::SemanticIrDb,
    ) -> Self {
        Self {
            parse,
            def_map,
            semantic_ir,
            policy: BodyIrBuildPolicy::default(),
            interners: NameInternerSource::Owned(PackageNameInterners::new(parse.package_count())),
        }
    }
}

impl<'db, 'names> BodyIrDbBuilder<'db, 'names> {
    pub fn name_interners(
        self,
        interners: &'names mut PackageNameInterners,
    ) -> BodyIrDbBuilder<'db, 'names> {
        BodyIrDbBuilder {
            parse: self.parse,
            def_map: self.def_map,
            semantic_ir: self.semantic_ir,
            policy: self.policy,
            interners: NameInternerSource::Borrowed(interners),
        }
    }

    pub fn policy(mut self, policy: BodyIrBuildPolicy) -> Self {
        self.policy = policy;
        self
    }

    pub fn build(mut self) -> anyhow::Result<BodyIrDb> {
        let def_map_txn = self
            .def_map
            .read_txn(PackageLoader::resident_only("resident body IR build"));
        let semantic_ir_txn = self
            .semantic_ir
            .read_txn(PackageLoader::resident_only("resident body IR build"));
        let packages = lower::build_packages(
            self.parse,
            &def_map_txn,
            &semantic_ir_txn,
            self.semantic_ir.package_count(),
            self.policy,
            self.interners.as_mut(),
        )?;
        let resolved = resolve::resolve_packages(
            packages,
            self.parse,
            self.interners.as_mut(),
            &def_map_txn,
            &semantic_ir_txn,
            false,
        )
        .context("while attempting to resolve body IR packages")?;
        debug_assert!(resolved.trait_selection_sessions.is_empty());
        let packages = resolved.packages;
        let packages = compact_packages_two_phase(packages);
        let mut db = BodyIrDb::from_packages(packages);
        {
            let mut mutator = db.mutator();
            mutator.compact_storage();
        }
        Ok(db)
    }
}

enum NameInternerSource<'names> {
    Owned(PackageNameInterners),
    Borrowed(&'names mut PackageNameInterners),
}

impl NameInternerSource<'_> {
    fn as_mut(&mut self) -> &mut PackageNameInterners {
        match self {
            Self::Owned(interners) => interners,
            Self::Borrowed(interners) => interners,
        }
    }
}

/// Builder for a Body IR snapshot that replaces selected packages.
pub struct BodyIrDbPackageRebuilder<'db, 'names> {
    old: &'db BodyIrDb,
    parse: &'db rg_parse::ParseDb,
    def_map: &'db rg_def_map::DefMapDb,
    semantic_ir: &'db rg_semantic_ir::SemanticIrDb,
    materialization: BodyIrMaterializationPlan,
    packages: &'db [PackageSlot],
    interners: &'names mut PackageNameInterners,
    def_map_loader: PackageLoader<'db, DefMapPackage>,
    semantic_ir_loader: PackageLoader<'db, PackageIr>,
    subset: &'db PackageSubset,
    saved_body_ir: Option<BodyIrLoader<'db>>,
}

impl<'db, 'names> BodyIrDbPackageRebuilder<'db, 'names> {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        old: &'db BodyIrDb,
        parse: &'db rg_parse::ParseDb,
        def_map: &'db rg_def_map::DefMapDb,
        semantic_ir: &'db rg_semantic_ir::SemanticIrDb,
        packages: &'db [PackageSlot],
        interners: &'names mut PackageNameInterners,
        def_map_loader: PackageLoader<'db, DefMapPackage>,
        semantic_ir_loader: PackageLoader<'db, PackageIr>,
        subset: &'db PackageSubset,
    ) -> Self {
        Self {
            old,
            parse,
            def_map,
            semantic_ir,
            materialization: BodyIrMaterializationPlan::ConfiguredBodies(
                BodyIrBuildPolicy::default(),
            ),
            packages,
            interners,
            def_map_loader,
            semantic_ir_loader,
            subset,
            saved_body_ir: None,
        }
    }

    /// Lowers body contents selected by the package policy.
    pub fn configured_bodies(mut self, policy: BodyIrBuildPolicy) -> Self {
        self.materialization = BodyIrMaterializationPlan::ConfiguredBodies(policy);
        self
    }

    /// Builds coverage records without lowering body contents selected by the policy.
    pub fn coverage_only(mut self, policy: BodyIrBuildPolicy) -> Self {
        self.materialization = BodyIrMaterializationPlan::CoverageOnly(policy);
        self
    }

    pub fn selected_files(mut self, files: Vec<BodyIrFile>) -> Self {
        self.materialization = BodyIrMaterializationPlan::SelectedFiles(files);
        self
    }

    /// Reuse saved item lookup indexes after the caller verifies their cache keys.
    ///
    /// Only the independently encoded indexes are read. Body shards remain lazy, and each missing
    /// or malformed index falls back to ordinary construction.
    pub fn reuse_item_lookup_indexes(mut self, saved_body_ir: BodyIrLoader<'db>) -> Self {
        self.saved_body_ir = Some(saved_body_ir);
        self
    }

    pub fn build(self) -> anyhow::Result<BodyIrDb> {
        let (body_ir, trait_selection_sessions) = self.build_inner(false)?;
        debug_assert!(trait_selection_sessions.is_empty());
        Ok(body_ir)
    }

    /// Rebuild Body IR and return the crate-semantic solver state warmed by that exact snapshot.
    ///
    /// The sessions do not borrow build inputs. A caller serving an immediate query can reuse them
    /// and then drop them with its other request-owned resources; ordinary builds should use
    /// [`Self::build`] so solver state is not retained accidentally.
    pub fn build_with_trait_selection_sessions(
        self,
    ) -> anyhow::Result<(BodyIrDb, Vec<rg_ty::TraitSelectionSession>)> {
        self.build_inner(true)
    }

    /// Rebuild selected package slots without changing the old snapshot.
    ///
    /// The work stays private until lowering, resolution, and compaction have all succeeded. A
    /// dirty rebuild may also bring verified lookup indexes from the saved snapshot; each missing
    /// index simply takes the ordinary path that builds a fresh index during crate resolution.
    fn build_inner(
        self,
        retain_trait_selection: bool,
    ) -> anyhow::Result<(BodyIrDb, Vec<rg_ty::TraitSelectionSession>)> {
        // 1. Start with the old snapshot so untouched package slots remain shared. The read
        // transactions may load dependencies from the bounded subset while selected packages are
        // rebuilt in memory; the saved Body IR transaction exists only when reuse was enabled.
        let rebuild_started = Instant::now();
        let clone_started = Instant::now();
        let mut next = self.old.clone();
        let clone_ms = clone_started.elapsed().as_millis();
        let setup_started = Instant::now();
        let packages = normalized_package_slots(self.packages);
        let materialization = self.materialization.lowering();
        let saved_body_ir = self
            .saved_body_ir
            .map(|loader| self.old.read_txn_for_subset(loader, self.subset));
        let semantic_ir_txn = self
            .semantic_ir
            .read_txn_for_subset(self.semantic_ir_loader, self.subset);
        let def_map_txn = self
            .def_map
            .read_txn_for_subset(self.def_map_loader, self.subset);
        let setup_ms = setup_started.elapsed().as_millis();

        // 2. Lower only the requested bodies. Crates whose resulting coverage is unmaterialized
        // remain in the package shape, but skip lookup-index construction and body resolution.
        let lowering_started = Instant::now();
        let rebuilt_packages = lower::build_selected_packages(
            self.parse,
            &def_map_txn,
            &semantic_ir_txn,
            materialization,
            &packages,
            self.interners,
        )
        .context("while attempting to lower rebuilt body IR packages")?;
        let lowering_ms = lowering_started.elapsed().as_millis();

        // 3. Load verified saved indexes for materialized crates. Reuse avoids rebuilding the
        // visibility-scoped index, but body resolution still needs the declaration packages that
        // the saved index points into, so those packages are warmed before parallel resolution.
        let lookup_index_preload_started = Instant::now();
        let mut saved_item_lookup_indexes = HashMap::new();
        if let Some(saved_body_ir) = &saved_body_ir {
            for (package, crate_bodies) in &rebuilt_packages {
                for (crate_idx, crate_bodies) in crate_bodies.iter().enumerate() {
                    if !crate_bodies.coverage().is_materialized() {
                        continue;
                    }
                    let crate_ref = CrateRef {
                        package: *package,
                        crate_id: CrateId(crate_idx),
                    };
                    // Saved indexes are an optional shortcut. An unavailable cache entry should
                    // fall back to constructing this crate's index from rebuilt declarations.
                    match saved_body_ir.item_lookup_index(crate_ref) {
                        Ok(Some(index)) => {
                            saved_item_lookup_indexes.insert(crate_ref, index.clone());
                        }
                        Ok(None) => {}
                        Err(error) => {
                            tracing::debug!(
                                package = crate_ref.package.0,
                                crate_id = crate_ref.crate_id.0,
                                error = %error,
                                "saved item lookup index unavailable for reuse"
                            );
                        }
                    }
                }
            }

            // Reusing the old index removes the sequential visible-store scan that ordinarily
            // warms declaration packages for body resolution. Each saved index records which
            // packages supplied those stores; load their union in parallel so the solver does not
            // decode the same artifacts serially or warm unrelated subset packages.
            let mut packages = Vec::new();
            for index in saved_item_lookup_indexes.values() {
                packages.extend_from_slice(index.visible_packages());
            }
            packages.sort_by_key(|package| package.0);
            packages.dedup();

            if !packages.is_empty() {
                let profile_context = rg_profile::ProfileThreadContext::capture();
                let thread_pool = local_thread_pool("rg-body-reuse-load")
                    .context("while attempting to create item lookup index loader pool")?;
                thread_pool
                    .install(|| {
                        packages.into_par_iter().try_for_each(|package| {
                            let _profile_guard = profile_context.enter();
                            def_map_txn.package(package)?;
                            semantic_ir_txn.package(package)?;
                            Ok::<_, rg_package_store::PackageStoreError>(())
                        })
                    })
                    .context("while attempting to preload item lookup index declarations")?;
            }
        }
        let saved_item_lookup_index_count = saved_item_lookup_indexes.len();
        let lookup_index_preload_ms = lookup_index_preload_started.elapsed().as_millis();

        // 4. Hand each saved index to its matching crate, resolve the lowered bodies, and compact
        // the rebuilt packages while their temporary allocation set is still grouped together.
        let resolution_started = Instant::now();
        let resolved = resolve::resolve_selected_packages(
            rebuilt_packages,
            self.parse,
            self.interners,
            &def_map_txn,
            &semantic_ir_txn,
            retain_trait_selection,
            saved_item_lookup_indexes,
        )
        .context("while attempting to resolve rebuilt body IR packages")?;
        let rebuilt_packages = resolved.packages;
        let resolution_ms = resolution_started.elapsed().as_millis();
        let compaction_started = Instant::now();
        let compacted_packages = compact_rebuilt_packages_two_phase(rebuilt_packages);
        let compaction_ms = compaction_started.elapsed().as_millis();

        // 5. Replace package slots only after every fallible build phase has succeeded. Close the
        // read views before returning; the caller may still retain their loaders as request cache,
        // but it does not need to retain these Body IR build transactions.
        let replacement_started = Instant::now();
        {
            let mut mutator = next.mutator();
            for (package, rebuilt) in compacted_packages {
                mutator.replace_package(package, rebuilt).with_context(|| {
                    format!("while attempting to replace body IR package {}", package.0)
                })?;
            }
        }
        let replacement_ms = replacement_started.elapsed().as_millis();
        let read_txn_drop_started = Instant::now();
        drop(semantic_ir_txn);
        drop(def_map_txn);
        drop(saved_body_ir);
        let read_txn_drop_ms = read_txn_drop_started.elapsed().as_millis();

        tracing::trace!(
            ?materialization,
            package_count = packages.len(),
            trait_selection_session_count = resolved.trait_selection_sessions.len(),
            clone_ms,
            setup_ms,
            lowering_ms,
            saved_item_lookup_index_count,
            lookup_index_preload_ms,
            resolution_ms,
            compaction_ms,
            replacement_ms,
            read_txn_drop_ms,
            total_ms = rebuild_started.elapsed().as_millis(),
            "Body IR package rebuild phases finished"
        );
        Ok((next, resolved.trait_selection_sessions))
    }
}

fn compact_packages_two_phase(packages: Vec<PackageBodies>) -> Vec<PackageBodies> {
    // In-place shrinking reallocates and frees nested body vectors one at a time. Large builds can
    // then leave the few final allocations scattered across allocator slabs that used to hold
    // transient capacity. Compact copies are built while the source allocation set is still dense,
    // then the source packages are dropped together so mostly-empty slabs can be reclaimed.
    let compacted = packages
        .iter()
        .map(compact_package_copy)
        .collect::<Vec<_>>();
    drop(packages);
    compacted
}

fn compact_rebuilt_packages_two_phase(
    rebuilt_packages: Vec<(PackageSlot, PackageBodies)>,
) -> Vec<(PackageSlot, PackageBodies)> {
    let compacted = rebuilt_packages
        .iter()
        .map(|(package, rebuilt)| (*package, compact_package_copy(rebuilt)))
        .collect::<Vec<_>>();
    drop(rebuilt_packages);
    compacted
}

fn compact_package_copy(package: &PackageBodies) -> PackageBodies {
    let mut compacted = package.clone();
    Shrink::shrink_to_fit(&mut compacted);
    compacted
}

fn local_thread_pool(thread_name_prefix: &'static str) -> anyhow::Result<rayon::ThreadPool> {
    rayon::ThreadPoolBuilder::new()
        .thread_name(move |index| format!("{thread_name_prefix}-{index}"))
        .build()
        .with_context(|| format!("while attempting to create {thread_name_prefix} thread pool"))
}

fn normalized_package_slots(packages: &[PackageSlot]) -> Vec<PackageSlot> {
    let mut slots = packages.to_vec();
    slots.sort_by_key(|slot| slot.0);
    slots.dedup();
    slots
}
