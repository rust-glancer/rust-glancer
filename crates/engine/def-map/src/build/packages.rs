//! Synchronous construction of selected packages against a frozen baseline.

use anyhow::Context as _;
use rg_ir_model::PackageSlot;
use rg_item_tree::ItemTreeDb;
use rg_macro_runtime::MacroExpansionPerformancePreference;
use rg_parse::ParseDb;
use rg_text::PackageNameInterners;
use rg_workspace::WorkspaceMetadata;

use super::{
    collect::collect_package_crates,
    crates::CrateBuildStore,
    implicit_roots::build_implicit_roots,
    scopes::{build_scopes, select_preludes},
};
use crate::{
    DefMapBuildOutput, DefMapDb, DefMapReadTxn, GeneratedItemStores, MacroSourceFileLoader,
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

        // Collect fresh declarations once. Resolution reads these builders for selected packages
        // and the frozen baseline for every other package.
        let implicit_roots = build_implicit_roots(workspace, parse.packages(), interners)
            .context("while attempting to build implicit crate roots")?;
        let mut crates = CrateBuildStore::new(parse.package_count());
        for package_slot in &packages {
            let parse_package = parse.package(package_slot.0).with_context(|| {
                format!(
                    "while attempting to fetch parsed package {}",
                    package_slot.0
                )
            })?;
            let item_tree_package = item_tree.package(package_slot.0).with_context(|| {
                format!(
                    "while attempting to fetch item-tree package {}",
                    package_slot.0
                )
            })?;
            let package_states = collect_package_crates(
                package_slot.0,
                parse_package,
                item_tree_package,
                implicit_roots.as_slice(),
            )
            .with_context(|| {
                format!(
                    "while attempting to collect crate declarations for package {}",
                    parse_package.package_name()
                )
            })?;
            crates
                .replace_package(*package_slot, package_states)
                .expect("selected package should have a crate-builder slot");
        }

        // Prelude selection gives lookup its implicit names. Scope construction then alternates
        // settled import waves with macro batches, loading and collecting real files as needed.
        select_preludes(
            baseline_read,
            workspace,
            parse.packages(),
            &mut crates,
            interners,
        )
        .context("while attempting to select crate preludes")?;
        build_scopes(
            baseline_read,
            parse,
            item_tree,
            &mut crates,
            interners,
            performance_preference,
            source_loader,
        )
        .context("while attempting to construct DefMap scopes")?;

        // Only settled data crosses into the next snapshot. Generated declarations travel beside
        // it until Semantic IR has copied their signatures.
        let mut next = self.clone();
        let mut generated_items = GeneratedItemStores::default();
        for package_slot in packages.iter().copied() {
            let parse_package = parse
                .package(package_slot.0)
                .expect("selected package should remain parsed after source loading");
            let package = crates.freeze_package(package_slot, parse_package, &mut generated_items);
            next.mutator()
                .replace_package(package_slot, package)
                .with_context(|| {
                    format!(
                        "while attempting to replace def-map package {}",
                        package_slot.0
                    )
                })?;
        }
        if reallocate_payloads {
            next.mutator().reallocate_packages(&packages);
        }
        Ok(DefMapBuildOutput::new(next, generated_items))
    }
}
