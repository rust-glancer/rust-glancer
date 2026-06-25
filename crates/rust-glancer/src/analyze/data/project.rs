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
        Self {
            indexing_preference: project.indexing_preference().config_name().to_string(),
            packages: PackageReport::capture(project),
            def_map: DefMapReport {
                target_count: stats.def_map.target_count,
                module_count: stats.def_map.module_count,
                local_def_count: stats.def_map.local_def_count,
                local_impl_count: stats.def_map.local_impl_count,
                import_count: stats.def_map.import_count,
                unresolved_import_count: stats.def_map.unresolved_import_count,
            },
            semantic_ir: SemanticIrReport {
                target_count: stats.semantic_ir.target_count,
                struct_count: stats.semantic_ir.struct_count,
                union_count: stats.semantic_ir.union_count,
                enum_count: stats.semantic_ir.enum_count,
                trait_count: stats.semantic_ir.trait_count,
                impl_count: stats.semantic_ir.impl_count,
                function_count: stats.semantic_ir.function_count,
                type_alias_count: stats.semantic_ir.type_alias_count,
                const_count: stats.semantic_ir.const_count,
                static_count: stats.semantic_ir.static_count,
            },
            // TODO: We're missing local items in the body IR report (e.g. items/impls/functions).
            body_ir: BodyIrReport {
                target_count: stats.body_ir.target_count,
                built_target_count: stats.body_ir.built_target_count,
                skipped_target_count: stats.body_ir.skipped_target_count,
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

#[derive(Debug, Serialize)]
pub(crate) struct DefMapReport {
    pub(crate) target_count: usize,
    pub(crate) module_count: usize,
    pub(crate) local_def_count: usize,
    pub(crate) local_impl_count: usize,
    pub(crate) import_count: usize,
    pub(crate) unresolved_import_count: usize,
}

impl DefMapReport {
    fn append_fields(&self, fields: &mut ReportFieldsBuilder) {
        fields
            .count_as("target_count", "targets", self.target_count)
            .count_as("module_count", "modules", self.module_count)
            .count_as("local_def_count", "local definitions", self.local_def_count)
            .count_as("local_impl_count", "local impls", self.local_impl_count)
            .count_as("import_count", "imports", self.import_count)
            .count_as(
                "unresolved_import_count",
                "unresolved imports",
                self.unresolved_import_count,
            );
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct SemanticIrReport {
    pub(crate) target_count: usize,
    pub(crate) struct_count: usize,
    pub(crate) union_count: usize,
    pub(crate) enum_count: usize,
    pub(crate) trait_count: usize,
    pub(crate) impl_count: usize,
    pub(crate) function_count: usize,
    pub(crate) type_alias_count: usize,
    pub(crate) const_count: usize,
    pub(crate) static_count: usize,
}

impl SemanticIrReport {
    fn append_fields(&self, fields: &mut ReportFieldsBuilder) {
        fields
            .count_as("target_count", "targets", self.target_count)
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
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct BodyIrReport {
    pub(crate) target_count: usize,
    pub(crate) built_target_count: usize,
    pub(crate) skipped_target_count: usize,
    pub(crate) body_count: usize,
    pub(crate) scope_count: usize,
    pub(crate) binding_count: usize,
    pub(crate) statement_count: usize,
    pub(crate) expression_count: usize,
}

impl BodyIrReport {
    fn append_fields(&self, fields: &mut ReportFieldsBuilder) {
        fields
            .count_as("target_count", "targets", self.target_count)
            .count_as(
                "built_target_count",
                "built targets",
                self.built_target_count,
            )
            .count_as(
                "skipped_target_count",
                "skipped targets",
                self.skipped_target_count,
            )
            .count_as("body_count", "bodies", self.body_count)
            .count_as("scope_count", "scopes", self.scope_count)
            .count_as("binding_count", "bindings", self.binding_count)
            .count_as("statement_count", "statements", self.statement_count)
            .count_as("expression_count", "expressions", self.expression_count);
    }
}
