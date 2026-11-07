//! Temporary package view of file queries for the legacy DefMap and Semantic IR builders.
//!
//! Phase 08 removes this bridge when those consumers read Ajika directly.

use std::{collections::BTreeSet, sync::Arc};

use anyhow::Context as _;
use rayon::prelude::*;
use rg_arena::Arena;
use rg_ir_model::{FileId, PackageSlot};
use rg_parse::{ModuleFileContext, ParseDb};
use rg_source::SourceInput;
use rg_std::MemorySize;

use crate::{Package, TargetRoot, discovery::SourceDiscovery, file_declarations};

/// File work performed while adding a source discovery wave to the package view.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IncrementalItemTreeLowering {
    pub newly_lowered_files: usize,
    pub reused_files: usize,
}

/// Lowered item trees for selected parsed packages.
#[derive(Debug, Clone, PartialEq, Eq, Default, MemorySize)]
pub struct ItemTreeDb {
    packages: Vec<Option<Package>>,
}

impl ItemTreeDb {
    /// Discovers the selected package files, then lowers the captured batch in parallel.
    pub fn build_packages(
        database: &mut ajika::Database,
        parse: &mut ParseDb,
        packages: &[usize],
    ) -> anyhow::Result<Self> {
        let mut selected = vec![false; parse.package_count()];
        for &slot in packages {
            *selected
                .get_mut(slot)
                .with_context(|| format!("unknown package slot {slot}"))? = true;
        }
        let sources = parse.source_inventory_handle();

        // Discovery is still a host walk. It must capture module edges even when Ajika can
        // reuse every declaration, and uses the logical directory at each visit.
        let discovered = parse
            .packages_mut()
            .par_iter_mut()
            .enumerate()
            .map(|(slot, package)| {
                if !selected[slot] {
                    return Ok(None);
                }
                let roots = Arena::from_vec(
                    package
                        .targets()
                        .iter()
                        .map(|target| TargetRoot {
                            target: target.id,
                            root_file: target.root_file,
                        })
                        .collect(),
                );
                let mut discovery = SourceDiscovery::new(PackageSlot(slot), package, &sources);
                for root in roots.iter() {
                    discovery.visit_root(root.root_file)?;
                }
                let files = discovery.files;
                let includes = discovery.includes;
                package.evict_syntax_trees();
                Ok(Some((
                    Package {
                        target_roots: roots,
                        includes,
                        ..Package::default()
                    },
                    files,
                )))
            })
            .collect::<anyhow::Result<Vec<_>>>()?;

        database.update_inputs(|update| sources.write_inputs(update))?;
        let files = discovered
            .iter()
            .enumerate()
            .flat_map(|(slot, package)| {
                package.iter().flat_map(move |(_, files)| {
                    files.iter().map(move |&file| (PackageSlot(slot), file))
                })
            })
            .collect::<Vec<_>>();
        let mut trees = Self {
            packages: discovered
                .into_iter()
                .map(|package| package.map(|(package, _)| package))
                .collect(),
        };
        trees.lower_files(database, &files)?;
        Ok(trees)
    }

    pub fn package(&self, package_slot: usize) -> Option<&Package> {
        self.packages.get(package_slot)?.as_ref()
    }

    /// Adds one discovery wave from DefMap to the package view.
    pub fn lower_package_files(
        &mut self,
        database: &mut ajika::Database,
        parse: &mut ParseDb,
        requests: &[(PackageSlot, FileId, Arc<ModuleFileContext>)],
    ) -> anyhow::Result<IncrementalItemTreeLowering> {
        let sources = parse.source_inventory_handle();
        let mut touched = BTreeSet::new();
        let mut new_files = BTreeSet::new();
        let mut reused_files = 0;
        for (slot, file, context) in requests {
            let parse_package = parse
                .package_mut(slot.0)
                .context("unknown parsed package")?;
            let package = self
                .packages
                .get_mut(slot.0)
                .and_then(Option::as_mut)
                .context("unknown item-tree package")?;
            let mut discovery = SourceDiscovery::new(*slot, parse_package, &sources);
            // Visits retain their logical directories, even if this file already has a tree.
            discovery
                .visit_file(*file, context)
                .with_context(|| format!("discover source edges for {file:?} in {slot:?}"))?;
            for file in discovery.files {
                if package.files.contains_key(&file) || !new_files.insert((*slot, file)) {
                    reused_files += 1;
                }
            }
            for (origin, includes) in discovery.includes {
                package.includes.entry(origin).or_default().extend(includes);
            }
            touched.insert(*slot);
        }
        for slot in touched {
            parse
                .package_mut(slot.0)
                .expect("discovered package")
                .evict_syntax_trees();
        }

        if !new_files.is_empty() {
            // Publish all new entries before running any query so a field table is copied at
            // most once for this wave, even when several requests reach the same source.
            database.update_inputs(|update| {
                for &(slot, file) in &new_files {
                    let path = parse
                        .package(slot.0)
                        .and_then(|package| package.file_path(file))
                        .expect("discovered file has a path");
                    let entry = sources.entry(path).expect("discovered file was captured");
                    SourceInput::set_captured(update, entry)?;
                }
                Ok(())
            })?;
            let files = new_files.iter().copied().collect::<Vec<_>>();
            self.lower_files(database, &files)?;
        }

        Ok(IncrementalItemTreeLowering {
            newly_lowered_files: new_files.len(),
            reused_files,
        })
    }

    fn lower_files(
        &mut self,
        database: &mut ajika::Database,
        files: &[(PackageSlot, FileId)],
    ) -> ajika::Result<()> {
        if files.is_empty() {
            return Ok(());
        }
        let lowered = database.run(|db| {
            db.parallel_map(files, |db, &(package, file)| {
                file_declarations(db, package, file)
            })
        })?;
        for (&(slot, file), tree) in files.iter().zip(lowered) {
            self.packages[slot.0]
                .as_mut()
                .expect("lowering selected package")
                .files
                .insert(file, tree);
        }
        Ok(())
    }
}
