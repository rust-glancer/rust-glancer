//! Builds temporary project snapshots for unsaved editor buffers.
//!
//! Dirty overlays are deliberately separate from saved-source updates. They reuse the same package
//! rebuild machinery, but never update fingerprints, write package artifacts, or restore
//! offloadable residency after the rebuild; callers query the overlay and then drop it.

use std::path::{Path, PathBuf};

use anyhow::Context as _;
use rg_body_ir::BodyIrFile;
use rg_def_map::PackageSlot;

use super::{ChangedFile, Project, update};

/// One in-memory source file used to build a temporary dirty analysis overlay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirtyFileChange {
    pub path: PathBuf,
    pub text: String,
}

impl DirtyFileChange {
    pub fn new(path: impl AsRef<Path>, text: impl Into<String>) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
            text: text.into(),
        }
    }
}

pub(super) fn build_overlay(
    project: &Project,
    changes: impl IntoIterator<Item = DirtyFileChange>,
) -> anyhow::Result<Option<Project>> {
    let changes = canonicalize_changes(changes)?;
    let mut overlay = project.clone();
    let mut changed_files = Vec::new();
    let mut fallback_package_roots = Vec::new();

    for change in changes {
        let changed = overlay
            .state
            .parse_db_mut()
            .reparse_file_from_source(&change.path, &change.text)
            .with_context(|| {
                format!(
                    "while attempting to apply dirty file change for {}",
                    change.path.display()
                )
            })?;

        if changed.is_empty() {
            // Dirty overlays intentionally avoid virtual module files for now. Rebuilding the
            // containing package can still make an existing on-disk file visible if a dirty root
            // reaches it through ordinary module discovery.
            for package_slot in overlay
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
    }

    let affected_packages =
        update::affected_packages(&overlay, &changed_files, &fallback_package_roots);
    if affected_packages.is_empty() {
        return Ok(None);
    }

    let body_files = changed_files
        .iter()
        .map(|file| BodyIrFile::new(file.package, file.file))
        .collect::<Vec<_>>();
    update::rebuild_dirty_overlay_packages(&mut overlay.state, &affected_packages, &body_files)
        .context("while attempting to rebuild dirty analysis overlay packages")?;

    Ok(Some(overlay))
}

fn canonicalize_changes(
    changes: impl IntoIterator<Item = DirtyFileChange>,
) -> anyhow::Result<Vec<DirtyFileChange>> {
    changes
        .into_iter()
        .map(|change| {
            let path = change.path.canonicalize().with_context(|| {
                format!(
                    "while attempting to canonicalize dirty file {}",
                    change.path.display()
                )
            })?;
            Ok(DirtyFileChange {
                path,
                text: change.text,
            })
        })
        .collect()
}
