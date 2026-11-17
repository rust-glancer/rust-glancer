//! Saved source routing for the accepted target graph.

use std::{collections::HashMap, path::Path};

use rg_arena::Arena;
use rg_ir_model::{CrateId, CrateRef, FileId, PackageSlot};
use rg_source::SourceInput;
use rg_std::MemorySize;
use wincode::{SchemaRead, SchemaWrite};

use crate::{CargoTarget, CargoTargetId, Package, file::FileDb};

/// Source routing published by one configured target after its declarations have settled.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub struct TargetFileManifest {
    pub root: FileId,
    pub files: Vec<FileId>,
}

/// Parsed project metadata, packages, and source files.
#[derive(Debug, MemorySize)]
pub struct ParseDb {
    packages: Vec<Package>,
}

impl ParseDb {
    /// Restore routing after the target queries validated against the candidate's inputs.
    /// All targets retain their original slots. The per-package file tables are only routing;
    /// unioning the manifests here does not introduce a package dependency for providers.
    pub fn from_target_manifests<'a>(
        workspace: &rg_workspace::WorkspaceMetadata,
        inputs: &ajika::Inputs,
        manifests: impl IntoIterator<Item = (CrateRef, &'a TargetFileManifest)>,
    ) -> ajika::Result<Self> {
        let manifests: HashMap<_, _> = manifests.into_iter().collect();
        let mut packages = Vec::with_capacity(workspace.packages().len());
        for (package, metadata) in workspace.packages().iter().enumerate() {
            let mut files = FileDb::new(metadata.edition);
            let mut targets = Arena::new();
            for (index, target) in metadata.analyzed_targets().enumerate() {
                let krate = CrateRef {
                    package: PackageSlot(package),
                    crate_id: CrateId(index),
                };
                let manifest = manifests.get(&krate).ok_or_else(|| {
                    ajika::Error::contract(format!("missing source manifest for {krate:?}"))
                })?;
                for &file in &manifest.files {
                    let source = SourceInput::captured(inputs, file).ok_or_else(|| {
                        ajika::Error::contract(format!(
                            "source {file:?} was not captured while validating {krate:?}"
                        ))
                    })?;
                    files.insert_captured(file, source);
                }
                if !manifest.files.contains(&manifest.root) {
                    return Err(ajika::Error::contract(format!(
                        "source manifest for {krate:?} omits its root"
                    )));
                }
                targets.alloc(CargoTarget {
                    id: CargoTargetId(index),
                    name: target.name.clone(),
                    kind: target.kind.clone(),
                    src_path: target.src_path.clone(),
                    root_file: manifest.root,
                });
            }
            packages.push(Package::from_parts(metadata, files, targets));
        }
        Ok(Self { packages })
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

    /// Returns one parsed package by slot.
    pub fn package(&self, package_slot: usize) -> Option<&Package> {
        self.packages.get(package_slot)
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
                    .map(|file| PackageFileRef {
                        package: PackageSlot(package),
                        file,
                    })
            })
            .collect()
    }
}

/// One package-local interpretation of a captured source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, MemorySize)]
pub struct PackageFileRef {
    pub package: PackageSlot,
    pub file: FileId,
}
