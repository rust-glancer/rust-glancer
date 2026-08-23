//! Resident item-tree database and package-selection builders.

use anyhow::Context as _;
use rayon::prelude::*;

use rg_parse::ParseDb;
use rg_std::{MemorySize, Shrink};
use rg_text::PackageNameInterners;

use crate::{Package, lower};

/// File work performed while adding one captured source to an existing package tree.
///
/// The requested file may lead to ordinary descendants such as `mod nested;`, so both counts can
/// cover more than the file passed to `ItemTreeDb::lower_package_file`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IncrementalItemTreeLowering {
    /// Files that received a new `FileTree` during this call.
    pub newly_lowered_files: usize,
    /// Files encountered through the module graph whose `FileTree` already existed.
    pub reused_files: usize,
}

/// Lowered item trees for all parsed packages.
#[derive(Debug, Clone, PartialEq, Eq, Default, MemorySize, Shrink)]
pub struct ItemTreeDb {
    pub(crate) packages: Vec<Option<Package>>,
}

impl ItemTreeDb {
    /// Builds item trees only for selected packages using caller-retained interners.
    ///
    /// Project rebuilds use this as a temporary lowering input: affected packages are populated,
    /// while unrelated packages stay absent so accidental cross-package item-tree access fails
    /// loudly instead of retaining the whole item-tree graph.
    pub fn build_packages(
        parse: &mut ParseDb,
        packages: &[usize],
        interners: &mut PackageNameInterners,
    ) -> anyhow::Result<Self> {
        let package_slots = normalized_package_slots(parse.package_count(), packages)?;
        anyhow::ensure!(
            interners.package_count() == parse.package_count(),
            "name interner count {} does not match parse package count {}",
            interners.package_count(),
            parse.package_count(),
        );

        let mut trees = Self {
            packages: vec![None; parse.package_count()],
        };

        let sources = parse.source_inventory_handle();
        Self::build_packages_parallel(parse, &sources, &package_slots, interners, &mut trees)?;
        Shrink::shrink_to_fit(&mut trees);

        Ok(trees)
    }

    /// Returns one package tree set by slot.
    pub fn package(&self, package_slot: usize) -> Option<&Package> {
        self.packages.get(package_slot)?.as_ref()
    }

    /// Adds one captured package file through the ordinary package lowerer.
    ///
    /// The file may recursively add ordinary out-of-line descendants. Existing file trees and
    /// target roots retain their package-local ids and are never replaced. Syntax loaded for this
    /// incremental pass is evicted before the method returns.
    pub fn lower_package_file(
        &mut self,
        parse: &mut ParseDb,
        package_slot: usize,
        file_id: rg_parse::FileId,
        module_file_context: rg_parse::ModuleFileContext,
        interners: &mut PackageNameInterners,
    ) -> anyhow::Result<IncrementalItemTreeLowering> {
        let sources = parse.source_inventory_handle();
        let parse_package = parse
            .package_mut(package_slot)
            .with_context(|| format!("while attempting to fetch parsed package {package_slot}"))?;
        let item_tree_package = self
            .packages
            .get_mut(package_slot)
            .and_then(Option::as_mut)
            .with_context(|| {
                format!("while attempting to fetch item-tree package {package_slot}")
            })?;
        let interner = interners.package_mut(package_slot).with_context(|| {
            format!("while attempting to fetch name interner for package {package_slot}")
        })?;

        let stats = lower::extend_package(
            parse_package,
            &sources,
            interner,
            item_tree_package,
            file_id,
            module_file_context,
        )
        .with_context(|| {
            format!(
                "while attempting to incrementally lower file {file_id:?} for package {package_slot}"
            )
        })?;
        parse_package.evict_syntax_trees();

        Ok(IncrementalItemTreeLowering {
            newly_lowered_files: stats.lowered_files,
            reused_files: stats.reused_files,
        })
    }

    fn build_packages_parallel(
        parse: &mut ParseDb,
        sources: &rg_source::SourceInventory,
        package_slots: &[usize],
        interners: &mut PackageNameInterners,
        trees: &mut Self,
    ) -> anyhow::Result<()> {
        let mut selected = vec![false; parse.package_count()];
        for &package_slot in package_slots {
            selected[package_slot] = true;
        }

        let thread_pool = rayon::ThreadPoolBuilder::new()
            .thread_name(|index| format!("rg-item-tree-{index}"))
            .build()
            .context("while attempting to create item-tree lowering thread pool")?;

        // Each package owns its parse cache, name interner, and output slot. Zipping mutable
        // slices makes that independence visible to Rayon, while the selection bitmap preserves
        // the sparse rebuild behavior where unrelated package slots stay absent.
        thread_pool.install(|| {
            parse
                .packages_mut()
                .par_iter_mut()
                .zip(interners.packages_mut().par_iter_mut())
                .zip(trees.packages.par_iter_mut())
                .enumerate()
                .try_for_each(
                    |(package_slot, ((parse_package, interner), output))| -> anyhow::Result<()> {
                        if !selected[package_slot] {
                            return Ok(());
                        }

                        *output = Some(Self::lower_package(
                            package_slot,
                            parse_package,
                            sources,
                            interner,
                        )?);
                        Ok(())
                    },
                )
        })
    }

    fn lower_package(
        package_slot: usize,
        package: &mut rg_parse::Package,
        sources: &rg_source::SourceInventory,
        interner: &mut rg_text::NameInterner,
    ) -> anyhow::Result<Package> {
        let package_name = package.package_name().to_owned();
        // Parse syntax is only needed while this package is being lowered. Drop it at the package
        // boundary so large workspaces do not keep every syntax tree alive until the whole
        // item-tree phase finishes.
        package.discover_modules(sources).with_context(|| {
            format!("while attempting to discover modules for package {package_name}")
        })?;
        let item_tree = lower::build_package(package, sources, interner)
            .with_context(|| {
                format!("while attempting to build item trees for package {package_name}")
            })
            .with_context(|| {
                format!("while attempting to build item tree package {package_slot}")
            })?;
        package.evict_syntax_trees();
        Ok(item_tree)
    }
}

fn normalized_package_slots(
    package_count: usize,
    packages: &[usize],
) -> anyhow::Result<Vec<usize>> {
    let mut packages = packages.to_vec();
    packages.sort_unstable();
    packages.dedup();

    if let Some(package_slot) = packages.iter().copied().find(|slot| *slot >= package_count) {
        anyhow::bail!(
            "package slot {package_slot} is out of bounds for {package_count} parsed packages"
        );
    }

    Ok(packages)
}
