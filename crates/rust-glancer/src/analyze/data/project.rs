use rg_project::Project;
use serde::Serialize;

use crate::report::{ReportFieldsBuilder, ReportSectionBuilder};

#[derive(Debug, Serialize)]
pub(crate) struct ProjectReport {
    package_count: usize,
    workspace_package_count: usize,
    target_count: usize,
    file_count: usize,
    prefilled_body_count: usize,
    macro_expansion_limits: Vec<MacroExpansionLimitReport>,
}

impl ProjectReport {
    pub(crate) fn capture(project: &Project) -> Self {
        let stats = project.stats();
        Self {
            package_count: stats.package_count,
            workspace_package_count: stats.workspace_package_count,
            target_count: stats.target_count,
            file_count: stats.file_count,
            prefilled_body_count: stats.prefilled_body_count,
            macro_expansion_limits: project
                .macro_expansion_limit_reports()
                .map(Into::into)
                .collect(),
        }
    }

    pub(super) fn append_document(&self, section: &mut ReportSectionBuilder) {
        section.untitled();
        section.fields("project", |fields| {
            fields.title("Saved graph");
            fields
                .count_as("package_count", "packages", self.package_count)
                .count_as(
                    "workspace_package_count",
                    "workspace packages",
                    self.workspace_package_count,
                )
                .count_as("target_count", "targets", self.target_count)
                .count_as("file_count", "files across packages", self.file_count)
                .count_as(
                    "prefilled_body_count",
                    "prefilled body owners",
                    self.prefilled_body_count,
                );
        });
        if !self.macro_expansion_limits.is_empty() {
            section.fields("macro_expansion_limits", |fields| {
                self.append_macro_limits(fields)
            });
        }
    }

    fn append_macro_limits(&self, fields: &mut ReportFieldsBuilder) {
        for (report_index, report) in self.macro_expansion_limits.iter().enumerate() {
            for (group_index, group) in report.groups.iter().enumerate() {
                let mut detail = format!(
                    "{}/{} {}: {} skipped ({} source, {} generated)",
                    report.package_name,
                    report.crate_name,
                    group.macro_name,
                    group.skipped_call_count,
                    group.source_call_count,
                    group.generated_call_count,
                );
                if !group.example_chain.is_empty() {
                    detail.push_str("; chain ");
                    detail.push_str(&group.example_chain.join(" -> "));
                    if group.chain_truncated {
                        detail.push_str(" -> …");
                    }
                }
                fields.text(
                    format!("macro_expansion_limit_{report_index}_{group_index}"),
                    detail,
                );
            }
            if report.omitted_call_count > 0 {
                fields.count_as(
                    format!("macro_expansion_limit_{report_index}_omitted_call_count"),
                    format!(
                        "{}/{} omitted macro-limit calls",
                        report.package_name, report.crate_name
                    ),
                    report.omitted_call_count,
                );
            }
        }
    }
}

/// Report-facing copy of one crate's bounded macro-limit diagnostic.
#[derive(Debug, Serialize)]
pub(crate) struct MacroExpansionLimitReport {
    pub(crate) package_name: String,
    pub(crate) crate_name: String,
    pub(crate) groups: Vec<MacroExpansionLimitGroup>,
    pub(crate) omitted_call_count: usize,
}

impl From<&rg_project::MacroExpansionLimitReport> for MacroExpansionLimitReport {
    fn from(report: &rg_project::MacroExpansionLimitReport) -> Self {
        Self {
            package_name: report.package_name.clone(),
            crate_name: report.crate_name.clone(),
            groups: report
                .groups
                .iter()
                .map(MacroExpansionLimitGroup::from)
                .collect(),
            omitted_call_count: report.omitted_call_count,
        }
    }
}

/// Counts and one source-to-leaf ancestry example for a rendered macro identity.
#[derive(Debug, Serialize)]
pub(crate) struct MacroExpansionLimitGroup {
    pub(crate) macro_name: String,
    pub(crate) skipped_call_count: usize,
    pub(crate) source_call_count: usize,
    pub(crate) generated_call_count: usize,
    pub(crate) example_chain: Vec<String>,
    pub(crate) chain_truncated: bool,
}

impl From<&rg_project::MacroExpansionLimitGroup> for MacroExpansionLimitGroup {
    fn from(group: &rg_project::MacroExpansionLimitGroup) -> Self {
        Self {
            macro_name: group.macro_name.clone(),
            skipped_call_count: group.skipped_call_count,
            source_call_count: group.source_call_count,
            generated_call_count: group.generated_call_count,
            example_chain: group.example_chain.clone(),
            chain_truncated: group.chain_truncated,
        }
    }
}
