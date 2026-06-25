//! Fixture summary report section.

use serde::Serialize;

use crate::{
    compare_lsp::{
        fixture::Fixture,
        query::{QueryCase, QueryKind},
    },
    report::{ReportDocumentBuilder, ReportSectionBuilder},
};

#[derive(Debug, Serialize)]
pub(super) struct FixtureReport {
    kind: String,
    root: String,
    query_count: usize,
    opened_files: usize,
    methods: QueryMethodReport,
}

impl FixtureReport {
    pub(super) fn capture(fixture: &Fixture, opened_files: usize) -> Self {
        let query_cases = fixture.query_cases();
        Self {
            kind: fixture.kind().to_string(),
            root: fixture.root().display().to_string(),
            query_count: query_cases.len(),
            opened_files,
            methods: QueryMethodReport::capture(query_cases),
        }
    }

    pub(super) fn append_section(&self, document: ReportDocumentBuilder) -> ReportDocumentBuilder {
        document.section("summary", |section| self.configure_summary_section(section))
    }

    fn configure_summary_section(&self, section: &mut ReportSectionBuilder) {
        section.group("summary", "Summary");
        section.fields("fixture", |fields| {
            fields
                .text("fixture", &self.kind)
                .text("root", &self.root)
                .count_as("query_count", "Query count", self.query_count)
                .count_as("opened_files", "Opened files", self.opened_files)
                .count_as(
                    "references",
                    QueryKind::References {
                        include_declaration: true,
                    }
                    .lsp_method(),
                    self.methods.references,
                )
                .count_as(
                    "references_include_declaration",
                    "textDocument/references includeDeclaration",
                    self.methods.references_include_declaration,
                )
                .count_as(
                    "goto_definition",
                    QueryKind::GotoDefinition.lsp_method(),
                    self.methods.goto_definition,
                )
                .count_as(
                    "type_definition",
                    QueryKind::TypeDefinition.lsp_method(),
                    self.methods.type_definition,
                )
                .count_as(
                    "implementation",
                    QueryKind::Implementation.lsp_method(),
                    self.methods.implementation,
                )
                .count_as(
                    "prepare_rename",
                    QueryKind::PrepareRename.lsp_method(),
                    self.methods.prepare_rename,
                )
                .count_as(
                    "rename",
                    QueryKind::Rename.lsp_method(),
                    self.methods.rename,
                )
                .count_as(
                    "document_highlight",
                    QueryKind::DocumentHighlight.lsp_method(),
                    self.methods.document_highlight,
                )
                .count_as(
                    "document_symbol",
                    QueryKind::DocumentSymbol.lsp_method(),
                    self.methods.document_symbol,
                )
                .count_as(
                    "workspace_symbol",
                    QueryKind::WorkspaceSymbol.lsp_method(),
                    self.methods.workspace_symbol,
                )
                .count_as(
                    "inlay_hint",
                    QueryKind::InlayHint.lsp_method(),
                    self.methods.inlay_hint,
                )
                .count_as("hover", QueryKind::Hover.lsp_method(), self.methods.hover);
        });
    }
}

#[derive(Debug, Serialize)]
struct QueryMethodReport {
    references: usize,
    references_include_declaration: usize,
    goto_definition: usize,
    type_definition: usize,
    implementation: usize,
    prepare_rename: usize,
    rename: usize,
    document_highlight: usize,
    document_symbol: usize,
    workspace_symbol: usize,
    inlay_hint: usize,
    hover: usize,
}

impl QueryMethodReport {
    fn capture(query_cases: &[QueryCase]) -> Self {
        Self {
            references: query_cases
                .iter()
                .filter(|query| query.kind().is_references())
                .count(),
            references_include_declaration: query_cases
                .iter()
                .filter_map(|query| query.kind().references_include_declaration())
                .filter(|include_declaration| *include_declaration)
                .count(),
            goto_definition: query_cases
                .iter()
                .filter(|query| query.kind().is_goto_definition())
                .count(),
            type_definition: query_cases
                .iter()
                .filter(|query| query.kind().is_type_definition())
                .count(),
            implementation: query_cases
                .iter()
                .filter(|query| query.kind().is_implementation())
                .count(),
            prepare_rename: query_cases
                .iter()
                .filter(|query| query.kind().is_prepare_rename())
                .count(),
            rename: query_cases
                .iter()
                .filter(|query| query.kind().is_rename())
                .count(),
            document_highlight: query_cases
                .iter()
                .filter(|query| query.kind().is_document_highlight())
                .count(),
            document_symbol: query_cases
                .iter()
                .filter(|query| query.kind().is_document_symbol())
                .count(),
            workspace_symbol: query_cases
                .iter()
                .filter(|query| query.kind().is_workspace_symbol())
                .count(),
            inlay_hint: query_cases
                .iter()
                .filter(|query| query.kind().is_inlay_hint())
                .count(),
            hover: query_cases
                .iter()
                .filter(|query| query.kind().is_hover())
                .count(),
        }
    }
}
