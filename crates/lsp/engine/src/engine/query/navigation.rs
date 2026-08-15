//! Definition, type-definition, and implementation navigation queries.
//!
//! All three features have the same LSP-side shape: prepare the source file, ask each applicable
//! crate context for semantic targets, convert those targets to locations, and keep the first copy
//! of locations reached through multiple crate roots. `NavigationQuery` only selects the semantic
//! operation inside that shared flow.

use std::{collections::HashMap, path::PathBuf, time::Instant};

use anyhow::Context as _;
use rg_analysis::{
    CurrentSourceView, NavigationTarget, NavigationTargetSource, SavedSourceRelationship,
};
use rg_def_map::PackageSlot;
use rg_ir_model::CrateRef;
use rg_lsp_proto::{
    DocumentQueryCoverage, DocumentQueryResult, EditorDocumentSnapshot, GlobalOperationResult,
    GlobalPositionSnapshot,
};
use rg_parse::FileId;
use rg_project::ProjectSnapshot;
use rg_std::UniqueVec;

use super::{QueryCancellation, QueryRunner};
use crate::proto::navigation as navigation_proto;

impl QueryRunner<'_> {
    pub(crate) fn goto_definition(
        &mut self,
        input: GlobalPositionSnapshot,
        cancellation: &QueryCancellation<'_>,
    ) -> anyhow::Result<DocumentQueryResult<Vec<ls_types::Location>>> {
        self.current_navigation_query(input, CurrentNavigationQuery::Definition, cancellation)
            .context("navigate to definition")
    }

    pub(crate) fn goto_type_definition(
        &mut self,
        input: GlobalPositionSnapshot,
        cancellation: &QueryCancellation<'_>,
    ) -> anyhow::Result<DocumentQueryResult<Vec<ls_types::Location>>> {
        self.current_navigation_query(input, CurrentNavigationQuery::TypeDefinition, cancellation)
            .context("navigate to type definition")
    }

    pub(crate) fn goto_implementation(
        &mut self,
        input: GlobalPositionSnapshot,
    ) -> anyhow::Result<GlobalOperationResult<Vec<ls_types::Location>>> {
        self.saved_implementation_query(input)
            .context("navigate to implementation")
    }

    /// Resolve a definition or type definition using the current body and saved declarations.
    fn current_navigation_query(
        &mut self,
        input: GlobalPositionSnapshot,
        query: CurrentNavigationQuery,
        cancellation: &QueryCancellation<'_>,
    ) -> anyhow::Result<DocumentQueryResult<Vec<ls_types::Location>>> {
        let (target, _, documents, position) = input.into_parts();
        let document = documents
            .iter()
            .find(|document| document.target() == &target)
            .context("target document is absent from navigation input")?;
        let path = document.source_path().to_path_buf();
        let started = Instant::now();
        let snapshot = self
            .project
            .saved_snapshot()
            .context("borrow saved project for navigation")?;
        let Some(current) =
            Self::current_position_analysis(snapshot, document, position, cancellation)
                .context("prepare current navigation analysis")?
        else {
            return Ok(DocumentQueryResult::new(
                Vec::new(),
                DocumentQueryCoverage::Partial,
            ));
        };
        let mut locations = UniqueVec::new();
        let mut every_target_has_exact_source = true;
        let mut omitted_unsafe_target = false;
        let mut destinations = CapturedNavigationDocuments::new(snapshot, &documents);

        for source in &current.targets {
            let has_exact_body = current.target_has_exact_body(source);
            let mut matches_saved = None;
            if !has_exact_body {
                let matches = current
                    .analysis
                    .current_source_relationship(source.context.package, source.context.file)
                    == Some(SavedSourceRelationship::Exact);
                matches_saved = Some(matches);
                if !matches {
                    every_target_has_exact_source = false;
                    continue;
                }
            }
            let targets = match query {
                CurrentNavigationQuery::Definition => current
                    .analysis
                    .goto_definition(source.crate_ref, source.context.file, current.offset)
                    .context("resolve definition targets")?,
                CurrentNavigationQuery::TypeDefinition => current
                    .analysis
                    .goto_type_definition(source.crate_ref, source.context.file, current.offset)
                    .context("resolve type-definition targets")?,
            };

            for target in targets {
                let location = match target.source {
                    NavigationTargetSource::Current => {
                        let same_current_file = target.crate_ref.package == source.context.package
                            && target.file_id == source.context.file;
                        if !same_current_file {
                            // Current Body IR is built only for the request target. A current span
                            // claiming another file would have no captured source to interpret it.
                            omitted_unsafe_target = true;
                            None
                        } else {
                            navigation_proto::location_for_current_document(
                                document.path(),
                                current.source.line_index(),
                                &target,
                            )
                        }
                    }
                    NavigationTargetSource::Saved => match destinations
                        .location_for_saved_target(&target)
                        .context("convert saved navigation target for captured documents")?
                    {
                        CapturedTargetLocation::Ready(location) => Some(location),
                        CapturedTargetLocation::Unsafe => {
                            omitted_unsafe_target = true;
                            None
                        }
                        CapturedTargetLocation::Unavailable => None,
                    },
                };
                if let Some(location) = location {
                    locations.push(location);
                }
            }
            every_target_has_exact_source &= has_exact_body || matches_saved == Some(true);
        }

        let locations = locations.into_vec();

        tracing::trace!(
            query = query.name(),
            path = %path.display(),
            line = position.line,
            character = position.character,
            result_count = locations.len(),
            elapsed_ms = started.elapsed().as_millis(),
            "navigation query finished"
        );

        let coverage = if omitted_unsafe_target {
            DocumentQueryCoverage::Partial
        } else if current.coverage.is_exact() || every_target_has_exact_source {
            DocumentQueryCoverage::Exact
        } else {
            DocumentQueryCoverage::Partial
        };
        Ok(DocumentQueryResult::new(locations, coverage))
    }

    /// Resolve implementations after checking that saved ranges still match all open documents.
    fn saved_implementation_query(
        &mut self,
        input: GlobalPositionSnapshot,
    ) -> anyhow::Result<GlobalOperationResult<Vec<ls_types::Location>>> {
        if let Some(path) = self.save_required_for_global_operation(&input)? {
            return Ok(GlobalOperationResult::save_required(path));
        }
        let document = Self::global_operation_target(&input)?;
        let path = document.source_path().to_path_buf();
        let position = input.position();
        let started = Instant::now();
        self.ensure_path("implementation", &path)
            .context("prepare implementation path")?;
        let snapshot = self
            .project
            .saved_snapshot()
            .context("borrow saved project for implementation navigation")?;
        let crate_offsets = Self::crate_offsets(snapshot, &path, position)
            .context("resolve implementation position")?;
        let analysis_crates = crate_offsets
            .iter()
            .map(|(_, crate_ref, _)| *crate_ref)
            .collect::<Vec<_>>();
        let analysis = snapshot
            .analysis_for_crates(&analysis_crates)
            .context("load implementation analysis")?;
        let mut locations = UniqueVec::new();

        for (context, crate_ref, offset) in crate_offsets {
            for target in analysis
                .goto_implementation(crate_ref, context.file, offset)
                .context("resolve implementation targets")?
            {
                let Some(location) = navigation_proto::location_for_target(snapshot, &target)
                    .context("convert implementation target")?
                else {
                    continue;
                };
                locations.push(location);
            }
        }
        let locations = locations.into_vec();

        tracing::trace!(
            path = %path.display(),
            line = position.line,
            character = position.character,
            result_count = locations.len(),
            elapsed_ms = started.elapsed().as_millis(),
            "implementation query finished"
        );
        Ok(GlobalOperationResult::ready(locations))
    }
}

/// Which current-body navigation query to run through the shared flow above.
#[derive(Debug, Clone, Copy)]
enum CurrentNavigationQuery {
    Definition,
    TypeDefinition,
}

impl CurrentNavigationQuery {
    fn name(self) -> &'static str {
        match self {
            Self::Definition => "definition",
            Self::TypeDefinition => "type_definition",
        }
    }
}

/// Captured source needed to convert saved navigation targets into editor locations.
///
/// Most definition targets are unopened and can use their saved source directly. For an open
/// destination, this value prepares its current line index and compares it with the selected saved
/// project only if a result actually points there. A dirty destination is mapped through the same
/// conservative declaration association used by current-body analysis.
struct CapturedNavigationDocuments<'documents, 'project> {
    snapshot: ProjectSnapshot<'project>,
    documents: &'documents [EditorDocumentSnapshot],
    sources: HashMap<(PackageSlot, FileId), CapturedNavigationSource>,
}

impl<'documents, 'project> CapturedNavigationDocuments<'documents, 'project> {
    fn new(
        snapshot: ProjectSnapshot<'project>,
        documents: &'documents [EditorDocumentSnapshot],
    ) -> Self {
        Self {
            snapshot,
            documents,
            sources: HashMap::new(),
        }
    }

    /// Convert one saved target after proving which source owns its editor range.
    fn location_for_saved_target(
        &mut self,
        target: &NavigationTarget,
    ) -> anyhow::Result<CapturedTargetLocation> {
        let key = (target.crate_ref.package, target.file_id);
        if !self.sources.contains_key(&key) {
            let source = self
                .capture_source(target.crate_ref, target.file_id)
                .context("prepare captured navigation destination")?;
            self.sources.insert(key, source);
        }

        let source = self
            .sources
            .get(&key)
            .expect("captured navigation source should have been inserted");
        match source {
            CapturedNavigationSource::Unopened => Ok(match navigation_proto::location_for_target(
                self.snapshot,
                target,
            )? {
                Some(location) => CapturedTargetLocation::Ready(location),
                None => CapturedTargetLocation::Unavailable,
            }),
            CapturedNavigationSource::Unavailable => Ok(CapturedTargetLocation::Unavailable),
            CapturedNavigationSource::Open(open) => {
                let CapturedOpenNavigationSource { path, source } = open.as_ref();
                let mapped_span = match (
                    target.span,
                    source.relationship(target.crate_ref.package, target.file_id),
                ) {
                    (None, _) => None,
                    (Some(span), Some(SavedSourceRelationship::Exact)) => Some(span),
                    (Some(span), Some(SavedSourceRelationship::Different)) => {
                        let Some(associations) = source
                            .declaration_associations(target.crate_ref.package, target.file_id)
                        else {
                            return Ok(CapturedTargetLocation::Unsafe);
                        };
                        let Some(current) = associations.current_header_span_for_saved(span) else {
                            return Ok(CapturedTargetLocation::Unsafe);
                        };
                        Some(current)
                    }
                    (Some(_), None) => return Ok(CapturedTargetLocation::Unavailable),
                };
                let mut current_target = target.clone();
                current_target.source = NavigationTargetSource::Current;
                current_target.span = mapped_span;
                Ok(
                    match navigation_proto::location_for_current_document(
                        path,
                        source.source().line_index(),
                        &current_target,
                    ) {
                        Some(location) => CapturedTargetLocation::Ready(location),
                        None => CapturedTargetLocation::Unavailable,
                    },
                )
            }
        }
    }

    /// Prepare source conversion state only for a document that receives a result.
    fn capture_source(
        &self,
        crate_ref: CrateRef,
        file: FileId,
    ) -> anyhow::Result<CapturedNavigationSource> {
        let package = crate_ref.package;
        let Some(saved_path) = self.snapshot.file_path(package, file) else {
            return Ok(CapturedNavigationSource::Unavailable);
        };
        let Some(document) = self
            .documents
            .iter()
            .find(|document| document.source_path() == saved_path)
        else {
            return Ok(CapturedNavigationSource::Unopened);
        };
        let source = self
            .snapshot
            .prepare_current_source(&[(crate_ref, file)], document.text())
            .context("prepare open navigation destination source")?;

        Ok(CapturedNavigationSource::Open(Box::new(
            CapturedOpenNavigationSource {
                path: document.path().to_path_buf(),
                source,
            },
        )))
    }
}

enum CapturedNavigationSource {
    /// The editor has no live coordinate space for this saved file.
    Unopened,
    /// The selected project cannot provide the source needed to prove a safe location.
    Unavailable,
    Open(Box<CapturedOpenNavigationSource>),
}

struct CapturedOpenNavigationSource {
    path: PathBuf,
    source: CurrentSourceView,
}

enum CapturedTargetLocation {
    Ready(ls_types::Location),
    Unsafe,
    Unavailable,
}
