//! Output policies for the shared signature walk.
//!
//! Occurrence and completion scans visit the same declarations and type paths, but they produce
//! different output domains. Keeping their state in separate collectors makes each result valid
//! by construction and leaves the walker independent of a runtime scan mode.

use rg_item_tree::TypePath;
use rg_parse::{FileId, Span};

use super::{SignatureCompletionSite, SignatureSourceCandidate, SignatureTypePathScope};
use crate::source::scan::{
    NarrowestSourceSite, TypeNamePosition, type_path::TypePathCompletionSite,
};

/// Receives the declaration names and type paths discovered by one signature walk.
///
/// The walker supplies source facts in item order. A collector decides whether those facts become
/// a list of navigable occurrences or one cursor-selected completion site.
pub(super) trait SignatureScanCollector {
    fn selected_file(&self) -> Option<FileId>;

    fn push_candidate(&mut self, candidate: SignatureSourceCandidate);

    fn push_type_path(
        &mut self,
        scope: SignatureTypePathScope,
        path: &TypePath,
        file_id: FileId,
        position: TypeNamePosition,
    );
}

/// Collects signature occurrences, optionally restricted to one file and cursor offset.
pub(super) struct SignatureOccurrenceCollector {
    selected_file: Option<FileId>,
    offset: Option<u32>,
    candidates: Vec<SignatureSourceCandidate>,
}

impl SignatureOccurrenceCollector {
    pub(super) fn new(selected_file: Option<FileId>, offset: Option<u32>) -> Self {
        Self {
            selected_file,
            offset,
            candidates: Vec::new(),
        }
    }

    pub(super) fn finish(self) -> Vec<SignatureSourceCandidate> {
        self.candidates
    }

    fn offset_matches(&self, span: Span) -> bool {
        self.offset.is_none_or(|offset| span.touches(offset))
    }
}

impl SignatureScanCollector for SignatureOccurrenceCollector {
    fn selected_file(&self) -> Option<FileId> {
        self.selected_file
    }

    fn push_candidate(&mut self, candidate: SignatureSourceCandidate) {
        if self.offset_matches(candidate.span()) {
            self.candidates.push(candidate);
        }
    }

    /// Emits each unanchored path prefix as its own navigable occurrence.
    ///
    /// For `outer::Inner<Vec<T>>`, segment occurrences resolve as `outer` and `outer::Inner`;
    /// the shared type-reference walker visits `Vec` and `T` separately.
    fn push_type_path(
        &mut self,
        scope: SignatureTypePathScope,
        path: &TypePath,
        file_id: FileId,
        _position: TypeNamePosition,
    ) {
        if path.anchor.is_some() {
            return;
        }

        for (idx, segment) in path.segments.iter().enumerate() {
            if !self.offset_matches(segment.span) {
                continue;
            }
            let Some(path) = path.as_def_map_path_prefix(idx) else {
                continue;
            };
            self.push_candidate(SignatureSourceCandidate::TypePath {
                context: scope.context,
                path,
                file_id,
                span: segment.span,
            });
        }
    }
}

/// Keeps the smallest signature type path that can accept completion at one cursor offset.
pub(super) struct SignatureCompletionCollector {
    file_id: FileId,
    offset: u32,
    best: NarrowestSourceSite<SignatureCompletionSite>,
}

impl SignatureCompletionCollector {
    pub(super) fn new(file_id: FileId, offset: u32) -> Self {
        Self {
            file_id,
            offset,
            best: NarrowestSourceSite::new(),
        }
    }

    pub(super) fn finish(self) -> Option<SignatureCompletionSite> {
        self.best.finish()
    }
}

impl SignatureScanCollector for SignatureCompletionCollector {
    fn selected_file(&self) -> Option<FileId> {
        Some(self.file_id)
    }

    fn push_candidate(&mut self, _candidate: SignatureSourceCandidate) {}

    fn push_type_path(
        &mut self,
        scope: SignatureTypePathScope,
        path: &TypePath,
        _file_id: FileId,
        position: TypeNamePosition,
    ) {
        let Some(site) = TypePathCompletionSite::at(path, self.offset, position) else {
            return;
        };
        let site = match site {
            TypePathCompletionSite::Qualified {
                qualifier,
                member_prefix_span,
            } => SignatureCompletionSite::Qualified {
                scope,
                qualifier,
                member_prefix_span,
            },
            TypePathCompletionSite::Unqualified {
                member_prefix_span,
                member_prefix,
                position,
            } => SignatureCompletionSite::Unqualified {
                scope,
                member_prefix_span,
                member_prefix,
                position,
            },
        };
        self.best.consider(site, path.source_span.len());
    }
}
