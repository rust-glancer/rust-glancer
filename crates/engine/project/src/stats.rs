use rg_def_map::MacroExpansionLimitReport;
use rg_std::MemorySize;

// A warning should identify enough crates to make a bug report useful without turning a recursive
// expansion into a large log record. Detailed macro groups and ancestry remain in `analyze`.
const MAX_LISTED_MACRO_EXPANSION_LIMIT_CRATES: usize = 8;

/// Bounded summary of macro expansion limits in the published targets.
#[derive(Debug, Clone, PartialEq, Eq, Default, MemorySize)]
pub struct MacroExpansionLimitBuildSummary {
    affected_crate_count: usize,
    skipped_macro_call_count: usize,
    listed_crates: Vec<String>,
}

impl MacroExpansionLimitBuildSummary {
    pub(crate) fn capture(reports: &[MacroExpansionLimitReport]) -> Self {
        let mut summary = Self::default();
        for report in reports {
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
        summary
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

/// Coarse counters for the published source graph and startup body selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProjectStats {
    pub package_count: usize,
    pub workspace_package_count: usize,
    pub target_count: usize,
    pub file_count: usize,
    pub prefilled_body_count: usize,
}
