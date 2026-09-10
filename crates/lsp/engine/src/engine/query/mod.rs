//! Runs editor queries against one saved project.
//!
//! A feature method first finds every crate context in which the target file appears. It loads any
//! saved package data needed by the feature, prepares selected bodies and declarations from the
//! editor text, runs `rg_analysis`, and converts the result to LSP types. Current semantics belong
//! only to that request; they do not turn the saved project into an unsaved copy.
//!
//! The `lifecycle` module wraps this feature work with cancellation, stale-source recovery, result
//! tagging, and cleanup of data loaded for the request.

mod lifecycle;
mod navigation;
mod references;
mod semantic_tokens;
mod source;

use self::lifecycle::QueryRunError;
pub(super) use self::lifecycle::{QueryCancellation, QueryContext};
use self::navigation::{CapturedNavigationDocuments, CapturedTargetLocation};

use std::{path::Path, sync::Arc, time::Instant};

use anyhow::Context as _;
use rg_analysis::{
    Analysis, CodeActionKinds, CodeActionQuery, CodeActionTrigger, CompletionQuery,
    CompletionSource, InlayHint as AnalysisInlayHint,
};
use rg_ir_model::TextSpan;
use rg_lsp_proto::{
    CodeActionRequestContext, CodeActionRequestTrigger, CompletionClientCapabilities,
    DocumentPositionSnapshot, DocumentRangeSnapshot, EditorDocumentSnapshot,
    FoldingClientCapabilities, GlobalPositionSnapshot,
};
use rg_parse::{CurrentSource, LineIndex};
use rg_project::{
    AnalysisSurface, CurrentSourceBuildCheckpoint, CurrentSourceSelection, DocumentSourceView,
    FileContext, ProjectSnapshot,
};
use rg_std::UniqueVec;

use crate::{
    engine::project::ProjectCoordinator,
    memory::MemoryControl,
    proto::{
        code_action, completion, folding as folding_proto, formatting as formatting_proto, hover,
        inlay_hint, symbols,
    },
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
struct DocumentTarget {
    context: FileContext,
    crate_ref: rg_ir_model::CrateRef,
}

/// Editor coordinates used to choose the current bodies needed by one query.
enum DocumentSelection {
    Position(gen_lsp_types::Position),
    Range(gen_lsp_types::Range),
}

impl DocumentSelection {
    fn to_current_source_selection(
        &self,
        line_index: &LineIndex,
    ) -> Option<CurrentSourceSelection> {
        match self {
            Self::Position(position) => line_index
                .offset_from_utf16_position(crate::proto::position::parse_position(*position))
                .map(CurrentSourceSelection::AtOffset),
            Self::Range(range) => {
                let start = line_index.offset_from_utf16_position(
                    crate::proto::position::parse_position(range.start),
                )?;
                let end = line_index.offset_from_utf16_position(
                    crate::proto::position::parse_position(range.end),
                )?;
                Some(CurrentSourceSelection::IntersectingRange(TextSpan {
                    start,
                    end,
                }))
            }
        }
    }
}

/// Source coordinates retained after document analysis has been prepared.
///
/// `DocumentSourceView::Current` is consumed when it is attached to `Analysis`. The response still
/// needs the captured line index, so this smaller value keeps exactly what protocol conversion
/// needs. It does not make another saved/current source decision.
enum DocumentAnalysisSource {
    /// The captured text is identical to every saved file interpretation.
    SavedExact(LineIndex),
    /// The captured text differs, so matching bodies were rebuilt for this request.
    Current(Arc<CurrentSource>),
}

impl DocumentAnalysisSource {
    fn line_index(&self) -> &LineIndex {
        match self {
            Self::SavedExact(line_index) => line_index,
            Self::Current(source) => source.line_index(),
        }
    }

    fn name(&self) -> &'static str {
        match self {
            Self::SavedExact(_) => "saved_exact",
            Self::Current(_) => "current",
        }
    }
}

/// Analysis, source selection, and crate contexts prepared for one document query.
struct DocumentAnalysis<'project> {
    snapshot: ProjectSnapshot<'project>,
    analysis: Analysis<'project>,
    targets: Vec<DocumentTarget>,
    source: DocumentAnalysisSource,
    selection: CurrentSourceSelection,
}

impl DocumentAnalysis<'_> {
    fn offset(&self) -> u32 {
        match self.selection {
            CurrentSourceSelection::AtOffset(offset) => offset,
            CurrentSourceSelection::IntersectingRange(_) => {
                unreachable!("position query should retain a cursor selection")
            }
        }
    }

    fn range(&self) -> TextSpan {
        match self.selection {
            CurrentSourceSelection::IntersectingRange(range) => range,
            CurrentSourceSelection::AtOffset(_) => {
                unreachable!("range query should retain a range selection")
            }
        }
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

    /// Give cancellation logs a stable name for each shared current-source preparation boundary.
    fn current_source_checkpoint(checkpoint: CurrentSourceBuildCheckpoint) -> &'static str {
        match checkpoint {
            CurrentSourceBuildCheckpoint::DeclarationsPrepared => {
                "after current declaration preparation"
            }
            CurrentSourceBuildCheckpoint::SourceParsed => "after current source parsing",
            CurrentSourceBuildCheckpoint::OwnerAssociated => "after current body owner association",
            CurrentSourceBuildCheckpoint::BodyLowered => "after current body lowering",
            CurrentSourceBuildCheckpoint::BodyLocalItemsCollected => {
                "after current body-local item collection"
            }
            CurrentSourceBuildCheckpoint::ImplHeadersResolved => {
                "after current body-local impl header resolution"
            }
            CurrentSourceBuildCheckpoint::PatternBindingsMaterialized => {
                "after current pattern binding resolution"
            }
            CurrentSourceBuildCheckpoint::BodyResolved => "after current body resolution",
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
        cancellation: &QueryCancellation<'_>,
    ) -> anyhow::Result<Option<std::path::PathBuf>> {
        let snapshot = self
            .project
            .saved_snapshot()
            .context("borrow saved project for global-operation safety")?;

        for document in input.documents() {
            rg_std::check_cancel!(cancellation, "open document safety");
            let contexts = Self::file_contexts(snapshot, document.source_path())
                .context("resolve open document for global-operation safety")?;
            if contexts.is_empty() {
                return Ok(Some(document.path().to_path_buf()));
            }
            for context in contexts {
                rg_std::check_cancel!(cancellation, "saved source safety");
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

    /// Prepare saved or current-source analysis for one editor source selection.
    ///
    /// Exact captured text can use the saved line index and Body IR directly. Changed text follows
    /// the request-local body path. Cursor and range queries share this source decision but retain
    /// their different current-body selection rules.
    fn document_analysis<'project>(
        &'project mut self,
        query: &'static str,
        document: &EditorDocumentSnapshot,
        selection: DocumentSelection,
        cancellation: &QueryCancellation<'_>,
    ) -> anyhow::Result<Option<DocumentAnalysis<'project>>> {
        let started = Instant::now();

        // Resolve every saved interpretation, then choose its source coordinate space once. Exact
        // text returns before current syntax or declaration associations are built.
        let (targets, source_targets, source, source_selection) = {
            let snapshot = self
                .project
                .saved_snapshot()
                .context("borrow saved project for document analysis")?;
            let targets = Self::file_contexts(snapshot, document.source_path())
                .context("resolve document contexts")?
                .into_iter()
                .flat_map(|context| {
                    context
                        .crates
                        .clone()
                        .into_iter()
                        .map(move |crate_ref| DocumentTarget {
                            context: context.clone(),
                            crate_ref,
                        })
                })
                .collect::<Vec<_>>();
            if targets.is_empty() {
                return Ok(None);
            }
            let source_targets = targets
                .iter()
                .map(|target| (target.crate_ref, target.context.file))
                .collect::<Vec<_>>();
            let source = snapshot
                .prepare_document_source(&source_targets, document.text(), &cancellation.token())
                .context("prepare document source")?;
            let Some(source_selection) = selection.to_current_source_selection(source.line_index())
            else {
                return Ok(None);
            };
            (targets, source_targets, source, source_selection)
        };

        let prepared = match source {
            DocumentSourceView::SavedExact(line_index) => {
                // Exact text needs the ordinary saved bodies for this file. They are normally
                // resident or lazy-loadable; during early startup this may finish the file's
                // deferred Body IR before borrowing the read-only analysis snapshot.
                let files = targets
                    .iter()
                    .map(|target| (target.crate_ref, target.context.file))
                    .collect::<UniqueVec<_>>();
                self.project
                    .materialize_saved_project(
                        AnalysisSurface::Files(files.as_slice()),
                        &cancellation.token(),
                    )
                    .context("prepare exact saved document analysis")?;
                rg_std::check_cancel!(cancellation, "after exact saved document preparation");

                let snapshot = self
                    .project
                    .saved_snapshot()
                    .context("borrow exact saved document analysis")?;
                let crates = targets
                    .iter()
                    .map(|target| target.crate_ref)
                    .collect::<UniqueVec<_>>();
                let analysis = snapshot
                    .analysis_for_crates(crates.as_slice(), cancellation.token())
                    .context("load exact saved document analysis")?;
                DocumentAnalysis {
                    snapshot,
                    analysis,
                    targets,
                    source: DocumentAnalysisSource::SavedExact(line_index),
                    selection: source_selection,
                }
            }
            DocumentSourceView::Current(source_view) => {
                // Different text needs syntax and Body IR owned only by this request.
                let snapshot = self
                    .project
                    .saved_snapshot()
                    .context("borrow saved project for current document")?;
                let source = source_view.shared_source();
                let (analysis, build_summary) = snapshot
                    .analysis_for_current_source(
                        &source_targets,
                        source_view,
                        source_selection,
                        cancellation.token(),
                        |checkpoint| {
                            rg_std::check_cancel!(
                                cancellation,
                                Self::current_source_checkpoint(checkpoint)
                            );
                            Ok(())
                        },
                    )
                    .context("prepare current document analysis")?;
                tracing::trace!(
                    query,
                    complete_current_source_build = build_summary.is_complete(),
                    "current document bodies prepared"
                );
                DocumentAnalysis {
                    snapshot,
                    analysis,
                    targets,
                    source: DocumentAnalysisSource::Current(source),
                    selection: source_selection,
                }
            }
        };

        tracing::trace!(
            query,
            source = prepared.source.name(),
            selection = ?prepared.selection,
            target_count = prepared.targets.len(),
            elapsed_us = started.elapsed().as_micros(),
            "document analysis prepared"
        );
        Ok(Some(prepared))
    }

    /// Load deferred package data for every saved file identity of one source path.
    ///
    /// A path can appear in several crate roots. Preserve every exact crate/file interpretation so
    /// preparing a shared source does not implicitly materialize sibling Cargo targets.
    fn ensure_path(
        &mut self,
        query: &'static str,
        path: &Path,
        cancellation: &QueryCancellation<'_>,
    ) -> anyhow::Result<()> {
        let started = Instant::now();

        // Resolve the path before mutating the project. One file can have several crate contexts,
        // and each interpretation has independent deferred Body IR coverage.
        let files = {
            let snapshot = self
                .project
                .saved_snapshot()
                .context("borrow saved project")?;
            Self::file_contexts(snapshot, path)
                .context("resolve query path")?
                .into_iter()
                .flat_map(|context| {
                    let file = context.file;
                    context
                        .crates
                        .into_iter()
                        .map(move |crate_ref| (crate_ref, file))
                })
                .collect::<Vec<_>>()
        };
        self.project
            .materialize_saved_project(AnalysisSurface::Files(&files), &cancellation.token())
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
    ) -> Result<Vec<gen_lsp_types::CompletionItem>, QueryRunError> {
        let document = input.document();
        let position = input.position();
        let path = document.source_path().to_path_buf();
        let source_text = document.text();
        let started = Instant::now();
        let document_analysis_started = Instant::now();
        let Some(current) = self
            .document_analysis(
                "completion",
                document,
                DocumentSelection::Position(position),
                cancellation,
            )
            .context("prepare completion analysis")?
        else {
            return Ok(Vec::new());
        };
        let document_analysis_us = document_analysis_started.elapsed().as_micros();
        let offset = current.offset();
        let completion_source_started = Instant::now();
        let completion_source = CompletionSource::new(source_text, offset);
        let completion_source_us = completion_source_started.elapsed().as_micros();

        rg_std::check_cancel!(cancellation, "after completion syntax preparation");

        rg_std::check_cancel!(cancellation, "before semantic completion");

        let mut completions = UniqueVec::new();
        let mut analysis_compute_us = 0_u128;
        let mut protocol_conversion_us = 0_u128;
        for target in &current.targets {
            rg_std::check_cancel!(cancellation, "before completion crate interpretation");
            let mut query = CompletionQuery::new(target.crate_ref, target.context.file, offset)
                .with_client_capabilities(rg_analysis::CompletionClientCapabilities {
                    snippet_support: client_capabilities.snippet_support,
                });
            query = match completion_source.as_ref() {
                Some(source) => query.with_completion_source(source),
                None => query.with_source_text(source_text),
            };
            let analysis_compute_started = Instant::now();
            let items = current
                .analysis
                .completions_at(query)
                .context("compute completions")?;
            analysis_compute_us += analysis_compute_started.elapsed().as_micros();

            // A crate query is synchronous. If it was overtaken, discard its items before
            // conversion and before starting another crate interpretation.
            rg_std::check_cancel!(cancellation, "after completion crate interpretation");
            let protocol_conversion_started = Instant::now();
            for (index, item) in items.into_iter().enumerate() {
                if index % 64 == 0 {
                    rg_std::check_cancel!(cancellation, "completion conversion");
                }
                completions.push(completion::completion_item(
                    item,
                    current.source.line_index(),
                ));
            }
            protocol_conversion_us += protocol_conversion_started.elapsed().as_micros();
        }

        let completions = completions.into_vec();
        let source_name = current.source.name();
        let crate_count = current.targets.len();
        let analysis_drop_started = Instant::now();
        drop(current);
        let analysis_drop_us = analysis_drop_started.elapsed().as_micros();
        tracing::trace!(
            crate_count,
            result_count = completions.len(),
            source = source_name,
            document_analysis_us,
            completion_source_us,
            analysis_compute_us,
            protocol_conversion_us,
            analysis_drop_us,
            "completion saved-semantics query phases finished"
        );

        tracing::trace!(
            path = %path.display(),
            line = position.line,
            character = position.character,
            result_count = completions.len(),
            elapsed_ms = started.elapsed().as_millis(),
            "completion query finished"
        );

        Ok(completions)
    }

    /// Return source actions with complete edits for the captured document and selected range.
    ///
    /// The request range enters as LSP UTF-16 positions. This method maps it to the captured
    /// source's UTF-8 offsets, asks every applicable crate interpretation for transport-neutral
    /// actions, then attaches the same captured document version during protocol conversion. One
    /// source file may belong to several crate targets, so all target interpretations are checked
    /// and equivalent actions are kept only once.
    pub(super) fn code_action(
        &mut self,
        input: DocumentRangeSnapshot,
        request_context: CodeActionRequestContext,
        cancellation: &QueryCancellation<'_>,
    ) -> Result<Vec<gen_lsp_types::CodeAction>, QueryRunError> {
        let (document, range) = input.into_parts();
        let path = document.source_path().to_path_buf();
        let started = Instant::now();

        // 1. Capture analysis for the request's editor revision and translate both ends of the LSP
        // range into byte offsets in that exact source text.
        let Some(current) = self
            .document_analysis(
                "code_action",
                &document,
                DocumentSelection::Position(range.start),
                cancellation,
            )
            .context("prepare code action analysis")?
        else {
            return Ok(Vec::new());
        };
        let Some(end) = current
            .source
            .line_index()
            .offset_from_utf16_position(crate::proto::position::parse_position(range.end))
        else {
            return Ok(Vec::new());
        };
        let start = current.offset();
        if start > end {
            return Ok(Vec::new());
        }

        // 2. Translate protocol filters once, then run analysis for every crate target that owns
        // this source file. `UniqueVec` removes the same action found through multiple targets.
        let request_kinds = request_context.kinds();
        let kinds = CodeActionKinds::none()
            .with_quick_fix(request_kinds.quick_fix())
            .with_refactor_rewrite(request_kinds.refactor_rewrite());
        let trigger = match request_context.trigger() {
            CodeActionRequestTrigger::Invoked => CodeActionTrigger::Invoked,
            CodeActionRequestTrigger::Automatic => CodeActionTrigger::Automatic,
            CodeActionRequestTrigger::Unspecified => CodeActionTrigger::Unspecified,
        };
        let mut actions = UniqueVec::new();
        for target in &current.targets {
            rg_std::check_cancel!(cancellation, "before code action crate interpretation");
            let query = CodeActionQuery::new(
                target.crate_ref,
                target.context.file,
                TextSpan { start, end },
                document.text(),
            )
            .with_kinds(kinds)
            .with_trigger(trigger);
            actions.extend(
                current
                    .analysis
                    .code_actions(query)
                    .context("compute code actions")?,
            );
            rg_std::check_cancel!(cancellation, "after code action crate interpretation");
        }

        // 3. Convert UTF-8 edits only after analysis is finished, attaching the URI and captured
        // document version to every action.
        let mut lsp_actions = Vec::new();
        for action in actions {
            rg_std::check_cancel!(cancellation, "code action conversion");
            lsp_actions.push(
                code_action::code_action(
                    document.path(),
                    document.client_version(),
                    current.source.line_index(),
                    action,
                )
                .context("convert code action")?,
            );
        }
        tracing::trace!(
            path = %path.display(),
            result_count = lsp_actions.len(),
            source = current.source.name(),
            elapsed_ms = started.elapsed().as_millis(),
            "code action query finished"
        );
        Ok(lsp_actions)
    }

    /// Return the first usable hover from the path's possible crate contexts.
    ///
    /// A hover is read from one document, but its item links can point into other open files.
    /// The input includes those documents so links can follow declarations moved by unsaved edits.
    pub(super) fn hover(
        &mut self,
        input: GlobalPositionSnapshot,
        cancellation: &QueryCancellation<'_>,
    ) -> Result<Option<gen_lsp_types::Hover>, QueryRunError> {
        let (target, _, documents, position) = input.into_parts();
        let document = documents
            .iter()
            .find(|document| document.target() == &target)
            .context("target document is absent from hover input")?;
        let path = document.source_path().to_path_buf();
        let started = Instant::now();
        let Some(current) = self
            .document_analysis(
                "hover",
                document,
                DocumentSelection::Position(position),
                cancellation,
            )
            .context("prepare hover analysis")?
        else {
            return Ok(None);
        };

        let mut hover = None;
        // Destination source is prepared only when a link points to that file. Keep this
        // conversion shared with goto requests so both features use the same editor positions.
        let mut destinations =
            CapturedNavigationDocuments::new(current.snapshot, &documents, cancellation.token());
        let offset = current.offset();
        for target in &current.targets {
            let info = current
                .analysis
                .hover(target.crate_ref, target.context.file, offset)
                .context("compute hover")?;
            let Some(info) = info else {
                continue;
            };
            let Some(value) = hover::hover(info, current.source.line_index(), |destination| {
                Ok(
                    match destinations
                        .location_for_target(
                            destination,
                            (target.context.package, target.context.file),
                            document,
                            current.source.line_index(),
                            current.analysis.current_source_view(),
                        )
                        .context("convert hover link destination")?
                    {
                        CapturedTargetLocation::Ready(location) => Some(location),
                        CapturedTargetLocation::Unsafe | CapturedTargetLocation::Unavailable => {
                            None
                        }
                    },
                )
            })
            .context("render hover documentation")?
            else {
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
            source = current.source.name(),
            elapsed_ms = started.elapsed().as_millis(),
            "hover query finished"
        );
        Ok(hover)
    }

    /// Build the document outline directly from the syntax shown by the editor.
    pub(super) fn document_symbol(
        &mut self,
        document: EditorDocumentSnapshot,
    ) -> Result<Vec<gen_lsp_types::DocumentSymbol>, QueryRunError> {
        let path = document.source_path().to_path_buf();
        let started = Instant::now();
        let snapshot = self
            .project
            .saved_snapshot()
            .context("borrow saved project for document symbols")?;
        let edition = Self::rust_edition_for_path(snapshot, &path)
            .context("resolve document-symbol edition")?;
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

        Ok(lsp_symbols)
    }

    /// Build folding ranges directly from the syntax shown by the editor.
    pub(super) fn folding_range(
        &mut self,
        document: EditorDocumentSnapshot,
        client_capabilities: FoldingClientCapabilities,
    ) -> Result<Vec<gen_lsp_types::FoldingRange>, QueryRunError> {
        let path = document.source_path().to_path_buf();
        let started = Instant::now();
        let snapshot = self
            .project
            .saved_snapshot()
            .context("borrow saved project for folding ranges")?;
        let edition = Self::rust_edition_for_path(snapshot, &path)
            .context("resolve folding-range edition")?;
        let syntax = rg_parse::parse_source_file(document.text(), edition).tree();
        let line_index = LineIndex::new(document.text());
        let lsp_ranges = Analysis::folding_ranges_from_syntax(&syntax)
            .into_iter()
            .filter_map(|fold| {
                folding_proto::folding_range(
                    document.text(),
                    &line_index,
                    client_capabilities.line_folding_only,
                    fold,
                )
            })
            .collect::<Vec<_>>();

        tracing::trace!(
            path = %path.display(),
            result_count = lsp_ranges.len(),
            elapsed_ms = started.elapsed().as_millis(),
            "folding range query finished"
        );

        Ok(lsp_ranges)
    }

    /// Format the live editor text using the owning package's Rust edition.
    ///
    /// Formatting does not need semantic materialization. The saved project is consulted only for
    /// edition metadata, and documents outside known packages use the newest supported edition.
    pub(super) fn formatting(
        &mut self,
        document: EditorDocumentSnapshot,
    ) -> Result<Option<Vec<gen_lsp_types::TextEdit>>, QueryRunError> {
        let path = document.source_path().to_path_buf();
        let text = document.text();
        let started = Instant::now();
        let edition = {
            let snapshot = self
                .project
                .saved_snapshot()
                .context("borrow saved project for formatting")?;
            Self::rust_edition_for_path(snapshot, &path).context("resolve formatting edition")?
        };
        let line_index = LineIndex::new(text);
        let formatted_text = crate::formatting::rustfmt(text, edition, line_index.line_endings())
            .context("format Rust source")?;
        let edits = formatting_proto::document_edits(text, formatted_text, &line_index)
            .context("build formatting edits")?;

        tracing::trace!(
            path = %path.display(),
            edition = %edition,
            edit_count = edits.len(),
            elapsed_ms = started.elapsed().as_millis(),
            "formatting query finished"
        );

        Ok(Some(edits))
    }

    /// Merge inlay hints from every crate context covering the requested source range.
    pub(super) fn inlay_hint(
        &mut self,
        input: DocumentRangeSnapshot,
        cancellation: &QueryCancellation<'_>,
    ) -> Result<Vec<gen_lsp_types::InlayHint>, QueryRunError> {
        let (document, range) = input.into_parts();
        let path = document.source_path().to_path_buf();
        let started = Instant::now();
        let Some(current) = self
            .document_analysis(
                "inlay_hint",
                &document,
                DocumentSelection::Range(range),
                cancellation,
            )
            .context("prepare inlay hint analysis")?
        else {
            return Ok(Vec::new());
        };
        let text_range = current.range();
        let mut hints = UniqueVec::<AnalysisInlayHint>::new();
        for target in &current.targets {
            rg_std::check_cancel!(cancellation, "before inlay hint crate interpretation");
            hints.extend(
                current
                    .analysis
                    .inlay_hints(target.crate_ref, target.context.file, Some(text_range))
                    .context("compute current body inlay hints")?,
            );
        }
        let lsp_hints = hints
            .into_iter()
            .enumerate()
            .map(|(index, hint)| {
                if index % 64 == 0 {
                    rg_std::check_cancel!(cancellation, "inlay hint conversion");
                }
                Ok(inlay_hint::inlay_hint_with_line_index(
                    current.source.line_index(),
                    hint,
                ))
            })
            .collect::<anyhow::Result<Vec<_>>>()
            .context("convert inlay hints")?;
        tracing::trace!(
            path = %path.display(),
            result_count = lsp_hints.len(),
            source = current.source.name(),
            elapsed_ms = started.elapsed().as_millis(),
            "inlay hint query finished"
        );

        Ok(lsp_hints)
    }

    /// Search the saved workspace index without loading source files for a document path.
    pub(super) fn workspace_symbol(
        &mut self,
        query: &str,
        cancellation: &QueryCancellation<'_>,
    ) -> Result<Vec<gen_lsp_types::WorkspaceSymbol>, QueryRunError> {
        let started = Instant::now();
        let lsp_symbols = self
            .project
            .saved_snapshot()
            .and_then(|snapshot| {
                let analysis = snapshot
                    .full_analysis(cancellation.token())
                    .context("load workspace-symbol analysis")?;
                let mut lsp_symbols = UniqueVec::new();

                for symbol in analysis
                    .workspace_symbols(query)
                    .context("search workspace symbols")?
                {
                    rg_std::check_cancel!(cancellation, "workspace symbol conversion");
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
