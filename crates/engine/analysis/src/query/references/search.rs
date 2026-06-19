//! Search-surface types for reference queries.
//!
//! `ReferenceQuery` describes where the resolver should look: whole targets, selected files, or one
//! file. For example, LSP "find references" usually scans a project-chosen target list, while a
//! text prefilter can narrow that to a few files. The query also says whether declaration locations
//! should be included in the result.
//!
//! `ReferenceSearchLabel` is a plain Rust identifier that analysis has validated before the project
//! layer uses it for fast text prefiltering.

use rg_ir_model::TargetRef;
use rg_parse::FileId;

impl<'a> ReferenceQuery<'a> {
    /// Returns a query for explicit find-references requests.
    pub fn find_references(search_targets: &'a [TargetRef], include_declarations: bool) -> Self {
        let declaration_policy = if include_declarations {
            ReferenceDeclarationPolicy::IncludeUnscoped
        } else {
            ReferenceDeclarationPolicy::Exclude
        };

        Self {
            search_scope: ReferenceSearchScope::Targets(search_targets),
            declaration_policy,
        }
    }

    /// Returns a query for explicit find-references requests prefiltered to selected files.
    pub fn find_references_in_files(
        search_files: &'a [ReferenceSearchFile],
        include_declarations: bool,
    ) -> Self {
        let declaration_policy = if include_declarations {
            ReferenceDeclarationPolicy::IncludeUnscoped
        } else {
            ReferenceDeclarationPolicy::Exclude
        };

        Self {
            search_scope: ReferenceSearchScope::Files(search_files),
            declaration_policy,
        }
    }

    /// Returns a query scoped to one file inside one target.
    pub fn file_scoped(target: TargetRef, file_id: FileId) -> Self {
        Self {
            search_scope: ReferenceSearchScope::File { target, file_id },
            declaration_policy: ReferenceDeclarationPolicy::IncludeInSearchScope,
        }
    }

    /// Removes declaration locations from this query.
    pub fn without_declarations(mut self) -> Self {
        self.declaration_policy = ReferenceDeclarationPolicy::Exclude;
        self
    }

    pub(super) fn search_scope(self) -> ReferenceSearchScope<'a> {
        self.search_scope
    }

    pub(super) fn includes_declarations(self) -> bool {
        !matches!(self.declaration_policy, ReferenceDeclarationPolicy::Exclude)
    }

    pub(super) fn accepts_declaration(self, target: TargetRef, file_id: FileId) -> bool {
        match self.declaration_policy {
            ReferenceDeclarationPolicy::Exclude => false,
            ReferenceDeclarationPolicy::IncludeUnscoped => true,
            ReferenceDeclarationPolicy::IncludeInSearchScope => match self.search_scope {
                ReferenceSearchScope::Targets(targets) => targets.contains(&target),
                ReferenceSearchScope::Files(files) => files
                    .iter()
                    .any(|file| file.target == target && file.file_id == file_id),
                ReferenceSearchScope::File {
                    target: selected_target,
                    file_id: selected_file_id,
                } => selected_target == target && selected_file_id == file_id,
            },
        }
    }

    pub(super) fn accepts_scan_target(self, scan: ReferenceScanTarget) -> bool {
        match self.search_scope {
            ReferenceSearchScope::Targets(targets) => targets.contains(&scan.target),
            ReferenceSearchScope::Files(files) => scan.file_id.is_some_and(|file_id| {
                files
                    .iter()
                    .any(|file| file.target == scan.target && file.file_id == file_id)
            }),
            ReferenceSearchScope::File { target, file_id } => {
                scan.target == target && scan.file_id == Some(file_id)
            }
        }
    }
}

/// Options for a source reference lookup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReferenceQuery<'a> {
    search_scope: ReferenceSearchScope<'a>,
    declaration_policy: ReferenceDeclarationPolicy,
}

/// One target/file pair whose source should be scanned for references.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ReferenceSearchFile {
    pub target: TargetRef,
    pub file_id: FileId,
}

/// Plain Rust identifier label that is safe for request-local text prefiltering.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ReferenceSearchLabel(String);

impl ReferenceSearchLabel {
    pub(crate) fn new(label: &str) -> Option<Self> {
        let mut bytes = label.bytes();
        let first = bytes.next()?;
        if (first == b'_' || first.is_ascii_alphabetic())
            && bytes.all(|byte| byte == b'_' || byte.is_ascii_alphanumeric())
        {
            Some(Self(label.to_string()))
        } else {
            None
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Source surface scanned for reference use-sites.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ReferenceSearchScope<'a> {
    /// Scans all source candidates inside the listed targets.
    Targets(&'a [TargetRef]),
    /// Scans source candidates inside the listed target/file pairs.
    Files(&'a [ReferenceSearchFile]),
    /// Scans source candidates in one file inside one target.
    File { target: TargetRef, file_id: FileId },
}

/// How declaration locations should relate to the reference search surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReferenceDeclarationPolicy {
    /// Do not return declaration locations.
    Exclude,
    /// Return declarations even when they are outside `ReferenceSearchScope`.
    IncludeUnscoped,
    /// Return declarations only when they are inside `ReferenceSearchScope`.
    IncludeInSearchScope,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ReferenceScanTarget {
    pub(super) target: TargetRef,
    pub(super) file_id: Option<FileId>,
}
