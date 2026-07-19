//! Lowered import facts used by both name resolution and source-site queries.
//!
//! Resolution needs a semantic `Path`, while cursor and completion queries need the exact source
//! span for each segment. `ImportPath` keeps both views together so macro expansion cannot rewrite
//! one path representation and leave another one stale.

use std::fmt;

use rg_std::{MemorySize, Shrink};
use wincode::{SchemaRead, SchemaWrite};

use rg_ir_model::{CrateRef, ModuleId, Path, PathSegment};
use rg_item_tree::{ImportAlias, UseImportKind, UsePath, UsePathSegmentKind};
use rg_parse::Span;
use rg_text::Name;

use crate::{ItemSource, scope::Visibility};

/// One lowered import declaration.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct ImportData {
    pub module: ModuleId,
    pub visibility: Visibility,
    pub kind: ImportKind,
    pub path: ImportPath,
    pub binding: ImportBinding,
    pub alias_span: Option<Span>,
    pub source: ItemSource,
    pub import_index: usize,
}

impl ImportData {
    /// Returns the binding name introduced by this import when it is not a glob import.
    pub fn binding_name(&self) -> Option<Name> {
        let inferred_name = match self.kind {
            ImportKind::Named | ImportKind::SelfImport => self.path.semantic().last_name(),
            ImportKind::Glob => None,
        };

        self.binding.resolve(inferred_name)
    }
}

/// Binding strategy for one lowered import or extern crate item.
#[derive(
    Debug, Clone, PartialEq, Eq, derive_more::Display, SchemaRead, SchemaWrite, MemorySize, Shrink,
)]
pub enum ImportBinding {
    #[display("")]
    Inferred,
    #[display(" as {_0}")]
    Explicit(Name),
    #[display(" as _")]
    Hidden,
}

impl ImportBinding {
    pub fn from_alias(alias: &ImportAlias) -> Self {
        match alias {
            ImportAlias::Inferred => Self::Inferred,
            ImportAlias::Explicit { name, .. } => Self::Explicit(name.clone()),
            ImportAlias::Hidden => Self::Hidden,
        }
    }

    pub fn resolve(&self, inferred_name: Option<Name>) -> Option<Name> {
        match self {
            Self::Inferred => inferred_name,
            Self::Explicit(name) => Some(name.clone()),
            Self::Hidden => None,
        }
    }
}

/// Import form that matters for scope propagation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[memsize(leaf)]
#[shrink(leaf)]
pub enum ImportKind {
    Named,
    SelfImport,
    Glob,
}

impl ImportKind {
    pub fn from_use_kind(kind: UseImportKind) -> Self {
        match kind {
            UseImportKind::Named => Self::Named,
            UseImportKind::SelfImport => Self::SelfImport,
            UseImportKind::Glob => Self::Glob,
        }
    }
}

/// One semantic import path paired with source spans for the same segments.
///
/// The resolver reads `semantic`; source queries use `segments_with_spans`. Both segment lists stay
/// private so rebasing and `$crate` rewriting cannot accidentally make them disagree.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct ImportPath {
    semantic: Path,
    source_span: Option<Span>,
    segment_spans: Vec<Span>,
}

impl ImportPath {
    pub fn from_use_path(path: &UsePath) -> Self {
        let semantic = Path {
            absolute: path.absolute,
            segments: path
                .segments
                .iter()
                .map(|segment| match &segment.kind {
                    UsePathSegmentKind::Name(name) => PathSegment::Name(name.clone()),
                    UsePathSegmentKind::SelfKw => PathSegment::SelfKw,
                    UsePathSegmentKind::SuperKw => PathSegment::SuperKw,
                    UsePathSegmentKind::CrateKw => PathSegment::CrateKw,
                })
                .collect(),
        };

        Self {
            semantic,
            source_span: path.source_span,
            segment_spans: path.segments.iter().map(|segment| segment.span).collect(),
        }
    }

    pub fn semantic(&self) -> &Path {
        &self.semantic
    }

    pub fn source_span(&self) -> Option<Span> {
        self.source_span
    }

    /// Pair each semantic segment with the source span that produced it.
    ///
    /// Normal construction keeps both lists the same length. Decoded data is still checked so a
    /// source query skips an invalid path instead of attaching a cursor to the wrong segment.
    pub fn segments_with_spans(
        &self,
    ) -> Option<impl ExactSizeIterator<Item = (&PathSegment, Span)>> {
        if self.semantic.segments.len() != self.segment_spans.len() {
            return None;
        }

        Some(
            self.semantic
                .segments
                .iter()
                .zip(self.segment_spans.iter().copied()),
        )
    }

    /// Generated imports point back to the macro call rather than synthetic token-tree spans.
    pub fn rebase(&mut self, span: Span) {
        self.source_span = Some(span);
        self.segment_spans.fill(span);
    }

    /// Rewrites the generated leading `$crate` marker without changing its source projection.
    ///
    /// The original span still points at the written `$crate` token, while semantic lookup uses the
    /// selected macro definition's crate_ref.
    pub fn rewrite_dollar_crate(&mut self, crate_ref: CrateRef) {
        let Some(first) = self.semantic.segments.first_mut() else {
            return;
        };
        if matches!(first, PathSegment::Name(name) if name.as_str() == "$crate") {
            *first = PathSegment::DollarCrate(crate_ref);
            self.semantic.absolute = false;
        }
    }
}

impl fmt::Display for ImportPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.semantic.fmt(f)
    }
}

#[cfg(test)]
mod tests {
    use rg_item_tree::{UsePath, UsePathSegment, UsePathSegmentKind};
    use rg_parse::{Span, TextSpan};
    use rg_text::Name;

    use super::ImportPath;

    #[test]
    fn builds_semantic_paths_from_use_paths() {
        let cases = [
            (
                "relative keywords and names",
                use_path(
                    false,
                    &[
                        UsePathSegmentKind::CrateKw,
                        UsePathSegmentKind::SuperKw,
                        UsePathSegmentKind::SelfKw,
                        UsePathSegmentKind::Name(Name::new("User")),
                    ],
                ),
                "crate::super::self::User",
            ),
            (
                "absolute path",
                use_path(
                    true,
                    &[
                        UsePathSegmentKind::Name(Name::new("api")),
                        UsePathSegmentKind::Name(Name::new("User")),
                    ],
                ),
                "::api::User",
            ),
        ];

        for (label, path, expected) in cases {
            assert_eq!(
                ImportPath::from_use_path(&path).to_string(),
                expected,
                "{label}"
            );
        }
    }

    fn use_path(absolute: bool, kinds: &[UsePathSegmentKind]) -> UsePath {
        UsePath {
            source_span: Some(span()),
            absolute,
            segments: kinds
                .iter()
                .cloned()
                .map(|kind| UsePathSegment { kind, span: span() })
                .collect(),
        }
    }

    fn span() -> Span {
        Span {
            text: TextSpan { start: 0, end: 0 },
        }
    }
}
