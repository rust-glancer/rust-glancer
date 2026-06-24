//! Public-LSP comparison command.
//!
//! This command  compares editor-facing query behavior by driving `rust-glancer lsp` and 
//! a reference server through the same protocol boundary.

mod config;
mod fixture;
mod query;

use std::path::PathBuf;

pub(crate) use self::config::{CliFixture, OutputFormat};
use self::fixture::Fixture;

pub(crate) fn run(
    fixture: CliFixture,
    path: Option<PathBuf>,
    output_format: OutputFormat,
) -> anyhow::Result<()> {
    let fixture = Fixture::resolve(fixture, path)?;

    anyhow::bail!(
        "LSP comparison fixture `{}` resolved to {} with {} query cases, \
         methods: {}. The LSP client/server harness is not implemented yet \
         (format: {output_format:?})",
        fixture.kind(),
        fixture.root().display(),
        fixture.query_cases().len(),
        query_summary(fixture.query_cases()),
    );
}

fn query_summary(query_cases: &[query::QueryCase]) -> String {
    let references = query_cases
        .iter()
        .filter(|query| query.kind().is_references())
        .count();
    let references_with_declaration = query_cases
        .iter()
        .filter_map(|query| query.kind().references_include_declaration())
        .filter(|include_declaration| *include_declaration)
        .count();
    let goto_definition = query_cases
        .iter()
        .filter(|query| query.kind().is_goto_definition())
        .count();
    let hover = query_cases
        .iter()
        .filter(|query| query.kind().is_hover())
        .count();

    format!(
        "references={references} (include_declaration={references_with_declaration}), \
         goto_definition={goto_definition}, hover={hover}",
    )
}
