use std::fmt;

use rg_project::Project;
use serde::Serialize;

use super::package::PackageReport;

#[derive(Debug, Serialize)]
pub(crate) struct ProjectReport {
    pub(crate) packages: PackageReport,
    pub(crate) def_map: DefMapReport,
    pub(crate) semantic_ir: SemanticIrReport,
    pub(crate) body_ir: BodyIrReport,
}

impl ProjectReport {
    pub(crate) fn capture(project: &Project) -> Self {
        let stats = project.stats();
        Self {
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
}

impl fmt::Display for ProjectReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "rust-glancer analysis built")?;
        writeln!(f, "packages: {}", self.packages)?;
        writeln!(
            f,
            "package residency: {} ({} resident, {} offloaded)",
            self.packages.residency_policy,
            self.packages.resident_count,
            self.packages.offloaded_count,
        )?;
        writeln!(f, "def maps: {}", self.def_map)?;
        writeln!(f, "semantic IR: {}", self.semantic_ir)?;
        write!(f, "body IR: {}", self.body_ir)
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

impl fmt::Display for DefMapReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} targets, {} modules, {} unresolved imports",
            self.target_count, self.module_count, self.unresolved_import_count,
        )
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

impl fmt::Display for SemanticIrReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} targets, {} type defs, {} traits, {} impls, {} functions",
            self.target_count,
            self.type_def_count(),
            self.trait_count,
            self.impl_count,
            self.function_count,
        )
    }
}

impl SemanticIrReport {
    fn type_def_count(&self) -> usize {
        self.struct_count + self.enum_count + self.union_count
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

impl fmt::Display for BodyIrReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} targets ({} built, {} skipped), {} bodies, {} expressions",
            self.target_count,
            self.built_target_count,
            self.skipped_target_count,
            self.body_count,
            self.expression_count,
        )
    }
}
