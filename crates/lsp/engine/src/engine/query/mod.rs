//! Runs editor queries against one saved project.
//!
//! A feature method first finds every crate context in which the target file appears. It loads any
//! saved package data needed by the feature, optionally rebuilds the body around the cursor from
//! the editor text, runs `rg_analysis`, and converts the result to LSP types. Rebuilt bodies belong
//! only to that request; they do not turn the saved project into an unsaved copy.
//!
//! The `lifecycle` module wraps this feature work with cancellation, stale-source recovery, result
//! tagging, and cleanup of data loaded for the request.

mod lifecycle;
mod navigation;
mod references;
mod source;

pub(super) use self::lifecycle::{QueryCancellation, QueryContext};

use std::{path::Path, sync::Arc, time::Instant};

use anyhow::Context as _;
use rg_analysis::{
    Analysis, CompletionQuery, CompletionSource, InlayHint as AnalysisInlayHint,
    SavedSourceRelationship,
};
use rg_lsp_proto::{
    CompletionClientCapabilities, CompletionResult, DocumentPositionSnapshot,
    DocumentQueryCoverage, DocumentQueryResult, DocumentRangeSnapshot, EditorDocumentSnapshot,
    GlobalPositionSnapshot,
};
use rg_parse::{CurrentSource, LineIndex};
use rg_project::{
    AnalysisSurface, CurrentBodyAnalysisCheckpoint, CurrentBodyAnalysisCoverage,
    CurrentBodySelection, FileContext, ProjectSnapshot,
};
use rg_std::UniqueVec;
use rg_text::RustEdition;

use crate::{
    engine::project::ProjectCoordinator,
    memory::MemoryControl,
    proto::{completion, formatting as formatting_proto, hover, inlay_hint, symbols},
};

/// Borrows the engine state used by one dispatched analysis request.
///
/// The runner does not own project generations. It prepares the query's analysis surface, borrows
/// the saved snapshot through `ProjectCoordinator`, and lets the shared lifecycle policy release
/// request-scoped loads before the dispatcher accepts the next command.
pub(super) struct QueryRunner<'a> {
    project: &'a mut ProjectCoordinator,
    memory_control: Arc<dyn MemoryControl>,
}

/// One way the target file appears in the saved project's crate graph.
#[derive(Debug)]
struct CurrentDocumentTarget {
    context: FileContext,
    crate_ref: rg_ir_model::CrateRef,
}

/// Current body, source position, and crate contexts prepared for a document query at one cursor.
struct CurrentPositionAnalysis<'project> {
    analysis: Analysis<'project>,
    coverage: CurrentBodyAnalysisCoverage,
    targets: Vec<CurrentDocumentTarget>,
    source: Arc<CurrentSource>,
    offset: u32,
}

impl CurrentPositionAnalysis<'_> {
    fn target_has_exact_body(&self, target: &CurrentDocumentTarget) -> bool {
        self.coverage
            .exact_body_spans()
            .iter()
            .any(|(crate_ref, file, span)| {
                *crate_ref == target.crate_ref
                    && *file == target.context.file
                    && span.touches(self.offset)
            })
    }
}

impl<'a> QueryRunner<'a> {
    pub(super) fn new(
        project: &'a mut ProjectCoordinator,
        memory_control: Arc<dyn MemoryControl>,
    ) -> Self {
        Self {
            project,
            memory_control,
        }
    }

    /// Give cancellation logs a stable name for each shared current-body build boundary.
    fn current_body_checkpoint(checkpoint: CurrentBodyAnalysisCheckpoint) -> &'static str {
        match checkpoint {
            CurrentBodyAnalysisCheckpoint::SourceParsed => "after current source parsing",
            CurrentBodyAnalysisCheckpoint::OwnerAssociated => {
                "after current body owner association"
            }
            CurrentBodyAnalysisCheckpoint::BodyLowered => "after current body lowering",
            CurrentBodyAnalysisCheckpoint::BodyLocalItemsCollected => {
                "after current body-local item collection"
            }
            CurrentBodyAnalysisCheckpoint::ImplHeadersResolved => {
                "after current body-local impl header resolution"
            }
            CurrentBodyAnalysisCheckpoint::PatternBindingsMaterialized => {
                "after current pattern binding resolution"
            }
            CurrentBodyAnalysisCheckpoint::BodyResolved => "after current body resolution",
        }
    }

    /// Find the target document inside a global operation's captured document set.
    fn global_operation_target(
        snapshot: &GlobalPositionSnapshot,
    ) -> anyhow::Result<&EditorDocumentSnapshot> {
        snapshot
            .target_document()
            .context("target document is absent from the global-operation input")
    }

    /// Return the first open document that does not match this saved project.
    ///
    /// References, rename, and implementation navigation return saved byte ranges. They can use
    /// those ranges only when every applicable open Rust document still has its saved text. A
    /// document that is not known to the saved project also requires a save.
    ///
    /// TODO: Body-local references and rename may eventually use exact current-body spans. Until
    /// that feature has one complete edit policy, keeping all global operations behind this rule
    /// is safer and easier to predict than mixing current and saved spans in one response.
    fn save_required_for_global_operation(
        &self,
        input: &GlobalPositionSnapshot,
    ) -> anyhow::Result<Option<std::path::PathBuf>> {
        let snapshot = self
            .project
            .saved_snapshot()
            .context("borrow saved project for global-operation safety")?;

        for document in input.documents() {
            let contexts = Self::file_contexts(snapshot, document.source_path())
                .context("resolve open document for global-operation safety")?;
            if contexts.is_empty() {
                return Ok(Some(document.path().to_path_buf()));
            }
            for context in contexts {
                let saved = snapshot
                    .file_source_text(context.package, context.file)
                    .context("load saved source for global-operation safety")?;
                if !saved.is_some_and(|source| source.as_ref() == document.text()) {
                    return Ok(Some(document.path().to_path_buf()));
                }
            }
        }

        Ok(None)
    }

    /// Prepare current-body analysis for one editor position.
    ///
    /// This converts the LSP position using the captured text, finds every crate context for the
    /// file, and rebuilds the matching body in each context. The returned analysis still borrows
    /// all module-level information from the saved project.
    fn current_position_analysis<'project>(
        snapshot: ProjectSnapshot<'project>,
        document: &EditorDocumentSnapshot,
        position: ls_types::Position,
        cancellation: &QueryCancellation<'_>,
    ) -> anyhow::Result<Option<CurrentPositionAnalysis<'project>>> {
        let targets = Self::file_contexts(snapshot, document.source_path())
            .context("resolve current document contexts")?
            .into_iter()
            .flat_map(|context| {
                context
                    .crates
                    .clone()
                    .into_iter()
                    .map(move |crate_ref| CurrentDocumentTarget {
                        context: context.clone(),
                        crate_ref,
                    })
            })
            .collect::<Vec<_>>();
        if targets.is_empty() {
            return Ok(None);
        }
        let body_targets = targets
            .iter()
            .map(|target| (target.crate_ref, target.context.file))
            .collect::<Vec<_>>();
        let source_view = snapshot
            .prepare_current_source(&body_targets, document.text())
            .context("prepare current document source")?;
        let source = source_view.shared_source();
        let Some(offset) = source
            .line_index()
            .offset_from_utf16_position(crate::proto::position::parse_position(position))
        else {
            return Ok(None);
        };
        let (analysis, coverage) = snapshot
            .analysis_for_current_bodies_from_source(
                &body_targets,
                source_view,
                CurrentBodySelection::AtOffset(offset),
                |checkpoint| cancellation.checkpoint(Self::current_body_checkpoint(checkpoint)),
            )
            .context("build current document body analysis")?;

        Ok(Some(CurrentPositionAnalysis {
            analysis,
            coverage,
            targets,
            source,
            offset,
        }))
    }

    /// Load deferred package data for every saved file identity of one source path.
    ///
    /// A path can appear in several crate roots. Loading is selected by package and file, so this
    /// first collects those pairs. The feature query expands them into crate contexts afterward.
    fn ensure_path(&mut self, query: &'static str, path: &Path) -> anyhow::Result<()> {
        let started = Instant::now();

        // Resolve the path before mutating the project. One file can have several crate contexts,
        // but file-shaped materialization only needs package-local file identities.
        let files = {
            let snapshot = self
                .project
                .saved_snapshot()
                .context("borrow saved project")?;
            Self::file_contexts(snapshot, path)
                .context("resolve query path")?
                .into_iter()
                .map(|context| (context.package, context.file))
                .collect::<Vec<_>>()
        };
        self.project
            .materialize_saved_project(AnalysisSurface::Files(&files))
            .with_context(|| format!("prepare {query} query path"))?;
        tracing::trace!(
            query,
            file_count = files.len(),
            elapsed_ms = started.elapsed().as_millis(),
            "analysis surface prepared for query path"
        );
        Ok(())
    }

    /// Compute completions from current syntax and body facts plus saved crate-level information.
    pub(super) fn completion(
        &mut self,
        input: DocumentPositionSnapshot,
        client_capabilities: CompletionClientCapabilities,
        cancellation: &QueryCancellation<'_>,
    ) -> anyhow::Result<CompletionResult> {
        let document = input.document();
        let position = input.position();
        let path = document.source_path().to_path_buf();
        let source_text = document.text();
        let started = Instant::now();
        let snapshot_query_started = Instant::now();
        let snapshot = self
            .project
            .saved_snapshot()
            .context("borrow saved project for completion")?;
        let contexts_started = Instant::now();
        let targets = Self::file_contexts(snapshot, &path)
            .context("resolve completion file contexts")?
            .into_iter()
            .flat_map(|context| {
                context
                    .crates
                    .into_iter()
                    .map(move |crate_ref| (crate_ref, context.file))
            })
            .collect::<Vec<_>>();
        let contexts_us = contexts_started.elapsed().as_micros();
        if targets.is_empty() {
            return Ok(CompletionResult::new(
                Vec::new(),
                DocumentQueryCoverage::Partial,
            ));
        }

        let current_source_started = Instant::now();
        let current_source_view = snapshot
            .prepare_current_source(&targets, source_text)
            .context("prepare current completion source")?;
        let current_source = current_source_view.shared_source();
        let Some(offset) = current_source
            .line_index()
            .offset_from_utf16_position(crate::proto::position::parse_position(position))
        else {
            return Ok(CompletionResult::new(
                Vec::new(),
                DocumentQueryCoverage::Partial,
            ));
        };
        let current_source_us = current_source_started.elapsed().as_micros();
        let completion_source_started = Instant::now();
        let completion_source = CompletionSource::new(source_text, offset);
        let completion_source_us = completion_source_started.elapsed().as_micros();

        cancellation
            .checkpoint("after current completion source parsing")
            .context("check cancellation after current completion source parsing")?;

        let current_body_started = Instant::now();
        let (analysis, coverage) = snapshot
            .analysis_for_current_bodies_from_source(
                &targets,
                current_source_view,
                CurrentBodySelection::AtOffset(offset),
                |checkpoint| cancellation.checkpoint(Self::current_body_checkpoint(checkpoint)),
            )
            .context("build current completion analysis")?;
        let current_body_us = current_body_started.elapsed().as_micros();
        let semantic_coverage = if coverage.is_exact() {
            DocumentQueryCoverage::Exact
        } else {
            DocumentQueryCoverage::Partial
        };

        cancellation
            .checkpoint("before semantic completion")
            .context("check cancellation before semantic completion")?;

        let mut completions = UniqueVec::new();
        let mut analysis_compute_us = 0_u128;
        let mut protocol_conversion_us = 0_u128;
        for &(crate_ref, file) in &targets {
            cancellation
                .checkpoint("before completion crate interpretation")
                .context("check cancellation before completion crate interpretation")?;
            let mut query = CompletionQuery::new(crate_ref, file, offset).with_client_capabilities(
                rg_analysis::CompletionClientCapabilities {
                    snippet_support: client_capabilities.snippet_support,
                },
            );
            query = match completion_source.as_ref() {
                Some(source) => query.with_completion_source(source),
                None => query.with_source_text(source_text),
            };
            let analysis_compute_started = Instant::now();
            let items = analysis
                .completions_at(query)
                .context("compute completions")?;
            analysis_compute_us += analysis_compute_started.elapsed().as_micros();

            // A crate query is synchronous. If it was overtaken, discard its items before
            // conversion and before starting another crate interpretation.
            cancellation
                .checkpoint("after completion crate interpretation")
                .context("check cancellation after completion crate interpretation")?;
            let protocol_conversion_started = Instant::now();
            for item in items {
                completions.push(completion::completion_item(
                    item,
                    current_source.line_index(),
                ));
            }
            protocol_conversion_us += protocol_conversion_started.elapsed().as_micros();
        }

        let completions = completions.into_vec();
        let analysis_drop_started = Instant::now();
        drop(analysis);
        let analysis_drop_us = analysis_drop_started.elapsed().as_micros();
        tracing::trace!(
            crate_count = targets.len(),
            result_count = completions.len(),
            coverage = ?semantic_coverage,
            current_source_us,
            completion_source_us,
            contexts_us,
            current_body_us,
            analysis_compute_us,
            protocol_conversion_us,
            analysis_drop_us,
            snapshot_query_us = snapshot_query_started.elapsed().as_micros(),
            "completion saved-semantics query phases finished"
        );

        tracing::trace!(
            path = %path.display(),
            line = position.line,
            character = position.character,
            result_count = completions.len(),
            coverage = ?semantic_coverage,
            elapsed_ms = started.elapsed().as_millis(),
            "completion query finished"
        );

        Ok(CompletionResult::new(completions, semantic_coverage))
    }

    /// Return the first usable hover from the path's possible crate contexts.
    pub(super) fn hover(
        &mut self,
        input: DocumentPositionSnapshot,
        cancellation: &QueryCancellation<'_>,
    ) -> anyhow::Result<DocumentQueryResult<Option<ls_types::Hover>>> {
        let (document, position) = input.into_parts();
        let path = document.source_path().to_path_buf();
        let started = Instant::now();
        let snapshot = self
            .project
            .saved_snapshot()
            .context("borrow saved project for hover")?;
        let Some(current) =
            Self::current_position_analysis(snapshot, &document, position, cancellation)
                .context("prepare hover analysis")?
        else {
            return Ok(DocumentQueryResult::new(
                None,
                DocumentQueryCoverage::Partial,
            ));
        };

        let mut hover = None;
        let mut every_target_has_exact_source = true;
        for target in &current.targets {
            let has_exact_body = current.target_has_exact_body(target);
            let matches_saved = if has_exact_body {
                false
            } else {
                current
                    .analysis
                    .current_source_relationship(target.context.package, target.context.file)
                    == Some(SavedSourceRelationship::Exact)
            };
            every_target_has_exact_source &= has_exact_body || matches_saved;
            let info = current
                .analysis
                .hover(target.crate_ref, target.context.file, current.offset)
                .context("compute hover")?;
            let Some(info) = info else {
                continue;
            };
            let Some(value) = hover::hover(info, current.source.line_index()) else {
                continue;
            };
            hover = Some(value);
            break;
        }

        tracing::trace!(
            path = %path.display(),
            line = position.line,
            character = position.character,
            has_hover = hover.is_some(),
            exact_body = current.coverage.is_exact(),
            elapsed_ms = started.elapsed().as_millis(),
            "hover query finished"
        );
        let coverage = if current.coverage.is_exact() || every_target_has_exact_source {
            DocumentQueryCoverage::Exact
        } else {
            DocumentQueryCoverage::Partial
        };
        Ok(DocumentQueryResult::new(hover, coverage))
    }

    /// Build the document outline directly from the syntax shown by the editor.
    pub(super) fn document_symbol(
        &mut self,
        document: EditorDocumentSnapshot,
    ) -> anyhow::Result<DocumentQueryResult<Vec<ls_types::DocumentSymbol>>> {
        let path = document.source_path().to_path_buf();
        let started = Instant::now();
        let snapshot = self
            .project
            .saved_snapshot()
            .context("borrow saved project for document symbols")?;
        let contexts =
            Self::file_contexts(snapshot, &path).context("resolve document-symbol path")?;
        // The saved package is needed only for its Rust edition. A standalone Rust file may have
        // no package, so parse it with the newest supported edition and still return its outline.
        let edition = contexts
            .first()
            .and_then(|context| snapshot.package_edition(context.package))
            .unwrap_or(RustEdition::Edition2024);
        let syntax = rg_parse::parse_source_file(document.text(), edition).tree();
        let line_index = LineIndex::new(document.text());
        let lsp_symbols = Analysis::document_symbols_from_syntax(&syntax)
            .into_iter()
            .map(|symbol| symbols::document_symbol(&line_index, symbol))
            .collect::<Vec<_>>();

        tracing::trace!(
            path = %path.display(),
            result_count = lsp_symbols.len(),
            elapsed_ms = started.elapsed().as_millis(),
            "document symbol query finished"
        );

        Ok(DocumentQueryResult::new(
            lsp_symbols,
            DocumentQueryCoverage::Exact,
        ))
    }

    /// Format the live editor text using the owning package's Rust edition.
    ///
    /// Formatting does not need semantic materialization. The saved project is consulted only for
    /// edition metadata, and documents outside known packages use the newest supported edition.
    pub(super) fn formatting(
        &mut self,
        document: EditorDocumentSnapshot,
    ) -> anyhow::Result<DocumentQueryResult<Option<Vec<ls_types::TextEdit>>>> {
        let path = document.source_path().to_path_buf();
        let text = document.text();
        let started = Instant::now();
        let edition = {
            let snapshot = self
                .project
                .saved_snapshot()
                .context("borrow saved project for formatting")?;
            let contexts =
                Self::file_contexts(snapshot, &path).context("resolve formatting path")?;

            // Some routed documents may not map to package metadata. We use an explicit fallback
            // here so formatting can still run without reading Cargo.toml from disk.
            contexts
                .first()
                .and_then(|context| snapshot.package_edition(context.package))
                .unwrap_or(RustEdition::Edition2024)
        };
        let formatted_text =
            crate::formatting::rustfmt(text, edition).context("format Rust source")?;
        let edits = formatting_proto::document_edits(text, formatted_text)
            .context("build formatting edits")?;

        tracing::trace!(
            path = %path.display(),
            edition = %edition,
            edit_count = edits.len(),
            elapsed_ms = started.elapsed().as_millis(),
            "formatting query finished"
        );

        Ok(DocumentQueryResult::new(
            Some(edits),
            DocumentQueryCoverage::Exact,
        ))
    }

    /// Merge inlay hints from every crate context covering the requested source range.
    pub(super) fn inlay_hint(
        &mut self,
        input: DocumentRangeSnapshot,
        cancellation: &QueryCancellation<'_>,
    ) -> anyhow::Result<DocumentQueryResult<Vec<ls_types::InlayHint>>> {
        let (document, range) = input.into_parts();
        let path = document.source_path().to_path_buf();
        let started = Instant::now();
        let snapshot = self
            .project
            .saved_snapshot()
            .context("borrow saved project for inlay hints")?;
        let targets = Self::file_contexts(snapshot, &path)
            .context("resolve inlay-hint path")?
            .into_iter()
            .flat_map(|context| {
                context
                    .crates
                    .into_iter()
                    .map(move |crate_ref| (crate_ref, context.file))
            })
            .collect::<Vec<_>>();
        if targets.is_empty() {
            return Ok(DocumentQueryResult::new(
                Vec::new(),
                DocumentQueryCoverage::Partial,
            ));
        }
        let current_source_view = snapshot
            .prepare_current_source(&targets, document.text())
            .context("prepare current source for inlay hints")?;
        let current_source = current_source_view.shared_source();
        let Some(range_start) = current_source
            .line_index()
            .offset_from_utf16_position(crate::proto::position::parse_position(range.start))
        else {
            return Ok(DocumentQueryResult::new(
                Vec::new(),
                DocumentQueryCoverage::Partial,
            ));
        };
        let Some(range_end) = current_source
            .line_index()
            .offset_from_utf16_position(crate::proto::position::parse_position(range.end))
        else {
            return Ok(DocumentQueryResult::new(
                Vec::new(),
                DocumentQueryCoverage::Partial,
            ));
        };
        let text_range = rg_parse::TextSpan {
            start: range_start,
            end: range_end,
        };
        let (analysis, coverage) = snapshot
            .analysis_for_current_bodies_from_source(
                &targets,
                current_source_view,
                CurrentBodySelection::IntersectingRange(text_range),
                |checkpoint| cancellation.checkpoint(Self::current_body_checkpoint(checkpoint)),
            )
            .context("build current bodies for inlay hints")?;
        let mut hints = UniqueVec::<AnalysisInlayHint>::new();
        for &(crate_ref, file) in &targets {
            cancellation
                .checkpoint("before inlay hint crate interpretation")
                .context("check cancellation before inlay hint crate interpretation")?;
            hints.extend(
                analysis
                    .inlay_hints(crate_ref, file, Some(text_range))
                    .context("compute current body inlay hints")?,
            );
        }
        let lsp_hints = hints
            .into_iter()
            .map(|hint| inlay_hint::inlay_hint_with_line_index(current_source.line_index(), hint))
            .collect::<Vec<_>>();

        tracing::trace!(
            path = %path.display(),
            result_count = lsp_hints.len(),
            exact_bodies = coverage.is_exact(),
            elapsed_ms = started.elapsed().as_millis(),
            "inlay hint query finished"
        );

        let semantic_coverage = if coverage.is_exact() {
            DocumentQueryCoverage::Exact
        } else {
            DocumentQueryCoverage::Partial
        };
        Ok(DocumentQueryResult::new(lsp_hints, semantic_coverage))
    }

    /// Search the saved workspace index without loading source files for a document path.
    pub(super) fn workspace_symbol(
        &mut self,
        query: &str,
    ) -> anyhow::Result<Vec<ls_types::WorkspaceSymbol>> {
        let started = Instant::now();
        let lsp_symbols = self
            .project
            .saved_snapshot()
            .and_then(|snapshot| {
                let analysis = snapshot
                    .full_analysis()
                    .context("load workspace-symbol analysis")?;
                let mut lsp_symbols = UniqueVec::new();

                for symbol in analysis
                    .workspace_symbols(query)
                    .context("search workspace symbols")?
                {
                    let Some(symbol) = symbols::workspace_symbol(snapshot, symbol)
                        .context("convert workspace symbol")?
                    else {
                        continue;
                    };
                    lsp_symbols.push(symbol);
                }
                Ok(lsp_symbols.into_vec())
            })
            .context("run workspace-symbol query")?;

        tracing::trace!(
            query,
            result_count = lsp_symbols.len(),
            elapsed_ms = started.elapsed().as_millis(),
            "workspace symbol query finished"
        );

        Ok(lsp_symbols)
    }
}
