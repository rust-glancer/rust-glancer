//! Analysis-query execution on top of a saved or dirty project snapshot.
//!
//! Feature methods in this module do the request-local work: map an LSP path into every matching
//! crate context, materialize deferred data when the feature needs it, run `rg_analysis`, and
//! convert semantic results back to protocol types. `lifecycle` wraps those methods with
//! cancellation, stale-buffer checks, recovery, and request-memory cleanup. Keeping the two layers
//! separate lets feature code read like a query without giving it ownership of the long-lived
//! project.

mod lifecycle;
mod navigation;
mod references;
mod source;

pub(super) use self::lifecycle::QueryContext;

use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Instant,
};

use anyhow::Context as _;
use rg_analysis::{CompletionQuery, InlayHint as AnalysisInlayHint};
use rg_lsp_proto::CompletionClientCapabilities;
use rg_project::{AnalysisSurface, DirtyOverlayScope};
use rg_std::UniqueVec;
use rg_text::RustEdition;

use crate::{
    dirty_state::DirtyState,
    documents::DirtyDocumentSnapshot,
    engine::project::ProjectCoordinator,
    memory::MemoryControl,
    proto::{completion, formatting as formatting_proto, hover, inlay_hint, symbols},
};

/// Borrows the engine state needed to execute one dispatched analysis request.
///
/// The runner does not own project generations. It prepares the query's analysis surface, borrows
/// a saved or dirty snapshot through `ProjectCoordinator`, and lets the shared lifecycle policy
/// release request-scoped loads before the dispatcher accepts the next command.
pub(super) struct QueryRunner<'a> {
    project: &'a mut ProjectCoordinator,
    dirty_state: &'a DirtyState,
    memory_control: Arc<dyn MemoryControl>,
}

impl<'a> QueryRunner<'a> {
    pub(super) fn new(
        project: &'a mut ProjectCoordinator,
        dirty_state: &'a DirtyState,
        memory_control: Arc<dyn MemoryControl>,
    ) -> Self {
        Self {
            project,
            dirty_state,
            memory_control,
        }
    }

    /// Materialize deferred data for every package-local identity of one source path.
    ///
    /// A path can belong to several crate roots, but file-shaped materialization only needs the
    /// package/file pairs. The query later expands those files into their individual crate
    /// contexts after the saved project has been enriched.
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
            .materialize(AnalysisSurface::Files(&files))
            .with_context(|| format!("prepare {query} query path"))?;
        tracing::trace!(
            query,
            file_count = files.len(),
            elapsed_ms = started.elapsed().as_millis(),
            "saved analysis surface prepared for query path"
        );
        Ok(())
    }

    /// Collect completions from every crate interpretation of the cursor.
    ///
    /// Source text is loaded only for this request so speculative syntax can recover incomplete
    /// cursor sites without making saved text permanently resident. Semantic reads still come
    /// from the matching saved project or dirty overlay.
    pub(super) fn completion(
        &mut self,
        path: PathBuf,
        position: ls_types::Position,
        client_capabilities: CompletionClientCapabilities,
        dirty: Option<DirtyDocumentSnapshot>,
    ) -> anyhow::Result<Vec<ls_types::CompletionItem>> {
        let started = Instant::now();
        self.ensure_path("completion", &path)
            .context("prepare completion path")?;
        let source_text = dirty.as_ref().map(DirtyDocumentSnapshot::text);
        let completions = self
            .project
            .with_query_snapshot(
                dirty.as_ref(),
                DirtyOverlayScope::ChangedPackages,
                |snapshot| {
                    let snapshot_query_started = Instant::now();
                    let crate_offsets_started = Instant::now();
                    let crate_offsets = Self::crate_offsets(snapshot, &path, position)
                        .context("resolve completion position")?;
                    let crate_offsets_us = crate_offsets_started.elapsed().as_micros();
                    let saved_source_text = if source_text.is_none() {
                        let Some((context, _, _)) = crate_offsets.first() else {
                            return Ok(Vec::new());
                        };
                        snapshot
                            .file_source_text(context.package, context.file)
                            .context("load saved completion source")?
                    } else {
                        None
                    };
                    let completion_source_text = source_text.or(saved_source_text.as_deref());
                    let analysis_crates = crate_offsets
                        .iter()
                        .map(|(_, crate_ref, _)| *crate_ref)
                        .collect::<Vec<_>>();
                    let analysis_load_started = Instant::now();
                    let analysis = snapshot
                        .analysis_for_crates(&analysis_crates)
                        .context("load completion analysis")?;
                    let analysis_load_us = analysis_load_started.elapsed().as_micros();
                    let mut completions = UniqueVec::new();
                    let mut line_index_load_us = 0_u128;
                    let mut analysis_compute_us = 0_u128;
                    let mut protocol_conversion_us = 0_u128;

                    for (context, crate_ref, offset) in crate_offsets {
                        let line_index_load_started = Instant::now();
                        let line_index = snapshot
                            .file_line_index(context.package, context.file)
                            .context("load completion line index")?;
                        line_index_load_us += line_index_load_started.elapsed().as_micros();
                        let Some(line_index) = line_index else {
                            continue;
                        };
                        let mut query = CompletionQuery::new(crate_ref, context.file, offset)
                            .with_client_capabilities(rg_analysis::CompletionClientCapabilities {
                                snippet_support: client_capabilities.snippet_support,
                            });
                        if let Some(source_text) = completion_source_text {
                            query = query.with_source_text(source_text);
                        }
                        let analysis_compute_started = Instant::now();
                        let items = analysis
                            .completions_at(query)
                            .context("compute completions")?;
                        analysis_compute_us += analysis_compute_started.elapsed().as_micros();
                        let protocol_conversion_started = Instant::now();
                        for item in items {
                            let item = completion::completion_item(item, line_index);
                            completions.push(item);
                        }
                        protocol_conversion_us += protocol_conversion_started.elapsed().as_micros();
                    }

                    let completions = completions.into_vec();
                    let analysis_drop_started = Instant::now();
                    drop(analysis);
                    let analysis_drop_us = analysis_drop_started.elapsed().as_micros();
                    tracing::trace!(
                        crate_count = analysis_crates.len(),
                        result_count = completions.len(),
                        crate_offsets_us,
                        analysis_load_us,
                        line_index_load_us,
                        analysis_compute_us,
                        protocol_conversion_us,
                        analysis_drop_us,
                        total_us = snapshot_query_started.elapsed().as_micros(),
                        "completion snapshot query phases finished"
                    );
                    Ok(completions)
                },
            )
            .context("run completion query")?;

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

    /// Return the first usable hover from the path's possible crate contexts.
    pub(super) fn hover(
        &mut self,
        path: PathBuf,
        position: ls_types::Position,
        dirty: Option<DirtyDocumentSnapshot>,
    ) -> anyhow::Result<Option<ls_types::Hover>> {
        let started = Instant::now();
        self.ensure_path("hover", &path)
            .context("prepare hover path")?;
        let hover = self
            .project
            .with_query_snapshot(
                dirty.as_ref(),
                DirtyOverlayScope::ChangedPackages,
                |snapshot| {
                    let crate_offsets = Self::crate_offsets(snapshot, &path, position)
                        .context("resolve hover position")?;
                    let analysis_crates = crate_offsets
                        .iter()
                        .map(|(_, crate_ref, _)| *crate_ref)
                        .collect::<Vec<_>>();
                    let analysis = snapshot
                        .analysis_for_crates(&analysis_crates)
                        .context("load hover analysis")?;

                    for (context, crate_ref, offset) in crate_offsets {
                        let Some(info) = analysis
                            .hover(crate_ref, context.file, offset)
                            .context("compute hover")?
                        else {
                            continue;
                        };
                        let Some(line_index) = snapshot
                            .file_line_index(context.package, context.file)
                            .context("load hover line index")?
                        else {
                            continue;
                        };
                        let Some(hover) = hover::hover(info, line_index) else {
                            continue;
                        };
                        return Ok(Some(hover));
                    }

                    Ok(None)
                },
            )
            .context("run hover query")?;

        tracing::trace!(
            path = %path.display(),
            line = position.line,
            character = position.character,
            has_hover = hover.is_some(),
            elapsed_ms = started.elapsed().as_millis(),
            "hover query finished"
        );
        Ok(hover)
    }

    /// Merge the document outline produced through every crate that owns this file.
    pub(super) fn document_symbol(
        &mut self,
        path: PathBuf,
        dirty: Option<DirtyDocumentSnapshot>,
    ) -> anyhow::Result<Vec<ls_types::DocumentSymbol>> {
        let started = Instant::now();
        let lsp_symbols = self
            .project
            .with_query_snapshot(
                dirty.as_ref(),
                DirtyOverlayScope::ChangedPackages,
                |snapshot| {
                    let contexts = Self::file_contexts(snapshot, &path)
                        .context("resolve document-symbol path")?;
                    let analysis_crates = contexts
                        .iter()
                        .flat_map(|context| context.crates.iter().copied())
                        .collect::<Vec<_>>();
                    let analysis = snapshot
                        .analysis_for_crates(&analysis_crates)
                        .context("load document-symbol analysis")?;
                    let mut lsp_symbols = UniqueVec::new();

                    for context in contexts {
                        for crate_ref in context.crates {
                            let symbols = analysis
                                .document_symbols(crate_ref, context.file)
                                .context("collect document symbols")?;
                            for symbol in symbols {
                                let symbol =
                                    symbols::document_symbol(snapshot, context.package, symbol)
                                        .context("convert document symbol")?;
                                lsp_symbols.push(symbol);
                            }
                        }
                    }

                    Ok(lsp_symbols.into_vec())
                },
            )
            .context("run document-symbol query")?;

        tracing::trace!(
            path = %path.display(),
            result_count = lsp_symbols.len(),
            elapsed_ms = started.elapsed().as_millis(),
            "document symbol query finished"
        );

        Ok(lsp_symbols)
    }

    /// Format the live editor text using the owning package's Rust edition.
    ///
    /// Formatting does not need semantic materialization. The saved project is consulted only for
    /// edition metadata, and documents outside known packages use the newest supported edition.
    pub(super) fn formatting(
        &mut self,
        path: PathBuf,
        text: Arc<str>,
    ) -> anyhow::Result<Vec<ls_types::TextEdit>> {
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
            crate::formatting::rustfmt(text.as_ref(), edition).context("format Rust source")?;
        let edits = formatting_proto::document_edits(text.as_ref(), formatted_text)
            .context("build formatting edits")?;

        tracing::trace!(
            path = %path.display(),
            edition = %edition,
            edit_count = edits.len(),
            elapsed_ms = started.elapsed().as_millis(),
            "formatting query finished"
        );

        Ok(edits)
    }

    /// Merge inlay hints from every crate context covering the requested source range.
    pub(super) fn inlay_hint(
        &mut self,
        path: PathBuf,
        range: ls_types::Range,
        dirty: Option<DirtyDocumentSnapshot>,
    ) -> anyhow::Result<Vec<ls_types::InlayHint>> {
        let started = Instant::now();
        self.ensure_path("inlay_hint", &path)
            .context("prepare inlay-hint path")?;
        let lsp_hints = self
            .project
            .with_query_snapshot(
                dirty.as_ref(),
                DirtyOverlayScope::ChangedPackages,
                |snapshot| {
                    let contexts =
                        Self::file_contexts(snapshot, &path).context("resolve inlay-hint path")?;
                    let analysis_crates = contexts
                        .iter()
                        .flat_map(|context| context.crates.iter().copied())
                        .collect::<Vec<_>>();
                    let analysis = snapshot
                        .analysis_for_crates(&analysis_crates)
                        .context("load inlay-hint analysis")?;
                    // A semantic hint already contains its file and source span. The package is only
                    // retained so protocol conversion can load that file, and is intentionally not
                    // part of deduplication when two crate contexts produce the same hint.
                    let mut hints = Vec::<(rg_def_map::PackageSlot, AnalysisInlayHint)>::new();

                    for context in contexts {
                        let Some(range) = Self::text_span_for_context(snapshot, &context, range)
                            .context("convert inlay-hint range")?
                        else {
                            continue;
                        };

                        for crate_ref in context.crates {
                            for hint in analysis
                                .inlay_hints(crate_ref, context.file, Some(range))
                                .context("compute inlay hints")?
                            {
                                if !hints
                                    .iter()
                                    .any(|(_, existing_hint)| existing_hint == &hint)
                                {
                                    hints.push((context.package, hint));
                                }
                            }
                        }
                    }

                    let mut lsp_hints = Vec::new();
                    for (package, hint) in hints {
                        let Some(hint) = inlay_hint::inlay_hint(snapshot, package, hint)
                            .context("convert inlay hint")?
                        else {
                            continue;
                        };
                        lsp_hints.push(hint);
                    }

                    Ok(lsp_hints)
                },
            )
            .context("run inlay-hint query")?;

        tracing::trace!(
            path = %path.display(),
            result_count = lsp_hints.len(),
            elapsed_ms = started.elapsed().as_millis(),
            "inlay hint query finished"
        );

        Ok(lsp_hints)
    }

    /// Search project-wide saved state without creating a dirty overlay or path materialization.
    pub(super) fn workspace_symbol(
        &self,
        query: &str,
    ) -> anyhow::Result<Vec<ls_types::WorkspaceSymbol>> {
        let started = Instant::now();
        let snapshot = self
            .project
            .saved_snapshot()
            .context("borrow saved project for workspace symbols")?;
        let analysis = snapshot
            .full_analysis()
            .context("load workspace-symbol analysis")?;
        let mut lsp_symbols = UniqueVec::new();

        for symbol in analysis
            .workspace_symbols(query)
            .context("search workspace symbols")?
        {
            let Some(symbol) =
                symbols::workspace_symbol(snapshot, symbol).context("convert workspace symbol")?
            else {
                continue;
            };
            lsp_symbols.push(symbol);
        }
        let lsp_symbols = lsp_symbols.into_vec();

        tracing::trace!(
            query,
            result_count = lsp_symbols.len(),
            elapsed_ms = started.elapsed().as_millis(),
            "workspace symbol query finished"
        );

        Ok(lsp_symbols)
    }
}
