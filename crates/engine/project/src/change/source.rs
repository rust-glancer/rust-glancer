//! Applies ordinary source-file saves without invalidating the workspace graph.
//!
//! This path keeps package and Cargo-target slots stable. It reparses the saved file, rebuilds
//! affected packages and their reverse dependents, and reports changed crates from the updated def-map
//! snapshot.

use std::{collections::HashSet, path::PathBuf};

use anyhow::Context as _;
use rg_ir_model::{CrateRef, PackageSlot};
use rg_parse::SavedFileRefresh;
use rg_std::UniqueVec;

use super::affected_packages;
use crate::{
    AnalysisChangeSummary, ChangedFile, ProjectMemoryPurgePoint, SavedFileChange, indexing,
    selection::subset, state::ProjectState, storage::residency::ResidencyApplication,
};

pub(super) fn apply_source_changes(
    project: &mut ProjectState,
    database: &mut ajika::Database,
    changes: Vec<SavedFileChange>,
) -> anyhow::Result<AnalysisChangeSummary> {
    let mut changed_files = Vec::new();
    let mut changed_files_seen = HashSet::new();
    let mut fallback_package_roots = UniqueVec::new();
    let mut fallback_saved_paths = HashSet::new();

    // Reparse every known file first, then rebuild the union of affected packages once. This keeps
    // large watcher batches proportional to the changed package set instead of to the number of
    // changed paths in the batch.
    for change in changes {
        let refresh = match change.captured_source() {
            Some(captured) => project.parse_db_mut().refresh_captured_saved_file(captured),
            None => project
                .parse_db_mut()
                .refresh_saved_file_from_disk(change.path()),
        };
        let refresh = match refresh {
            Ok(refresh) => refresh,
            Err(error) if error.io_kind() == Some(std::io::ErrorKind::NotFound) => {
                // The path disappeared after canonicalization, usually because a checkout or
                // rename advanced again while this command waited. Other paths remain useful.
                continue;
            }
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "while attempting to capture saved file change for {}",
                        change.path().display()
                    )
                });
            }
        };

        let changed = match refresh {
            SavedFileRefresh::Unchanged => continue,
            SavedFileRefresh::Reparsed(changed) => changed,
            SavedFileRefresh::Unknown => {
                // A saved file can be new to the graph even though it now exists on disk. In that
                // case, package roots are the coarse ownership boundary: rebuilding the containing
                // package lets item-tree lowering rediscover any newly materialized `mod foo;`
                // files through the normal Rust module rules.
                fallback_saved_paths.insert(change.path().to_path_buf());
                for package_slot in project
                    .workspace()
                    .package_slots_containing_path(change.path())
                {
                    fallback_package_roots.push(PackageSlot(package_slot));
                }
                Vec::new()
            }
        };

        for changed_file in changed {
            let changed_file = ChangedFile {
                package: PackageSlot(changed_file.package),
                file: changed_file.file,
            };
            if changed_files_seen.insert(changed_file) {
                changed_files.push(changed_file);
            }
        }
    }

    let affected_packages = affected_packages(
        project,
        changed_files.as_slice(),
        fallback_package_roots.as_slice(),
    );
    if !affected_packages.is_empty() {
        indexing::rebuild_packages(project, database, &affected_packages)
            .context("while attempting to rebuild affected analysis packages")?;
    }
    promote_discovered_fallback_files(
        project,
        &fallback_saved_paths,
        fallback_package_roots.as_slice(),
        &mut changed_files,
        &mut changed_files_seen,
    );
    let changed_crates = crates_for_changed_files(project, &changed_files)
        .context("while attempting to report changed analysis crates")?;

    // Rebuilds seal their source inventory once discovery finishes. A watcher event can also
    // leave an open candidate without rebuilding a package.
    if affected_packages.is_empty() {
        project.parse_db().seal_sources();
        project
            .parse_db()
            .validate_saved_sources()
            .context("while attempting to validate captured project source generation")?;
        project.parse_db().evict_saved_source_text();
    } else {
        ResidencyApplication::restore(project, &affected_packages).apply()?;
        project
            .memory_hooks
            .purge(ProjectMemoryPurgePoint::AfterPackageRebuild);
    }

    Ok(AnalysisChangeSummary {
        changed_files,
        affected_packages,
        changed_crates: changed_crates.into_vec(),
    })
}

fn promote_discovered_fallback_files(
    project: &ProjectState,
    saved_paths: &HashSet<PathBuf>,
    fallback_package_roots: &[PackageSlot],
    changed_files: &mut Vec<ChangedFile>,
    changed_files_seen: &mut HashSet<ChangedFile>,
) {
    for package_slot in fallback_package_roots {
        let Some(package) = project.parse_db().package(package_slot.0) else {
            continue;
        };

        // Unknown saved files only become crate/file diagnostics candidates after a package
        // rebuild proves they are actually part of the parsed module graph. Scan each rebuilt
        // package once instead of scanning all parsed files again for every new saved path.
        for parsed_file in package.parsed_files() {
            if !saved_paths.contains(parsed_file.path()) {
                continue;
            }

            let changed_file = ChangedFile {
                package: *package_slot,
                file: parsed_file.file_id(),
            };
            if changed_files_seen.insert(changed_file) {
                changed_files.push(changed_file);
            }
        }
    }
}

fn crates_for_changed_files(
    project: &ProjectState,
    changed_files: &[ChangedFile],
) -> anyhow::Result<UniqueVec<CrateRef>> {
    let packages = changed_files
        .iter()
        .map(|changed_file| changed_file.package)
        .collect::<UniqueVec<_>>();

    // Reporting changed crates only needs package-local file ownership. Avoid materializing
    // dependency closures on the save path when semantic resolution is not involved.
    let subset = subset::packages_only(project.workspace(), packages.as_slice());
    let def_map = project.def_map_read_txn_for_subset(&subset);
    let mut crates = UniqueVec::new();

    for changed_file in changed_files {
        for crate_ref in def_map
            .crates_for_file(changed_file.package, changed_file.file)
            .context("while attempting to find crate ownership for changed file")?
        {
            crates.push(crate_ref);
        }
    }

    Ok(crates)
}
