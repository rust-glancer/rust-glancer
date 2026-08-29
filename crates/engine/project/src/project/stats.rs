//! Coarse project counters for status output and observability.
//!
//! These stats intentionally summarize the currently retained project state without exposing the
//! raw phase databases. Some phase counters are residency-sensitive because offloaded payloads are
//! intentionally absent from memory.

use rg_body_ir::BodyIrStats;
use rg_def_map::{DefMapDb, DefMapStats, PackageSlot};
use rg_semantic_ir::SemanticIrStats;
use rg_std::MemorySize;

use super::state::ProjectState;

// A warning should identify enough crates to make a bug report useful without turning a recursive
// expansion into a large log record. Detailed macro groups and ancestry remain in `analyze`.
const MAX_LISTED_MACRO_EXPANSION_LIMIT_CRATES: usize = 8;

/// Bounded diagnostics from the packages expanded during the latest def-map build.
///
/// Cache-hit packages are not expanded again, so they do not contribute to this build summary.
/// Detailed per-macro reports belong to resident DefMap packages and leave memory when those
/// packages are offloaded. This type retains only the small amount of data needed for operational
/// logging after that residency transition.
#[derive(Debug, Clone, PartialEq, Eq, Default, MemorySize)]
pub struct MacroExpansionLimitBuildSummary {
    affected_crate_count: usize,
    skipped_macro_call_count: usize,
    listed_crates: Vec<String>,
}

impl MacroExpansionLimitBuildSummary {
    pub(crate) fn capture(def_map: &DefMapDb, built_packages: &[PackageSlot]) -> Self {
        let mut summary = Self::default();

        for &package_slot in built_packages {
            let Some(package) = def_map.resident_package(package_slot) else {
                continue;
            };
            for report in package.macro_expansion_limits() {
                summary.affected_crate_count += 1;
                let report_skipped_call_count = report
                    .groups
                    .iter()
                    .fold(report.omitted_call_count, |count, group| {
                        count.saturating_add(group.skipped_call_count)
                    });
                summary.skipped_macro_call_count = summary
                    .skipped_macro_call_count
                    .saturating_add(report_skipped_call_count);

                if summary.listed_crates.len() < MAX_LISTED_MACRO_EXPANSION_LIMIT_CRATES {
                    summary
                        .listed_crates
                        .push(format!("{}/{}", report.package_name, report.crate_name));
                }
            }
        }

        summary
    }

    /// Adds one package batch while preserving the bounded user-facing crate list.
    pub(crate) fn extend(&mut self, other: Self) {
        self.affected_crate_count = self
            .affected_crate_count
            .saturating_add(other.affected_crate_count);
        self.skipped_macro_call_count = self
            .skipped_macro_call_count
            .saturating_add(other.skipped_macro_call_count);

        let remaining =
            MAX_LISTED_MACRO_EXPANSION_LIMIT_CRATES.saturating_sub(self.listed_crates.len());
        self.listed_crates
            .extend(other.listed_crates.into_iter().take(remaining));
    }

    pub fn is_empty(&self) -> bool {
        self.affected_crate_count == 0
    }

    pub fn affected_crate_count(&self) -> usize {
        self.affected_crate_count
    }

    pub fn skipped_macro_call_count(&self) -> usize {
        self.skipped_macro_call_count
    }

    /// Returns the bounded `package/crate` list suitable for one log field.
    pub fn listed_crates(&self) -> &[String] {
        &self.listed_crates
    }

    pub fn omitted_crate_count(&self) -> usize {
        self.affected_crate_count
            .saturating_sub(self.listed_crates.len())
    }
}

/// Coarse counters for one built project snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProjectStats {
    pub package_count: usize,
    pub workspace_package_count: usize,
    pub def_map: DefMapStats,
    pub semantic_ir: SemanticIrStats,
    pub body_ir: BodyIrStats,
}

impl ProjectStats {
    pub(crate) fn capture(project: &ProjectState) -> Self {
        Self {
            package_count: project.parse.package_count(),
            workspace_package_count: project.parse.workspace_packages().count(),
            def_map: project.def_map.stats(project.workspace()),
            semantic_ir: project.semantic_ir.stats(),
            body_ir: project.body_ir.stats(),
        }
    }
}
