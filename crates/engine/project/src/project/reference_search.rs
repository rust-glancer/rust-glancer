//! Project-side planning for reference and rename scans.
//!
//! Reference queries do not need to scan every source file. If a declaration comes from crate
//! `model`, only `model` and crates that depend on it can contain normal references to it, so
//! unrelated workspace crates can be skipped. When analysis also provides a safe label, such as
//! `name` for a local binding or field, this module does a request-local text prefilter and scans
//! only files that contain that word.
//!
//! These choices only reduce the amount of source sent to semantic matching. The analysis layer
//! still resolves every surviving candidate before it is returned as a reference.

use anyhow::Context as _;
use rg_analysis::{ReferenceSearchFile, ReferenceSearchLabel};
use rg_def_map::PackageSlot;
use rg_ir_model::CrateRef;
use rg_std::UniqueVec;

use super::{state::ProjectState, subset};

/// Builds request-local source scan surfaces for reference and rename queries.
pub(super) struct ReferenceSearchPlanner<'a> {
    state: &'a ProjectState,
}

impl<'a> ReferenceSearchPlanner<'a> {
    pub(super) fn new(state: &'a ProjectState) -> Self {
        Self { state }
    }

    /// Returns crates whose source should be scanned for an explicit references query.
    ///
    /// Queries scan the selected declaration packages and their package reverse-dependency
    /// closure. Workspace-origin queries keep that closure focused on workspace members, falling
    /// back to the whole workspace only when the declaration package is graph-opaque.
    pub(super) fn crates(
        &self,
        origin_package: PackageSlot,
        declaration_crates: &[CrateRef],
    ) -> Vec<CrateRef> {
        let packages = self.packages(origin_package, declaration_crates);
        let mut crates = UniqueVec::new();
        for package in packages {
            for crate_ref in self.state.crate_refs_for_package(package) {
                crates.push(crate_ref);
            }
        }
        crates.into_vec()
    }

    /// Returns crate/file pairs whose source text contains one of the safe reference labels.
    ///
    /// This is a request-local text prefilter. It narrows expensive semantic scans without storing
    /// a persistent text index or changing the declaration matcher that proves each result.
    pub(super) fn files_matching_labels(
        &self,
        search_crates: &[CrateRef],
        labels: &[ReferenceSearchLabel],
    ) -> anyhow::Result<Option<Vec<ReferenceSearchFile>>> {
        let Some(prefilter) = ReferenceTextPrefilter::new(labels) else {
            return Ok(None);
        };

        let packages = Self::unique_crate_packages(search_crates);
        let subset = subset::packages_only(self.state.workspace(), &packages);
        let def_map = self.state.def_map_read_txn_for_subset(&subset);

        let mut files = UniqueVec::new();
        for package in packages {
            let Some(parsed_package) = self.state.parse_db().package(package.0) else {
                continue;
            };

            for parsed_file in parsed_package.parsed_files() {
                let source = parsed_file.source_text().with_context(|| {
                    format!(
                        "while attempting to read source text for {}",
                        parsed_file.path().display()
                    )
                })?;
                if !prefilter.matches(source.as_bytes()) {
                    continue;
                }

                for crate_ref in def_map
                    .crates_for_file(package, parsed_file.file_id())
                    .context("while attempting to find crate ownership for source file")?
                {
                    if !search_crates.contains(&crate_ref) {
                        continue;
                    }
                    let file = ReferenceSearchFile {
                        crate_ref,
                        file_id: parsed_file.file_id(),
                    };
                    files.push(file);
                }
            }
        }

        Ok(Some(files.into_vec()))
    }

    /// Returns packages whose crates should be scanned for references.
    fn packages(
        &self,
        origin_package: PackageSlot,
        declaration_crates: &[CrateRef],
    ) -> Vec<PackageSlot> {
        let workspace = self.state.workspace();
        let origin_is_workspace = workspace
            .packages()
            .get(origin_package.0)
            .is_some_and(|package| package.is_workspace_member);

        // Start from the declaration package rather than the cursor package: references can only
        // appear in packages that either define the item or depend on the package that defines it.
        let mut root_packages = UniqueVec::new();
        for crate_ref in declaration_crates {
            root_packages.push(crate_ref.package);
        }
        if root_packages.is_empty() {
            root_packages.push(origin_package);
        }

        let root_ids = root_packages
            .into_iter()
            .filter_map(|package| {
                workspace
                    .packages()
                    .get(package.0)
                    .map(|package| package.id.clone())
            })
            .collect::<Vec<_>>();

        let mut packages = workspace
            .reverse_dependency_closure(&root_ids)
            .into_iter()
            .map(PackageSlot)
            .collect::<Vec<_>>();

        if origin_is_workspace {
            packages.retain(|package| {
                workspace
                    .packages()
                    .get(package.0)
                    .is_some_and(|package| package.is_workspace_member)
            });

            if packages.is_empty() {
                // Some roots, such as sysroot packages, are not always represented by normal Cargo
                // dependency edges. Keep those queries complete by preserving the old workspace scan.
                packages.extend(workspace.packages().iter().enumerate().filter_map(
                    |(slot, package)| package.is_workspace_member.then_some(PackageSlot(slot)),
                ));
            }
        }

        packages
    }

    fn unique_crate_packages(crates: &[CrateRef]) -> Vec<PackageSlot> {
        crates
            .iter()
            .map(|crate_ref| crate_ref.package)
            .collect::<UniqueVec<_>>()
            .into_vec()
    }
}

struct ReferenceTextPrefilter<'label> {
    finders: Vec<(memchr::memmem::Finder<'label>, usize)>,
}

impl<'label> ReferenceTextPrefilter<'label> {
    fn new(labels: &'label [ReferenceSearchLabel]) -> Option<Self> {
        if labels.is_empty() {
            return None;
        }

        Some(Self {
            finders: labels
                .iter()
                .map(|label| {
                    (
                        memchr::memmem::Finder::new(label.as_str().as_bytes()),
                        label.as_str().len(),
                    )
                })
                .collect(),
        })
    }

    fn matches(&self, source: &[u8]) -> bool {
        self.finders.iter().any(|(finder, label_len)| {
            finder.find_iter(source).any(|start| {
                let end = start + *label_len;
                Self::is_label_start_boundary(source, start)
                    && Self::is_label_end_boundary(source, end)
            })
        })
    }

    fn is_label_start_boundary(source: &[u8], idx: usize) -> bool {
        idx == 0 || !Self::is_label_byte(source[idx - 1])
    }

    fn is_label_end_boundary(source: &[u8], idx: usize) -> bool {
        source
            .get(idx)
            .is_none_or(|byte| !Self::is_label_byte(*byte))
    }

    fn is_label_byte(byte: u8) -> bool {
        byte == b'_' || byte.is_ascii_alphanumeric()
    }
}
