//! Small source-membership facts retained between query operations.

use std::collections::{BTreeSet, HashMap, HashSet};

use rg_ir_model::{CrateRef, FileId};
use rg_semantic_ir::TargetSourceFiles;
use rg_std::MemorySize;

use crate::AnalysisChangeSummary;

/// Membership published by module collection in one accepted project generation.
///
/// This is a cheap positive route for ordinary files, not an exhaustive source inventory.
/// Member includes can be discovered later through declaration-ownership queries. Capturing
/// their bytes does not mutate this index, and absence here cannot prove a file is detached.
/// No text, revisions, or package configuration are retained alongside these relationships.
#[derive(Debug, MemorySize)]
pub(crate) struct ProjectSourceIndex {
    by_crate: HashMap<CrateRef, TargetSourceFiles>,
}

impl ProjectSourceIndex {
    pub(crate) fn new(targets: impl IntoIterator<Item = (CrateRef, TargetSourceFiles)>) -> Self {
        Self {
            by_crate: targets.into_iter().collect(),
        }
    }

    pub(crate) fn contains_collected_file(&self, file: FileId) -> bool {
        self.by_crate
            .values()
            .any(|sources| sources.collected.binary_search(&file).is_ok())
    }

    /// Prefer the first written module interpretation in package/target order. Build order is
    /// dependency-first and must not change which interpretation an editor request selects.
    pub(crate) fn first_module_crate(&self, file: FileId) -> Option<CrateRef> {
        self.by_crate
            .iter()
            .filter(|(_, sources)| sources.modules.binary_search(&file).is_ok())
            .map(|(&krate, _)| krate)
            .min_by_key(|krate| (krate.package.0, krate.crate_id.0))
    }

    pub(crate) fn file_count(&self) -> usize {
        self.by_crate
            .iter()
            .flat_map(|(krate, sources)| {
                sources.collected.iter().map(|&file| (krate.package, file))
            })
            .collect::<HashSet<_>>()
            .len()
    }

    /// Combine explicit source edits with membership changes after successful validation.
    /// Source inputs report edits even for lazy includes. This index can additionally report
    /// modules entering or leaving a target without expanding those includes to find owners.
    pub(crate) fn changes_from(
        &self,
        before: &Self,
        source_changes: &[FileId],
    ) -> AnalysisChangeSummary {
        let mut changed_files = source_changes.iter().copied().collect::<BTreeSet<_>>();
        let mut changed_crates = Vec::new();
        for (&krate, sources) in &self.by_crate {
            let old = before
                .by_crate
                .get(&krate)
                .expect("source-only update preserves target identities");
            for &file in &old.collected {
                if sources.collected.binary_search(&file).is_err() {
                    changed_files.insert(file);
                }
            }
            for &file in &sources.collected {
                if old.collected.binary_search(&file).is_err() {
                    changed_files.insert(file);
                }
            }
            if old.modules != sources.modules
                || source_changes.iter().any(|file| {
                    old.modules.binary_search(file).is_ok()
                        || sources.modules.binary_search(file).is_ok()
                })
            {
                changed_crates.push(krate);
            }
        }
        changed_crates.sort_unstable_by_key(|krate| (krate.package.0, krate.crate_id.0));
        AnalysisChangeSummary {
            changed_files: changed_files.into_iter().collect(),
            changed_crates,
        }
    }
}
