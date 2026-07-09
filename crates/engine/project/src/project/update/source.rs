//! Applies ordinary source-file saves without invalidating the workspace graph.
//!
//! This path keeps package and target slots stable. It reparses the saved file, rebuilds affected
//! packages and their reverse dependents, and reports changed targets from the updated def-map
//! snapshot.

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
    let mut changed_files = UniqueVec::new();
    let mut fallback_package_roots = UniqueVec::new();
    let mut fallback_saved_paths = Vec::new();

    // Reparse every known file first, then rebuild the union of affected packages once. This keeps
    // large watcher batches proportional to the changed package set instead of to the number of
    // changed paths in the batch.
    for change in changes {
        let changed = project
            .state
            .parse_db_mut()
            .reparse_saved_file(&change.path)
            .with_context(|| {
                format!(
                    "while attempting to apply saved file change for {}",
                    change.path.display()
                )
            })?;

        if changed.is_empty() {
            // A saved file can be new to the graph even though it now exists on disk. In that case,
            // package roots are the coarse ownership boundary: rebuilding the containing package
            // lets item-tree lowering rediscover any newly materialized `mod foo;` files through
            // the normal Rust module rules.
            fallback_saved_paths.push(change.path.clone());
            for package_slot in project
                .state
                .workspace()
                .package_slots_containing_path(&change.path)
            {
                fallback_package_roots.push(PackageSlot(package_slot));
            }
        }

        for changed_file in changed {
            changed_files.push(ChangedFile {
                package: PackageSlot(changed_file.package),
                file: changed_file.file,
            });
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
    for saved_path in fallback_saved_paths {
        promote_discovered_fallback_file(
            project,
            saved_path.as_path(),
            fallback_package_roots.as_slice(),
            &mut changed_files,
        );
    }
    let changed_targets = targets_for_changed_files(project, changed_files.as_slice())
        .context("while attempting to report changed analysis targets")?;

    Ok(AnalysisChangeSummary {
        changed_files: changed_files.into_vec(),
        affected_packages,
        changed_targets: changed_targets.into_vec(),
    })
}

fn promote_discovered_fallback_file(
    project: &Project,
    saved_path: &std::path::Path,
    fallback_package_roots: &[PackageSlot],
    changed_files: &mut UniqueVec<ChangedFile>,
) {
    for package_slot in fallback_package_roots {
        let Some(package) = project.state.parse_db().package(package_slot.0) else {
            continue;
        };

        // Unknown saved files only become target/file diagnostics candidates after a package
        // rebuild proves they are actually part of the parsed module graph.
        for parsed_file in package.parsed_files() {
            if parsed_file.path() != saved_path {
                continue;
            }

            changed_files.push(ChangedFile {
                package: *package_slot,
                file: parsed_file.file_id(),
            });
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
