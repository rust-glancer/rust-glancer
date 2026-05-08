//! Resident parsed-source database.

use std::{
    fmt,
    path::{Path, PathBuf},
};

use anyhow::Context as _;

use crate::{FileId, LineIndex, Package, PackageParseSnapshot};

/// Parsed project metadata, packages, and source files.
#[derive(Debug, Clone)]
pub struct ParseDb {
    pub(crate) workspace_root: PathBuf,
    pub(crate) packages: Vec<Package>,
}

/// One package-local file touched by a saved file update.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PackageFileRef {
    pub package: usize,
    pub file: FileId,
}

impl ParseDb {
    /// Builds parsed packages for one normalized workspace metadata graph.
    pub fn build(workspace: &rg_workspace::WorkspaceMetadata) -> anyhow::Result<Self> {
        let packages = workspace
            .packages()
            .iter()
            .map(|package| {
                Package::build(package).with_context(|| {
                    format!(
                        "while attempting to build parsed package for {}",
                        package.id
                    )
                })
            })
            .collect::<Result<Vec<_>, _>>()?;

        Ok(Self {
            workspace_root: workspace.workspace_root().to_path_buf(),
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

    /// Restores package-local file ids and source maps from a validated package artifact.
    pub fn apply_package_parse_snapshot(
        &mut self,
        package_slot: usize,
        snapshot: PackageParseSnapshot,
    ) -> anyhow::Result<()> {
        let package = self
            .package_mut(package_slot)
            .with_context(|| format!("while attempting to fetch parsed package {package_slot}"))?;
        package.apply_parse_snapshot(snapshot)
    }

    /// Returns whether a canonical path is already known to any parsed package.
    pub fn contains_file_path(&self, file_path: &Path) -> bool {
        self.packages
            .iter()
            .any(|package| package.parsed_files().any(|file| file.path() == file_path))
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

    /// Reparses a saved file for every parsed package that already owns `file_path`.
    ///
    /// This keeps package-local `FileId`s stable. Unknown files do not appear in the returned owner
    /// list yet; if a package rebuild later discovers them through `mod foo;`, ordinary parsing
    /// reads the same saved filesystem snapshot from disk.
    pub fn reparse_saved_file(&mut self, file_path: &Path) -> anyhow::Result<Vec<PackageFileRef>> {
        let canonical_file_path = file_path
            .canonicalize()
            .with_context(|| format!("while attempting to canonicalize {}", file_path.display()))?;
        let mut changed_files = Vec::new();

        for (package_slot, package) in self.packages.iter_mut().enumerate() {
            let Some(file_id) = package.reparse_saved_file(&canonical_file_path)? else {
                continue;
            };

            changed_files.push(PackageFileRef {
                package: package_slot,
                file: file_id,
            });
        }

        Ok(changed_files)
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
