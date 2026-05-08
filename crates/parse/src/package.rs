use std::path::Path;

use anyhow::Context as _;
use rg_arena::Arena;

use crate::{FileId, LineIndex, ParsedFile, Target, TargetId, file::FileDb};
use rg_workspace::{PackageId, PackageOrigin, TargetKind};

/// Parsed package, including package-local files and target entrypoints.
#[derive(Debug, Clone)]
pub struct Package {
    /// Stable package id from workspace metadata.
    pub(crate) id: PackageId,
    /// Package name from `Cargo.toml`.
    pub(crate) package_name: String,
    /// Whether this package belongs to the analyzed workspace.
    pub(crate) is_workspace_member: bool,
    /// Where this package came from in the normalized workspace graph.
    pub(crate) origin: PackageOrigin,
    /// All parsed files known to this package.
    pub(crate) files: FileDb,
    /// Parsed targets rooted in this package.
    pub(crate) targets: Arena<TargetId, Target>,
}

impl Package {
    /// Returns the target set that rust-glancer analyzes for one Cargo package.
    ///
    /// Workspace packages keep all user-facing targets, while dependencies keep only their library
    /// target. This selection must stay shared by parse construction and cache planning, because
    /// package artifacts are keyed by the targets that actually appear in analysis payloads.
    pub fn analyzed_targets(package: &rg_workspace::Package) -> Vec<rg_workspace::Target> {
        if package.is_workspace_member {
            return package.targets.clone();
        }

        package
            .targets
            .iter()
            .filter(|target| matches!(target.kind, TargetKind::Lib))
            .cloned()
            .collect()
    }

    /// Parses a package-local source file, or returns its existing file id if it is already cached.
    pub fn parse_file(&mut self, file_path: &Path) -> anyhow::Result<FileId> {
        self.files.get_or_parse_file(file_path)
    }

    /// Reparses a package file from disk when it is already known to this package.
    pub(crate) fn reparse_saved_file(
        &mut self,
        file_path: &Path,
    ) -> anyhow::Result<Option<FileId>> {
        self.files.reparse_file_from_disk(file_path)
    }

    /// Rehydrates syntax for a known file before an AST-consuming lowering pass.
    pub fn ensure_file_syntax(&mut self, file_id: FileId) -> anyhow::Result<()> {
        self.files.ensure_file_syntax(file_id)
    }

    /// Drops retained syntax trees while preserving file ids, paths, diagnostics, and line indexes.
    pub(crate) fn evict_syntax_trees(&mut self) {
        self.files.evict_syntax_trees();
    }

    pub(crate) fn shrink_to_fit(&mut self) {
        self.package_name.shrink_to_fit();
        self.files.shrink_to_fit();
        self.targets.shrink_to_fit();
        for target in self.targets.iter_mut() {
            target.shrink_to_fit();
        }
    }

    pub(crate) fn collect_line_indexes<'a>(&'a mut self, indexes: &mut Vec<&'a mut LineIndex>) {
        self.files.collect_line_indexes(indexes);
    }

    /// Returns the cached parsed file for a previously known `FileId`.
    pub fn parsed_file(&self, file_id: FileId) -> Option<ParsedFile<'_>> {
        self.files.parsed_file(file_id)
    }

    /// Iterates over all files parsed for this package.
    pub fn parsed_files(&self) -> impl Iterator<Item = ParsedFile<'_>> {
        self.files.parsed_files()
    }

    /// Returns the path associated with a file id, if the id is valid.
    pub fn file_path(&self, file_id: FileId) -> Option<&Path> {
        self.files.file_path(file_id)
    }

    /// Returns the logical package name from the parsed package.
    pub fn package_name(&self) -> &str {
        &self.package_name
    }

    /// Returns the stable package id.
    pub fn id(&self) -> &PackageId {
        &self.id
    }

    /// Returns whether this package belongs to the analyzed workspace.
    pub fn is_workspace_member(&self) -> bool {
        self.is_workspace_member
    }

    /// Returns where this package came from in the normalized workspace graph.
    pub fn origin(&self) -> &PackageOrigin {
        &self.origin
    }

    /// Returns all parsed targets for this package.
    pub fn targets(&self) -> &[Target] {
        self.targets.as_slice()
    }

    /// Returns one parsed target by stable id.
    pub fn target(&self, target_id: TargetId) -> Option<&Target> {
        self.targets.get(target_id)
    }

    /// Parses package targets and their root files.
    pub(super) fn build(package: &rg_workspace::Package) -> anyhow::Result<Self> {
        let mut files = FileDb::default();
        let mut parsed_targets = Arena::new();

        for target in Self::analyzed_targets(package) {
            let target_id = parsed_targets.next_id();
            let root_file = files.get_or_parse_file(&target.src_path).with_context(|| {
                format!(
                    "while attempting to parse target root {}",
                    target.src_path.display()
                )
            })?;

            parsed_targets.alloc(Target {
                id: target_id,
                name: target.name,
                kind: target.kind,
                src_path: target.src_path,
                root_file,
            });
        }

        Ok(Self {
            id: package.id.clone(),
            package_name: package.name.clone(),
            is_workspace_member: package.is_workspace_member,
            origin: package.origin.clone(),
            files,
            targets: parsed_targets,
        })
    }
}
