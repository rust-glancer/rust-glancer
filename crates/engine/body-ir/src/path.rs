use std::fmt;

use rg_def_map::Path;
use rg_parse::Span;

/// Body expression/pattern path together with the source span of each segment.
///
/// DefMap paths intentionally keep only the semantic shape. Body IR also needs cursor layout so
/// analysis can distinguish `Command` from `Configure` in `Command::Configure`.
#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct BodyPath {
    /// Semantic path shape shared with DefMap and Semantic IR.
    pub path: Path,
    /// Per-segment spans in the same order as `path.segments`.
    ///
    /// These are not needed for body resolution itself, but they let cursor queries resolve the
    /// prefix under the cursor instead of treating the whole qualified path as one symbol.
    pub segment_spans: Vec<Span>,
}

impl BodyPath {
    pub(crate) fn new(path: Path, segment_spans: Vec<Span>) -> Self {
        Self {
            path,
            segment_spans,
        }
    }

    /// Returns the path prefix ending at `segment_idx`.
    ///
    /// This is the shape editor queries need for `Enum::Variant`: a cursor on `Enum` should
    /// resolve the enum type, while a cursor on `Variant` should resolve the variant.
    pub fn prefix_through(&self, segment_idx: usize) -> Path {
        Path {
            absolute: self.path.absolute,
            segments: self
                .path
                .segments
                .iter()
                .take(segment_idx.saturating_add(1))
                .cloned()
                .collect(),
        }
    }

    pub fn segment_span(&self, segment_idx: usize) -> Option<Span> {
        self.segment_spans.get(segment_idx).copied()
    }

    pub fn segment_count(&self) -> usize {
        self.path.segments.len()
    }

    pub(crate) fn shrink_to_fit(&mut self) {
        self.path.shrink_to_fit();
        self.segment_spans.shrink_to_fit();
    }
}

impl fmt::Display for BodyPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.path.fmt(f)
    }
}
