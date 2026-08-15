//! Resident parsed-source database.

use std::{
    fmt,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::Context as _;

use crate::{FileId, LineIndex, Package, PackageParseSnapshot};
use rg_source::{CapturedSource, SourceEntry, SourceError, SourceInventory};
use rg_std::MemorySize;

/// Parsed project metadata, packages, and source files.
#[derive(Debug, MemorySize)]
pub struct ParseDb {
    pub(crate) workspace_root: PathBuf,
    pub(crate) sources: Arc<SourceInventory>,
    pub(crate) packages: Vec<Package>,
}

/// One package-local file touched by a saved file update.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, MemorySize)]
pub struct PackageFileRef {
    pub package: usize,
    pub file: FileId,
}

/// Result of refreshing one saved path against a parsed project generation.
///
/// A watcher notification does not necessarily mean that source bytes changed: rescan recovery
/// can report a path already applied by an earlier batch. Unknown paths are kept separate because
/// they may be newly created Rust modules that package-level discovery still needs to find.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SavedFileRefresh {
    /// The saved bytes have the same strong source identity as the existing generation.
    Unchanged,
    /// The path has new bytes and was reparsed in every package that already owns it.
    Reparsed(Vec<PackageFileRef>),
    /// The path is not present in any package file table and needs module rediscovery.
    Unknown,
}

impl ParseDb {
    /// Builds parsed packages for one normalized workspace metadata graph.
    pub fn build(workspace: &rg_workspace::WorkspaceMetadata) -> anyhow::Result<Self> {
        let sources = Arc::new(SourceInventory::new());
        let mut packages = Vec::with_capacity(workspace.packages().len());
        for package in workspace.packages() {
            packages.push(Package::build(package, &sources).with_context(|| {
                format!(
                    "while attempting to build parsed package for {}",
                    package.id
                )
            })?);
        }

        Ok(Self {
            workspace_root: workspace.workspace_root().to_path_buf(),
            sources,
            packages,
        })
    }

    /// Iterates over parsed packages that belong to the workspace members set.
    pub fn workspace_packages(&self) -> impl Iterator<Item = &Package> + '_ {
        self.packages
            .iter()
            .filter(|package| package.is_workspace_member())
    }

    /// Returns the number of parsed packages.
    pub fn package_count(&self) -> usize {
        self.packages.len()
    }

    /// Returns all parsed packages.
    pub fn packages(&self) -> &[Package] {
        &self.packages
    }

    /// Returns all parsed packages as disjoint mutable slots.
    ///
    /// Phase builders use this when they can process each package independently. Exposing the
    /// slice keeps ownership explicit while allowing callers to split work without repeated
    /// package-slot lookups.
    pub fn packages_mut(&mut self) -> &mut [Package] {
        &mut self.packages
    }

    /// Returns one parsed package by slot.
    pub fn package(&self, package_slot: usize) -> Option<&Package> {
        self.packages.get(package_slot)
    }

    /// Returns one mutable parsed package by slot.
    pub fn package_mut(&mut self, package_slot: usize) -> Option<&mut Package> {
        self.packages.get_mut(package_slot)
    }

    /// Recreates selected package file tables from their workspace target roots.
    ///
    /// ItemTree module discovery runs immediately after this reset and repopulates only source
    /// files reachable by the new module graph. Downstream phases rebuild the same package set, so
    /// assigning fresh package-local file ids does not reconnect retained IR to different files.
    pub fn reset_packages_from_workspace(
        &mut self,
        workspace: &rg_workspace::WorkspaceMetadata,
        package_slots: &[usize],
    ) -> anyhow::Result<()> {
        for &package_slot in package_slots {
            let workspace_package = workspace.packages().get(package_slot).with_context(|| {
                format!("while attempting to fetch workspace package {package_slot}")
            })?;
            let replacement =
                Package::build(workspace_package, &self.sources).with_context(|| {
                    format!(
                        "while attempting to reset parsed package {} from target roots",
                        workspace_package.id,
                    )
                })?;
            let package = self.packages.get_mut(package_slot).with_context(|| {
                format!("while attempting to fetch parsed package {package_slot}")
            })?;
            *package = replacement;
        }
        Ok(())
    }

    /// Restores package-local file ids and source maps from a validated package artifact.
    pub fn apply_package_parse_snapshot(
        &mut self,
        package_slot: usize,
        snapshot: PackageParseSnapshot,
    ) -> anyhow::Result<()> {
        let files = snapshot
            .files()
            .iter()
            .map(|file| {
                self.sources
                    .capture_descriptor(file.source_descriptor())
                    .map(|source| (file.clone(), source))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let package = self
            .package_mut(package_slot)
            .with_context(|| format!("while attempting to fetch parsed package {package_slot}"))?;
        package.apply_parse_snapshot(snapshot, files)
    }

    /// Captures every file named by a package artifact and verifies its exact source revision.
    pub fn validate_package_parse_snapshot(
        &self,
        snapshot: &PackageParseSnapshot,
    ) -> anyhow::Result<()> {
        for file in snapshot.files() {
            self.sources.capture_descriptor(file.source_descriptor())?;
        }
        Ok(())
    }

    /// Returns whether a canonical path is already known to any parsed package.
    pub fn contains_file_path(&self, file_path: &Path) -> bool {
        self.packages
            .iter()
            .any(|package| package.file_id_for_path(file_path).is_some())
    }

    /// Returns every package-local file reference matching a canonical source path.
    ///
    /// One source file can participate in several Cargo packages, so path lookup preserves every
    /// matching package context while using each package's existing path map.
    pub fn file_refs_for_path(&self, file_path: &Path) -> Vec<PackageFileRef> {
        self.packages
            .iter()
            .enumerate()
            .filter_map(|(package, parsed_package)| {
                parsed_package
                    .file_id_for_path(file_path)
                    .map(|file| PackageFileRef { package, file })
            })
            .collect()
    }

    /// Drops retained syntax trees from all packages after AST-consuming phases have finished.
    pub fn evict_syntax_trees(&mut self) {
        for package in &mut self.packages {
            package.evict_syntax_trees();
        }
    }

    /// Compacts saved parse metadata after a project snapshot has finished building.
    pub fn shrink_to_fit(&mut self) {
        self.packages.shrink_to_fit();
        self.sources.shrink_to_fit();
        for package in &mut self.packages {
            package.shrink_to_fit();
        }
    }

    /// Packs retained line indexes for all parsed files into shared source-map buffers.
    pub fn pack_line_indexes(&mut self) {
        let packages = (0..self.packages.len()).collect::<Vec<_>>();
        self.pack_line_indexes_for_packages(&packages);
    }

    /// Packs line indexes for selected packages into shared source-map buffers.
    pub fn pack_line_indexes_for_packages(&mut self, packages: &[usize]) {
        if packages.is_empty() {
            return;
        }

        let mut indexes = Vec::new();
        for (package_slot, package) in self.packages.iter_mut().enumerate() {
            if packages.contains(&package_slot) {
                package.collect_line_indexes(&mut indexes);
            }
        }

        LineIndex::pack_many(indexes.as_mut_slice());
    }

    /// Drops retained line indexes for packages whose source maps are backed by source files.
    pub fn offload_line_indexes_for_packages(&mut self, packages: &[usize]) {
        for package_slot in packages {
            let Some(package) = self.packages.get_mut(*package_slot) else {
                continue;
            };
            package.offload_line_indexes();
        }
    }

    /// Refreshes a saved file by reading disk once at the start of this project update.
    ///
    /// `canonical_file_path` must use the same canonical identity stored in the parse database.
    /// Exact watcher replays are reported without reparsing. Unlike editor-source reparsing, a
    /// changed parsed file remains filesystem-backed so captured source text is not retained after
    /// the update finishes.
    pub fn refresh_saved_file_from_disk(
        &mut self,
        canonical_file_path: &Path,
    ) -> Result<SavedFileRefresh, SourceError> {
        let known_file = self.contains_file_path(canonical_file_path);
        self.sources.begin_capture();
        let previous = self.sources.entry(canonical_file_path);
        let source = self.sources.replace_saved_from_disk(canonical_file_path)?;
        Ok(self.install_saved_source(canonical_file_path, known_file, previous, source))
    }

    /// Refreshes a saved file from exact source captured before this project update was submitted.
    pub fn refresh_captured_saved_file(
        &mut self,
        source: &CapturedSource,
    ) -> Result<SavedFileRefresh, SourceError> {
        let known_file = self.contains_file_path(source.path());
        self.sources.begin_capture();
        let previous = self.sources.entry(source.path());
        let entry = self.sources.replace_saved(source)?;
        Ok(self.install_saved_source(source.path(), known_file, previous, entry))
    }

    /// Installs one already-captured saved entry into every package-local file table.
    fn install_saved_source(
        &mut self,
        canonical_file_path: &Path,
        known_file: bool,
        previous: Option<Arc<SourceEntry>>,
        source: Arc<SourceEntry>,
    ) -> SavedFileRefresh {
        if known_file
            && previous.as_ref().is_some_and(|previous| {
                previous.revision() == source.revision() && previous.byte_len() == source.byte_len()
            })
        {
            return SavedFileRefresh::Unchanged;
        }
        let mut changed_files = Vec::new();

        for (package_slot, package) in self.packages.iter_mut().enumerate() {
            let Some(file_id) =
                package.reparse_saved_file_from_source(canonical_file_path, Arc::clone(&source))
            else {
                continue;
            };

            changed_files.push(PackageFileRef {
                package: package_slot,
                file: file_id,
            });
        }

        if changed_files.is_empty() {
            SavedFileRefresh::Unknown
        } else {
            SavedFileRefresh::Reparsed(changed_files)
        }
    }

    /// Returns the source inventory shared by every package-local file entry.
    pub fn source_inventory(&self) -> &SourceInventory {
        &self.sources
    }

    /// Returns a shared handle for parallel source discovery during item-tree lowering.
    pub fn source_inventory_handle(&self) -> Arc<SourceInventory> {
        Arc::clone(&self.sources)
    }

    /// Seals the source set after all file-discovering phases have completed.
    pub fn seal_sources(&self) {
        // Source capture can start before package ownership is known, and saved rebuilds can replace
        // an older module graph. The package file tables are authoritative after discovery, so
        // retire everything outside their union before validating or snapshotting the generation.
        self.sources.retain_paths(
            self.packages
                .iter()
                .flat_map(Package::parsed_files)
                .map(|file| file.path().to_path_buf()),
        );
        self.sources.seal();
    }

    /// Rejects a generation candidate if any captured saved source changed during construction.
    pub fn validate_saved_sources(&self) -> anyhow::Result<()> {
        Ok(self.sources.validate_saved()?)
    }

    /// Releases exact saved text while retaining strong source identity for verified reloads.
    pub fn evict_saved_source_text(&self) {
        self.sources.evict_saved_text();
    }
}

impl Clone for ParseDb {
    fn clone(&self) -> Self {
        Self {
            workspace_root: self.workspace_root.clone(),
            sources: Arc::new(self.sources.fork()),
            packages: self.packages.clone(),
        }
    }
}

/// Renders a project-level report of parsed packages and diagnostics.
impl fmt::Display for ParseDb {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let workspace_member_count = self.workspace_packages().count();
        let dependency_count = self.packages.len().saturating_sub(workspace_member_count);
        writeln!(f, "Project {}", self.workspace_root.display())?;
        writeln!(
            f,
            "Packages {} (workspace members: {}, dependencies: {})",
            self.packages.len(),
            workspace_member_count,
            dependency_count,
        )?;

        for package in &self.packages {
            writeln!(f)?;
            writeln!(f, "Package {} [{}]", package.package_name(), package.id())?;
        }

        Ok(())
    }
}
