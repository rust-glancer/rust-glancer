use rg_project::Project;
use serde::Serialize;

use super::package::PackageReport;
use crate::report::{ReportFieldsBuilder, ReportSectionBuilder};

#[derive(Debug, Serialize)]
pub(crate) struct ProjectReport {
    pub(crate) indexing_preference: String,
    pub(crate) packages: PackageReport,
    pub(crate) def_map: DefMapReport,
    pub(crate) semantic_ir: SemanticIrReport,
    pub(crate) body_ir: BodyIrReport,
}

impl ProjectReport {
    pub(crate) fn capture(project: &Project) -> Self {
        let stats = project.stats();
        let mut macro_expansion_limits =
            Vec::with_capacity(stats.def_map.macro_expansion_limit_crate_count);
        macro_expansion_limits.extend(
            project
                .macro_expansion_limit_reports()
                .map(MacroExpansionLimitReport::from),
        );

        Self {
            indexing_preference: project.indexing_preference().config_name().to_string(),
            packages: PackageReport::capture(project, &stats),
            def_map: DefMapReport {
                report_population: "resident_crate_maps",
                resident_package_count: stats.def_map.resident_package_count,
                crate_count: stats.def_map.crate_count,
                module_count: stats.def_map.module_count,
                local_def_count: stats.def_map.local_def_count,
                local_impl_count: stats.def_map.local_impl_count,
                import_count: stats.def_map.import_count,
                unresolved_import_count: stats.def_map.unresolved_import_count,
                workspace_unresolved_import_count: stats
                    .def_map
                    .unresolved_imports_by_origin
                    .workspace,
                dependency_unresolved_import_count: stats
                    .def_map
                    .unresolved_imports_by_origin
                    .dependency,
                sysroot_unresolved_import_count: stats.def_map.unresolved_imports_by_origin.sysroot,
                macro_expansion_limits,
            },
            semantic_ir: SemanticIrReport {
                crate_count: stats.semantic_ir.crate_count,
                struct_count: stats.semantic_ir.struct_count,
                union_count: stats.semantic_ir.union_count,
                enum_count: stats.semantic_ir.enum_count,
                trait_count: stats.semantic_ir.trait_count,
                impl_count: stats.semantic_ir.impl_count,
                function_count: stats.semantic_ir.function_count,
                type_alias_count: stats.semantic_ir.type_alias_count,
                const_count: stats.semantic_ir.const_count,
                static_count: stats.semantic_ir.static_count,
                lookup_index_count: stats.semantic_ir.lookup_index_count,
                lookup_index_entry_count: stats.semantic_ir.lookup_index_entry_count,
            },
            // TODO: We're missing local items in the body IR report (e.g. items/impls/functions).
            body_ir: BodyIrReport {
                crate_count: stats.body_ir.crate_count,
                built_crate_count: stats.body_ir.built_crate_count,
                skipped_crate_count: stats.body_ir.skipped_crate_count,
                complete_crate_count: stats.body_ir.complete_crate_count,
                partial_crate_count: stats.body_ir.partial_crate_count,
                missing_crate_count: stats.body_ir.missing_crate_count,
                skipped_by_policy_crate_count: stats.body_ir.skipped_by_policy_crate_count,
                body_count: stats.body_ir.body_count,
                scope_count: stats.body_ir.scope_count,
                binding_count: stats.body_ir.binding_count,
                statement_count: stats.body_ir.statement_count,
                expression_count: stats.body_ir.expression_count,
            },
        }
    }

    pub(super) fn append_document(&self, section: &mut ReportSectionBuilder) {
        section.untitled();
        section.fields("summary", |fields| {
            fields.text("indexing_preference", &self.indexing_preference);
        });
        section.fields("packages", |fields| self.packages.append_fields(fields));
        section.fields("def_map", |fields| {
            fields.title("def maps");
            self.def_map.append_fields(fields);
        });
        section.fields("semantic_ir", |fields| {
            fields.title("semantic IR");
            self.semantic_ir.append_fields(fields);
        });
        section.fields("body_ir", |fields| {
            fields.title("body IR");
            self.body_ir.append_fields(fields);
        });
    }
}

/// Serializable DefMap counters over the resident crate-map population.
///
/// The origin counters classify where each unresolved `use` was written. They partition the total;
/// they do not claim that a workspace, dependency, or sysroot package caused the failure.
#[derive(Debug, Serialize)]
pub(crate) struct DefMapReport {
    pub(crate) report_population: &'static str,
    pub(crate) resident_package_count: usize,
    pub(crate) crate_count: usize,
    pub(crate) module_count: usize,
    pub(crate) local_def_count: usize,
    pub(crate) local_impl_count: usize,
    pub(crate) import_count: usize,
    pub(crate) unresolved_import_count: usize,
    pub(crate) workspace_unresolved_import_count: usize,
    pub(crate) dependency_unresolved_import_count: usize,
    pub(crate) sysroot_unresolved_import_count: usize,
    pub(crate) macro_expansion_limits: Vec<MacroExpansionLimitReport>,
}

impl DefMapReport {
    fn append_fields(&self, fields: &mut ReportFieldsBuilder) {
        fields
            .text("report_population", self.report_population)
            .count_as(
                "resident_package_count",
                "resident packages",
                self.resident_package_count,
            )
            .count_as("crate_count", "crates", self.crate_count)
            .count_as("module_count", "modules", self.module_count)
            .count_as("local_def_count", "local definitions", self.local_def_count)
            .count_as("local_impl_count", "local impls", self.local_impl_count)
            .count_as("import_count", "imports", self.import_count)
            .count_as(
                "unresolved_import_count",
                "unresolved imports",
                self.unresolved_import_count,
            )
            .count_as(
                "workspace_unresolved_import_count",
                "workspace unresolved imports",
                self.workspace_unresolved_import_count,
            )
            .count_as(
                "dependency_unresolved_import_count",
                "dependency unresolved imports",
                self.dependency_unresolved_import_count,
            )
            .count_as(
                "sysroot_unresolved_import_count",
                "sysroot unresolved imports",
                self.sysroot_unresolved_import_count,
            )
            .count_as(
                "macro_expansion_limit_crate_count",
                "crates affected by macro expansion limit",
                self.macro_expansion_limits.len(),
            );

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

#[derive(Debug, Serialize)]
pub(crate) struct SemanticIrReport {
    pub(crate) crate_count: usize,
    pub(crate) struct_count: usize,
    pub(crate) union_count: usize,
    pub(crate) enum_count: usize,
    pub(crate) trait_count: usize,
    pub(crate) impl_count: usize,
    pub(crate) function_count: usize,
    pub(crate) type_alias_count: usize,
    pub(crate) const_count: usize,
    pub(crate) static_count: usize,
    pub(crate) lookup_index_count: usize,
    pub(crate) lookup_index_entry_count: usize,
}

impl SemanticIrReport {
    fn append_fields(&self, fields: &mut ReportFieldsBuilder) {
        fields
            .count_as("crate_count", "crates", self.crate_count)
            .count_as(
                "type_def_count",
                "type definitions",
                self.struct_count + self.union_count + self.enum_count,
            )
            .count_as("struct_count", "structs", self.struct_count)
            .count_as("union_count", "unions", self.union_count)
            .count_as("enum_count", "enums", self.enum_count)
            .count_as("trait_count", "traits", self.trait_count)
            .count_as("impl_count", "impls", self.impl_count)
            .count_as("function_count", "functions", self.function_count)
            .count_as("type_alias_count", "type aliases", self.type_alias_count)
            .count_as("const_count", "consts", self.const_count)
            .count_as("static_count", "statics", self.static_count);
        fields
            .count_as(
                "lookup_index_count",
                "local lookup indexes",
                self.lookup_index_count,
            )
            .count_as(
                "lookup_index_entry_count",
                "local lookup index entries",
                self.lookup_index_entry_count,
            );
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct BodyIrReport {
    pub(crate) crate_count: usize,
    pub(crate) built_crate_count: usize,
    pub(crate) skipped_crate_count: usize,
    pub(crate) complete_crate_count: usize,
    pub(crate) partial_crate_count: usize,
    pub(crate) missing_crate_count: usize,
    pub(crate) skipped_by_policy_crate_count: usize,
    pub(crate) body_count: usize,
    pub(crate) scope_count: usize,
    pub(crate) binding_count: usize,
    pub(crate) statement_count: usize,
    pub(crate) expression_count: usize,
}

impl BodyIrReport {
    fn append_fields(&self, fields: &mut ReportFieldsBuilder) {
        fields
            .count_as("crate_count", "crates", self.crate_count)
            .count_as("built_crate_count", "built crates", self.built_crate_count)
            .count_as(
                "skipped_crate_count",
                "skipped crates",
                self.skipped_crate_count,
            )
            .count_as(
                "complete_crate_count",
                "complete crates",
                self.complete_crate_count,
            )
            .count_as(
                "partial_crate_count",
                "partial crates",
                self.partial_crate_count,
            )
            .count_as(
                "missing_crate_count",
                "missing crates",
                self.missing_crate_count,
            )
            .count_as(
                "skipped_by_policy_crate_count",
                "crates skipped by policy",
                self.skipped_by_policy_crate_count,
            )
            .count_as("body_count", "bodies", self.body_count)
            .count_as("scope_count", "scopes", self.scope_count)
            .count_as("binding_count", "bindings", self.binding_count)
            .count_as("statement_count", "statements", self.statement_count)
            .count_as("expression_count", "expressions", self.expression_count);
    }
}
