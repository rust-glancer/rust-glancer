//! Applies ordinary source-file saves without invalidating the workspace graph.
//!
//! This path keeps package and target slots stable. It reparses the saved file, rebuilds affected
//! packages and their reverse dependents, and reports changed targets from the updated def-map
//! snapshot.

use anyhow::Context as _;

use rg_def_map::{PackageSlot, TargetRef};

use super::{affected_packages, package};
use crate::project::{AnalysisChangeSummary, ChangedFile, Project, SavedFileChange};

pub(super) fn apply_source_change(
    project: &mut Project,
    change: SavedFileChange,
) -> anyhow::Result<AnalysisChangeSummary> {
    let mut changed_files = Vec::new();
    let mut fallback_package_roots = Vec::new();
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

    let fallback_saved_path = changed.is_empty().then(|| change.path.clone());
    if fallback_saved_path.is_some() {
        // A saved file can be new to the graph even though it now exists on disk. In that case,
        // package roots are the coarse ownership boundary: rebuilding the containing package lets
        // item-tree lowering rediscover any newly materialized `mod foo;` files through the normal
        // Rust module rules.
        for package_slot in project
            .state
            .workspace()
            .package_slots_containing_path(&change.path)
        {
            let package_slot = PackageSlot(package_slot);
            if !fallback_package_roots.contains(&package_slot) {
                fallback_package_roots.push(package_slot);
            }
        }
    }

    for changed_file in changed {
        let changed_file = ChangedFile {
            package: PackageSlot(changed_file.package),
            file: changed_file.file,
        };
        if !changed_files.contains(&changed_file) {
            changed_files.push(changed_file);
        }
    }

    let affected_packages = affected_packages(project, &changed_files, &fallback_package_roots);
    if !affected_packages.is_empty() {
        package::rebuild_packages(&mut project.state, &affected_packages)
            .context("while attempting to rebuild affected analysis packages")?;
    }
    if let Some(saved_path) = fallback_saved_path {
        promote_discovered_fallback_file(
            project,
            &saved_path,
            &fallback_package_roots,
            &mut changed_files,
        );
    }
    let changed_targets = targets_for_changed_files(project, &changed_files)
        .context("while attempting to report changed analysis targets")?;

    Ok(AnalysisChangeSummary {
        changed_files,
        affected_packages,
        changed_targets,
    })
}

fn promote_discovered_fallback_file(
    project: &Project,
    saved_path: &std::path::Path,
    fallback_package_roots: &[PackageSlot],
    changed_files: &mut Vec<ChangedFile>,
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

            let changed_file = ChangedFile {
                package: *package_slot,
                file: parsed_file.file_id(),
            };
            if !changed_files.contains(&changed_file) {
                changed_files.push(changed_file);
            }
        }
    }
}

fn targets_for_changed_files(
    project: &Project,
    changed_files: &[ChangedFile],
) -> anyhow::Result<Vec<TargetRef>> {
    let packages = changed_files
        .iter()
        .map(|changed_file| changed_file.package)
        .collect::<Vec<_>>();
    let snapshot = project.snapshot();
    // Reporting changed targets only needs package-local file ownership. Avoid materializing
    // dependency closures on the save path when semantic resolution is not involved.
    let analysis = snapshot.shallow_analysis(&packages)?;
    let mut targets = Vec::new();

    for changed_file in changed_files {
        for target_ref in analysis.targets_for_file(changed_file.package, changed_file.file)? {
            if !targets.contains(&target_ref) {
                targets.push(target_ref);
            }
        }
    }

    Ok(targets)
}
