//! Builds frozen def-map snapshots by replacing selected packages in a shaped baseline.
//!
//! Collection records direct declarations and raw imports before names from other modules and
//! crates have been resolved. A dependency order lets construction finish one crate at a time.
//! The active crate has a mutable builder; every external crate is frozen, either completed in
//! this build or read lazily from the unchanged baseline.
//!
//! Macro expansion can reveal real files after the initial Parse/ItemTree pass. For example,
//! expanded syntax may contain `mod generated;` or an
//! `include!(concat!(env!("OUT_DIR"), "/bindings.rs"))`. DefMap knows how those constructs affect
//! modules and scopes, but source capture belongs to the project layer.
//! `DefMapDb::build_packages` calls an injected source loader between expansion batches until
//! the active crate can be frozen. It then proceeds to the next crate.

use std::sync::Arc;

use anyhow::Context as _;
use rg_ir_model::PackageSlot;
use rg_item_tree::ItemTreeDb;
use rg_macro_runtime::{MacroExpansionPerformancePreference, MacroExpansionRuntime};
use rg_parse::ParseDb;
use rg_text::PackageNameInterners;
use rg_workspace::WorkspaceMetadata;

use self::{
    collect::collect_crate,
    crates::KnownModuleFiles,
    output::CompletedCrateStore,
    plan::CrateBuildPlan,
    resolution_env::BuildResolutionEnv,
    resolve::{resolve_crate, select_prelude},
};
use crate::{DefMapDb, DefMapReadTxn};

mod collect;
mod crates;
mod imports;
mod macro_source_files;
mod macros;
mod output;
mod plan;
mod resolution_env;
mod resolve;

pub(crate) use self::macro_source_files::MacroSourceFileResolutions;
pub use self::{
    imports::{
        ImportCollectionEnv, ImportCollectionOutput, ImportCollector, ModuleImportInput,
        ResolvedModuleImports,
    },
    macro_source_files::{
        MacroSourceFileLoader, MacroSourceFileRequest, MacroSourceFileResolution,
    },
    output::{DefMapBuildOutput, GeneratedItemStores},
};

impl DefMapDb {
    /// Replaces selected packages, loading files discovered by expansion before returning.
    ///
    /// The loader may grow Parse and ItemTree between expansion batches. All semantic work stays
    /// inside this call; the output carries frozen packages and generated declarations for Semantic IR.
    /// Reallocate completed payloads only when the caller will retain them decoded; packages headed
    /// straight to disk do not need the extra copy.
    #[allow(clippy::too_many_arguments)]
    pub fn build_packages(
        &self,
        baseline_read: &DefMapReadTxn<'_>,
        workspace: &WorkspaceMetadata,
        parse: &mut ParseDb,
        item_tree: &mut ItemTreeDb,
        packages: &[PackageSlot],
        reallocate_payloads: bool,
        interners: &mut PackageNameInterners,
        performance_preference: MacroExpansionPerformancePreference,
        source_loader: &mut impl MacroSourceFileLoader,
    ) -> anyhow::Result<DefMapBuildOutput> {
        let mut packages = packages.to_vec();
        packages.sort_by_key(|slot| slot.0);
        packages.dedup();

        // Resolve each dependency completely before collecting its users. This order comes from
        // the same target-specific roots used by name lookup, including sibling libraries and
        // compiler-provided crates. Original slots stay unchanged in the completed output.
        let plan = CrateBuildPlan::prepare(workspace, parse.packages(), &packages, interners)
            .context("prepare crate dependencies")?;
        let mut completed = CompletedCrateStore::new(parse.packages(), &packages);

        // Reuse compilation/expansion caches, the worker pool, and file answers across crates.
        // Only import scheduling and pending macro directives are local to the active crate.
        let mut runtime = MacroExpansionRuntime::new(performance_preference);
        let mut source_resolutions = MacroSourceFileResolutions::default();
        let mut remaining_report_groups = 64;
        let mut known_module_files = vec![None; parse.package_count()];
        for input in plan.crates {
            let krate = input.crate_ref;
            let package = parse
                .package(krate.package.0)
                .context("read selected parsed package")?;
            let item_tree_package = item_tree
                .package(krate.package.0)
                .context("read selected item-tree package")?;
            let files = known_module_files[krate.package.0].get_or_insert_with(|| {
                Arc::new(KnownModuleFiles::from_package(package, item_tree_package))
            });
            let mut state = collect_crate(
                krate,
                package,
                item_tree_package,
                input.implicit_roots,
                Arc::clone(files),
            )
            .with_context(|| {
                format!("collect crate declarations for {}", package.package_name())
            })?;
            state.censor_proc_macro_exports();
            let metadata = workspace
                .package(package.id())
                .context("read selected workspace package")?;
            state.prelude = select_prelude(
                &BuildResolutionEnv::new(baseline_read, &completed, &state),
                &state,
                &metadata.origin,
                interners
                    .package_mut(krate.package.0)
                    .context("read selected name interner")?,
            )?;
            resolve_crate(
                baseline_read,
                &completed,
                parse,
                item_tree,
                &mut state,
                interners,
                &mut runtime,
                &mut source_resolutions,
                &mut remaining_report_groups,
                source_loader,
            )
            .with_context(|| format!("resolve crate {}", state.crate_name))?;
            // Late source loading updates the active file map. Sibling targets can reuse that
            // updated map without rebuilding a package-sized path index for every target.
            known_module_files[krate.package.0] = Some(Arc::clone(&state.known_module_files));
            let output = state.finish(
                parse
                    .package(krate.package.0)
                    .expect("completed crate remains parsed")
                    .package_name(),
            );
            completed.insert(output);
        }
        // Construction caches are no longer needed. Release them before final payload packing
        // and optional reallocation, which may briefly keep both old and compacted buffers alive.
        drop(runtime);
        drop(source_resolutions);
        drop(known_module_files);

        // Packaging is a storage boundary, independent of the order in which targets completed.
        // Move frozen crates into their original slots; generated syntax travels separately until
        // Semantic IR has copied its declarations.
        let mut next = self.clone();
        for package_slot in packages.iter().copied() {
            let package = completed.take_package(
                package_slot,
                parse
                    .package(package_slot.0)
                    .expect("selected package remains parsed"),
            );
            next.mutator()
                .replace_package(package_slot, package)
                .context("replace selected DefMap package")?;
        }
        if reallocate_payloads {
            next.mutator().reallocate_packages(&packages);
        }
        Ok(completed.into_output(next))
    }
}
