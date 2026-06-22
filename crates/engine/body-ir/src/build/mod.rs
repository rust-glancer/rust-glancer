//! Builds and rebuilds Body IR snapshots.

mod body_def_map;
mod body_item_store;
mod lower;
mod query_source;
mod resolve;
mod state;

use anyhow::Context as _;

use rg_def_map::PackageSlot;
use rg_ir_storage::PackageDefMaps as DefMapPackage;
use rg_package_store::{PackageLoader, PackageSubset};
use rg_semantic_ir::PackageIr;
use rg_std::Shrink;
use rg_text::PackageNameInterners;

use crate::{BodyIrBuildPolicy, BodyIrDb, BodyIrFile, PackageBodies};

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
        let mut packages = lower::build_packages(
            self.parse,
            &def_map_txn,
            &semantic_ir_txn,
            self.semantic_ir.package_count(),
            self.policy,
            self.interners.as_mut(),
        )?;
        resolve::resolve_packages(
            &mut packages,
            self.parse,
            self.interners.as_mut(),
            &def_map_txn,
            &semantic_ir_txn,
        )
        .context("while attempting to resolve body IR packages")?;
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
    policy: BodyIrBuildPolicy,
    selected_files: Option<Vec<BodyIrFile>>,
    packages: &'db [PackageSlot],
    interners: &'names mut PackageNameInterners,
    def_map_loader: PackageLoader<'db, DefMapPackage>,
    semantic_ir_loader: PackageLoader<'db, PackageIr>,
    subset: &'db PackageSubset,
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
            policy: BodyIrBuildPolicy::default(),
            selected_files: None,
            packages,
            interners,
            def_map_loader,
            semantic_ir_loader,
            subset,
        }
    }

    pub fn policy(mut self, policy: BodyIrBuildPolicy) -> Self {
        self.policy = policy;
        self
    }

    pub fn selected_files(mut self, files: Vec<BodyIrFile>) -> Self {
        self.selected_files = Some(files);
        self
    }

    pub fn build(self) -> anyhow::Result<BodyIrDb> {
        let mut next = self.old.clone();
        let packages = normalized_package_slots(self.packages);
        let lowering_scope = match &self.selected_files {
            Some(files) => lower::BodyIrLoweringScope::SelectedFiles(files),
            None => lower::BodyIrLoweringScope::PackagePolicy(self.policy),
        };
        let semantic_ir_txn = self
            .semantic_ir
            .read_txn_for_subset(self.semantic_ir_loader, self.subset);
        let def_map_txn = self
            .def_map
            .read_txn_for_subset(self.def_map_loader, self.subset);
        let mut rebuilt_packages = lower::build_selected_packages(
            self.parse,
            &def_map_txn,
            &semantic_ir_txn,
            lowering_scope,
            &packages,
            self.interners,
        )
        .context("while attempting to lower rebuilt body IR packages")?;
        resolve::resolve_selected_packages(
            &mut rebuilt_packages,
            self.parse,
            self.interners,
            &def_map_txn,
            &semantic_ir_txn,
        )
        .context("while attempting to resolve rebuilt body IR packages")?;
        let compacted_packages = compact_rebuilt_packages_two_phase(rebuilt_packages);

        {
            let mut mutator = next.mutator();
            for (package, rebuilt) in compacted_packages {
                mutator.replace_package(package, rebuilt).with_context(|| {
                    format!("while attempting to replace body IR package {}", package.0)
                })?;
            }
        }
        Ok(next)
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
