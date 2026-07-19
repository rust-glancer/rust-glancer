//! Definition, type-definition, and implementation navigation queries.
//!
//! All three features have the same LSP-side shape: prepare the source file, ask each applicable
//! crate context for semantic targets, convert those targets to locations, and keep the first copy
//! of locations reached through multiple crate roots. `NavigationQuery` only selects the semantic
//! operation inside that shared flow.

use std::{path::PathBuf, time::Instant};

use anyhow::Context as _;
use rg_std::UniqueVec;

use super::QueryRunner;
use crate::{documents::DirtyDocumentSnapshot, proto::navigation as navigation_proto};

impl QueryRunner<'_> {
    pub(crate) fn goto_definition(
        &mut self,
        path: PathBuf,
        position: ls_types::Position,
        dirty: Option<DirtyDocumentSnapshot>,
    ) -> anyhow::Result<Vec<ls_types::Location>> {
        self.navigation_query(path, position, NavigationQuery::Definition, dirty)
            .context("navigate to definition")
    }

    pub(crate) fn goto_type_definition(
        &mut self,
        path: PathBuf,
        position: ls_types::Position,
        dirty: Option<DirtyDocumentSnapshot>,
    ) -> anyhow::Result<Vec<ls_types::Location>> {
        self.navigation_query(path, position, NavigationQuery::TypeDefinition, dirty)
            .context("navigate to type definition")
    }

    pub(crate) fn goto_implementation(
        &mut self,
        path: PathBuf,
        position: ls_types::Position,
        dirty: Option<DirtyDocumentSnapshot>,
    ) -> anyhow::Result<Vec<ls_types::Location>> {
        self.navigation_query(path, position, NavigationQuery::Implementation, dirty)
            .context("navigate to implementation")
    }

    /// Run the common multi-crate navigation flow selected by `query`.
    fn navigation_query(
        &mut self,
        path: PathBuf,
        position: ls_types::Position,
        query: NavigationQuery,
        dirty: Option<DirtyDocumentSnapshot>,
    ) -> anyhow::Result<Vec<ls_types::Location>> {
        let started = Instant::now();
        self.ensure_path(query.name(), &path)
            .context("prepare navigation path")?;
        let locations = self
            .project
            .with_query_snapshot(dirty.as_ref(), |snapshot| {
                let crate_offsets = Self::crate_offsets(snapshot, &path, position)
                    .context("resolve navigation position")?;
                let analysis_crates = crate_offsets
                    .iter()
                    .map(|(_, crate_ref, _)| *crate_ref)
                    .collect::<Vec<_>>();
                let analysis = snapshot
                    .analysis_for_crates(&analysis_crates)
                    .context("load navigation analysis")?;
                let mut locations = UniqueVec::new();

                for (context, crate_ref, offset) in crate_offsets {
                    let targets = match query {
                        NavigationQuery::Definition => analysis
                            .goto_definition(crate_ref, context.file, offset)
                            .context("resolve definition targets")?,
                        NavigationQuery::TypeDefinition => analysis
                            .goto_type_definition(crate_ref, context.file, offset)
                            .context("resolve type-definition targets")?,
                        NavigationQuery::Implementation => analysis
                            .goto_implementation(crate_ref, context.file, offset)
                            .context("resolve implementation targets")?,
                    };

                    for target in targets {
                        let Some(location) =
                            navigation_proto::location_for_target(snapshot, &target)
                                .context("convert navigation target")?
                        else {
                            continue;
                        };
                        locations.push(location);
                    }
                }

                Ok(locations.into_vec())
            })
            .context("run navigation query")?;

        tracing::trace!(
            query = query.name(),
            path = %path.display(),
            line = position.line,
            character = position.character,
            result_count = locations.len(),
            elapsed_ms = started.elapsed().as_millis(),
            "navigation query finished"
        );

        Ok(locations)
    }
}

/// Semantic target kind selected by one of the three protocol methods.
#[derive(Debug, Clone, Copy)]
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
