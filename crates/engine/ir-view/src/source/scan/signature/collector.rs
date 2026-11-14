//! Output policies for the shared signature walk.
//!
//! Occurrence and completion scans visit the same declarations and type paths, but they produce
//! different output domains. The primary completion collector owns ordinary paths and bindings
//! already disambiguated by `=`. A separate collector reinterprets `Trait<Na$0>` as a possible
//! pre-`=` binding without erasing its primary meaning as a type argument. Keeping these policies
//! separate leaves the walker independent of a runtime scan mode.

use rg_ir_model::{FileId, Span};
use rg_item_tree::{SourceLocations, TypePath, TypeRef};

use super::{SignatureCompletionSite, SignatureSourceCandidate, SignatureTypePathScope};
use crate::source::scan::{
    NarrowestSourceSite, TypeNamePosition,
    type_path::{AssociatedTypeBindingSyntax, TypePathCompletionSite},
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
        locations: &dyn SourceLocations,
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
        locations: &dyn SourceLocations,
    ) {
        if path.anchor.is_some() {
            return;
        }

        for (idx, segment) in path.segments.iter().enumerate() {
            let Some(span) = locations.editable_span(segment.location) else {
                continue;
            };
            if !self.offset_matches(span) {
                continue;
            }
            let Some(def_map_path) = path.as_def_map_path_prefix(idx) else {
                continue;
            };
            self.push_candidate(SignatureSourceCandidate::TypePath {
                scope,
                path: def_map_path,
                type_ref: self.offset.map(|_| {
                    Box::new(TypeRef::Path(TypePath {
                        location: path.location,
                        absolute: path.absolute,
                        anchor: path.anchor.clone(),
                        segments: path.segments[..=idx].to_vec(),
                    }))
                }),
                file_id,
                span,
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
        locations: &dyn SourceLocations,
    ) {
        if let Some(binding) =
            AssociatedTypeBindingSyntax::explicit_at(path, self.offset, locations)
        {
            self.best.consider(
                SignatureCompletionSite::AssociatedTypeBinding {
                    scope,
                    trait_ref: binding.trait_ref,
                    member_prefix_span: binding.member_prefix_span,
                    existing_bindings: binding.existing_bindings,
                },
                locations.editable_span(path.location).map_or(0, Span::len),
            );
            return;
        }

        let Some(site) = TypePathCompletionSite::at(path, self.offset, position, locations) else {
            return;
        };
        let site = match site {
            TypePathCompletionSite::Qualified {
                module_qualifier,
                associated_qualifier,
                member_prefix_span,
            } => SignatureCompletionSite::Qualified {
                scope,
                module_qualifier,
                associated_qualifier,
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
        self.best.consider(
            site,
            locations.editable_span(path.location).map_or(0, Span::len),
        );
    }
}

/// Keeps the surrounding trait path for the speculative pre-`=` interpretation.
///
/// For `Iterator<It$0>`, the main collector reports `It` as a type argument. This collector runs
/// separately and reports the same spelling as a possible associated binding, so neither
/// interpretation has to erase the other.
pub(super) struct ImplicitAssociatedTypeBindingCollector {
    file_id: FileId,
    offset: u32,
    best: NarrowestSourceSite<SignatureCompletionSite>,
}

impl ImplicitAssociatedTypeBindingCollector {
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

impl SignatureScanCollector for ImplicitAssociatedTypeBindingCollector {
    fn selected_file(&self) -> Option<FileId> {
        Some(self.file_id)
    }

    fn push_candidate(&mut self, _candidate: SignatureSourceCandidate) {}

    fn push_type_path(
        &mut self,
        scope: SignatureTypePathScope,
        path: &TypePath,
        _file_id: FileId,
        _position: TypeNamePosition,
        locations: &dyn SourceLocations,
    ) {
        let Some(binding) = AssociatedTypeBindingSyntax::implicit_at(path, self.offset, locations)
        else {
            return;
        };
        self.best.consider(
            SignatureCompletionSite::AssociatedTypeBinding {
                scope,
                trait_ref: binding.trait_ref,
                member_prefix_span: binding.member_prefix_span,
                existing_bindings: binding.existing_bindings,
            },
            locations.editable_span(path.location).map_or(0, Span::len),
        );
    }
}
