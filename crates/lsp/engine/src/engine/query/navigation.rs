use gen_lsp_types::Location;
use rg_lsp_proto::DocumentPositionSnapshot;
use rg_std::UniqueVec;

use super::{DocumentSelection, QueryCancellation, QueryRunner};
use crate::proto::navigation::SourceLocations;

impl QueryRunner<'_> {
    pub(crate) fn goto_definition(
        &mut self,
        input: DocumentPositionSnapshot,
        cancellation: &QueryCancellation<'_>,
    ) -> anyhow::Result<Vec<Location>> {
        self.navigation(input, NavigationQuery::Definition, cancellation)
    }

    pub(crate) fn goto_type_definition(
        &mut self,
        input: DocumentPositionSnapshot,
        cancellation: &QueryCancellation<'_>,
    ) -> anyhow::Result<Vec<Location>> {
        self.navigation(input, NavigationQuery::TypeDefinition, cancellation)
    }

    pub(crate) fn goto_implementation(
        &mut self,
        input: DocumentPositionSnapshot,
        cancellation: &QueryCancellation<'_>,
    ) -> anyhow::Result<Vec<Location>> {
        self.navigation(input, NavigationQuery::Implementation, cancellation)
    }

    fn navigation(
        &self,
        input: DocumentPositionSnapshot,
        query: NavigationQuery,
        cancellation: &QueryCancellation<'_>,
    ) -> anyhow::Result<Vec<Location>> {
        self.with_document_analysis(
            query.name(),
            input.snapshot(),
            DocumentSelection::Position(input.position()),
            cancellation,
            |request| {
                let Some(request) = request else {
                    return Ok(Vec::new());
                };
                let source = request.target;
                let offset = request.selection.start;
                let targets = match query {
                    NavigationQuery::Definition => {
                        request
                            .analysis
                            .goto_definition(source.crate_ref, source.file, offset)?
                    }
                    NavigationQuery::TypeDefinition => request.analysis.goto_type_definition(
                        source.crate_ref,
                        source.file,
                        offset,
                    )?,
                    NavigationQuery::Implementation => request.analysis.goto_implementation(
                        source.crate_ref,
                        source.file,
                        offset,
                    )?,
                };
                let sources = SourceLocations::new(request.snapshot, input.snapshot().editor());
                // Shared declarations and macro expansions can project to the same physical range.
                let mut locations = UniqueVec::new();
                for target in targets {
                    rg_std::check_cancel!(cancellation, "navigation locations");
                    if let Some(location) = sources.for_target(&target)? {
                        locations.push(location);
                    }
                }
                Ok(locations.into_vec())
            },
        )
    }
}

#[derive(Clone, Copy)]
enum NavigationQuery {
    Definition,
    TypeDefinition,
    Implementation,
}

impl NavigationQuery {
    fn name(self) -> &'static str {
        match self {
            Self::Definition => "definition",
            Self::TypeDefinition => "type_definition",
            Self::Implementation => "implementation",
        }
    }
}
