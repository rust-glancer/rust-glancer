//! Applies ordinary source-file saves without invalidating the workspace graph.
//!
//! This path keeps package and target slots stable. It reparses the saved file, rebuilds affected
//! packages and their reverse dependents, and reports changed targets from the updated def-map
//! snapshot.

use std::{collections::HashSet, path::PathBuf};

use anyhow::Context as _;

use rg_def_map::PackageSlot;
use rg_ir_model::TargetRef;
use rg_std::UniqueVec;

use super::{affected_packages, package};
use crate::project::{AnalysisChangeSummary, ChangedFile, Project, SavedFileChange, subset};

pub(super) fn apply_source_changes(
    project: &mut Project,
    changes: Vec<SavedFileChange>,
) -> anyhow::Result<AnalysisChangeSummary> {
    // Read every source before changing ParseDb. A later I/O error can then reject the update
    // without leaving parsed files ahead of the package databases built from them.
    let mut staged_changes = Vec::with_capacity(changes.len());
    for change in changes {
        let source = match std::fs::read_to_string(&change.path) {
            Ok(source) => source,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                // The path disappeared after canonicalization, usually because a checkout or
                // rename advanced again while this command waited. Other staged paths remain valid.
                continue;
            }
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "while attempting to stage saved file change for {}",
                        change.path.display()
                    )
                });
            }
        };
        staged_changes.push((change, source));
    }

    let mut changed_files = Vec::new();
    let mut changed_files_seen = HashSet::new();
    let mut fallback_package_roots = UniqueVec::new();
    let mut fallback_saved_paths = HashSet::new();

    // Reparse every known file first, then rebuild the union of affected packages once. This keeps
    // large watcher batches proportional to the changed package set instead of to the number of
    // changed paths in the batch.
    for (change, source) in staged_changes {
        let changed = project
            .state
            .parse_db_mut()
            .reparse_saved_file_from_source(&change.path, &source);

        if changed.is_empty() {
            // A saved file can be new to the graph even though it now exists on disk. In that case,
            // package roots are the coarse ownership boundary: rebuilding the containing package
            // lets item-tree lowering rediscover any newly materialized `mod foo;` files through
            // the normal Rust module rules.
            fallback_saved_paths.insert(change.path.clone());
            for package_slot in project
                .state
                .workspace()
                .package_slots_containing_path(&change.path)
            {
                fallback_package_roots.push(PackageSlot(package_slot));
            }
        }

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
        package::rebuild_packages(&mut project.state, &affected_packages)
            .context("while attempting to rebuild affected analysis packages")?;
    }
    promote_discovered_fallback_files(
        project,
        &fallback_saved_paths,
        fallback_package_roots.as_slice(),
        &mut changed_files,
        &mut changed_files_seen,
    );
    let changed_targets = targets_for_changed_files(project, &changed_files)
        .context("while attempting to report changed analysis targets")?;

    Ok(AnalysisChangeSummary {
        changed_files,
        affected_packages,
        changed_targets: changed_targets.into_vec(),
    })
}

fn promote_discovered_fallback_files(
    project: &Project,
    saved_paths: &HashSet<PathBuf>,
    fallback_package_roots: &[PackageSlot],
    changed_files: &mut Vec<ChangedFile>,
    changed_files_seen: &mut HashSet<ChangedFile>,
) {
    for package_slot in fallback_package_roots {
        let Some(package) = project.state.parse_db().package(package_slot.0) else {
            continue;
        };

        // Unknown saved files only become target/file diagnostics candidates after a package
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

fn targets_for_changed_files(
    project: &Project,
    changed_files: &[ChangedFile],
) -> anyhow::Result<UniqueVec<TargetRef>> {
    let packages = changed_files
        .iter()
        .map(|changed_file| changed_file.package)
        .collect::<UniqueVec<_>>();

    // Reporting changed targets only needs package-local file ownership. Avoid materializing
    // dependency closures on the save path when semantic resolution is not involved.
    let subset = subset::packages_only(project.state.workspace(), packages.as_slice());
    let def_map = project.state.def_map_read_txn_for_subset(&subset);
    let mut targets = UniqueVec::new();

    for changed_file in changed_files {
        for target_ref in def_map
            .targets_for_file(changed_file.package, changed_file.file)
            .context("while attempting to find target ownership for changed file")?
        {
            targets.push(target_ref);
        }
    }

    Ok(targets)
}
