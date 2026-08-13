//! Builds disposable project snapshots by overriding known source values.
//!
//! The caller supplies exact `CapturedSource` values already bound to the selected saved
//! generation. This module clones that generation, replaces the known source values, and rebuilds
//! the affected packages. It never updates saved fingerprints, writes package artifacts, or
//! restores offloadable residency. Callers query the result and then drop it.

use anyhow::Context as _;
use rg_body_ir::BodyIrFile;
use rg_def_map::PackageSlot;
use rg_source::CapturedSource;

use super::{ChangedFile, Project, update};

/// Package rebuild boundary for one project derived from source overrides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceOverrideScope {
    /// Rebuild only packages that own a changed file.
    ///
    /// Queries inside those packages can still read their unchanged dependencies from the base
    /// project. Reverse dependents are deliberately left untouched because they are not visible to
    /// file-local analysis such as completion, hover, or inlay hints.
    ChangedPackages,
    /// Rebuild changed packages and every package that depends on them.
    ///
    /// Workspace-wide reference and edit queries need this broader coherent graph because an
    /// overridden public declaration can change how downstream source resolves.
    ReverseDependencyClosure,
}

impl SourceOverrideScope {
    /// Returns whether a project built for `self` contains all packages requested by `other`.
    pub fn covers(self, other: Self) -> bool {
        self == other || self == Self::ReverseDependencyClosure
    }
}

pub(super) fn derive_project(
    project: &Project,
    scope: SourceOverrideScope,
    sources: impl IntoIterator<Item = CapturedSource>,
) -> anyhow::Result<Option<Project>> {
    let sources = sources
        .into_iter()
        .filter(|source| {
            project
                .state
                .parse_db()
                .source_inventory()
                .entry(source.path())
                .is_some()
        })
        .collect::<Vec<_>>();

    // The input may contain values that still match the saved generation. Preserve those exact
    // bytes in the base's evictable source cache, then derive changed packages from the same frozen
    // source identities.
    let mut has_changes = false;
    for source in &sources {
        let base_source = project
            .state
            .parse_db()
            .source_inventory()
            .entry(source.path())
            .expect("filtered captured source should remain in the immutable base inventory");
        let byte_len = source.byte_len();
        let matches =
            base_source.byte_len() == byte_len && base_source.revision() == source.revision();
        if matches {
            project
                .state
                .parse_db()
                .source_inventory()
                .retain_matching_text(source);
        } else {
            has_changes = true;
        }
    }
    if !has_changes {
        return Ok(None);
    }

    // Persisted indexes are valid shortcuts only when unchanged dependencies still come from the
    // saved project. A source-override base may carry declaration changes in packages this
    // derivation does not rebuild, and those changes are absent from every saved artifact.
    let can_reuse_saved_item_lookup_indexes = !project.has_source_overrides;
    let mut derived = project.clone();
    derived.has_source_overrides = true;
    // A saved project should not carry request state, but clearing here also makes deriving one
    // disposable project from another safe for library callers.
    derived.state.clear_query_cache();
    derived.state.parse_db().begin_source_overrides();
    let mut changed_files = Vec::new();

    for source in sources {
        let changed = derived
            .state
            .parse_db_mut()
            .apply_source_override(&source)
            .with_context(|| {
                format!(
                    "while attempting to apply source override for {}",
                    source.path().display()
                )
            })?;

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

    let source_packages = match scope {
        SourceOverrideScope::ChangedPackages => update::changed_packages(&changed_files, &[]),
        SourceOverrideScope::ReverseDependencyClosure => {
            update::affected_packages(&derived, &changed_files, &[])
        }
    };
    if source_packages.is_empty() {
        return Ok(None);
    }

    let body_files = changed_files
        .iter()
        .map(|file| BodyIrFile::new(file.package, file.file))
        .collect::<Vec<_>>();
    update::rebuild_packages_for_source_overrides(
        &mut derived.state,
        &source_packages,
        &body_files,
        can_reuse_saved_item_lookup_indexes,
    )
    .context("while attempting to rebuild packages for source overrides")?;

    Ok(Some(derived))
}
