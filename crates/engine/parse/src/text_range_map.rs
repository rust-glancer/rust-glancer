//! Keep positions connected when text is copied, decoded, or surrounded by generated syntax.

use std::ops::Range;

/// Translate between transformed text and the original bytes that supplied it.
///
/// The map owns no text. Callers record the fragments they copy or transform, leaving generated
/// text and omitted source bytes unmapped. For example, a synthetic function wrapper has no
/// original range, while the `o` decoded from `\u{6f}` maps to that whole escape.
///
/// Entries follow transformed-text order and must not overlap there. Original ranges can appear
/// in a different order, so range projection searches the transformed side and reverse cursor
/// lookup scans the original side. All positions are half-open UTF-8 byte ranges.
#[derive(Debug, Default)]
pub struct TextRangeMap {
    mappings: Vec<TextRangeMapping>,
}

/// One correspondence between transformed text and its original spelling.
///
/// Range projection returns the same shape so a caller can copy the projected text into another
/// buffer, adjust `generated`, and retain how the original bytes should be selected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextRangeMapping {
    pub generated: Range<usize>,
    pub original: Range<usize>,
    kind: MappingKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MappingKind {
    Copied,
    Transformed,
}

impl TextRangeMapping {
    /// Copied text translates byte for byte; both ranges must have the same length.
    pub fn copied(generated: Range<usize>, original: Range<usize>) -> Self {
        Self {
            generated,
            original,
            kind: MappingKind::Copied,
        }
    }

    /// Any selection within transformed text selects its whole original spelling, even when
    /// the two ranges happen to have the same length.
    pub fn transformed(generated: Range<usize>, original: Range<usize>) -> Self {
        Self {
            generated,
            original,
            kind: MappingKind::Transformed,
        }
    }
}

impl TextRangeMap {
    /// Append a correspondence in transformed-text order. Empty sides have no positions to map.
    /// Touching copied fragments share an entry, while transformed fragments stay separate so
    /// each original spelling can still be selected as a whole.
    pub fn push(&mut self, mapping: TextRangeMapping) {
        if mapping.generated.is_empty() || mapping.original.is_empty() {
            return;
        }
        assert!(
            mapping.kind != MappingKind::Copied
                || mapping.generated.len() == mapping.original.len(),
            "copied ranges must have equal byte lengths"
        );
        if let Some(last) = self.mappings.last_mut() {
            assert!(
                last.generated.end <= mapping.generated.start,
                "mappings must follow transformed-text order without overlap"
            );
            if last.kind == MappingKind::Copied
                && mapping.kind == MappingKind::Copied
                && last.generated.end == mapping.generated.start
                && last.original.end == mapping.original.start
            {
                last.generated.end = mapping.generated.end;
                last.original.end = mapping.original.end;
                return;
            }
        }
        self.mappings.push(mapping);
    }

    /// Project a transformed-text range onto the original text, yielding nonempty intersections.
    /// Copied ranges are clipped on both sides. A transformed fragment keeps its complete
    /// original range, and gaps in the map contribute nothing.
    pub fn project(&self, range: Range<usize>) -> impl Iterator<Item = TextRangeMapping> + '_ {
        let first = self
            .mappings
            .partition_point(|mapping| mapping.generated.end <= range.start);
        self.mappings[first..]
            .iter()
            .take_while(move |mapping| mapping.generated.start < range.end)
            .filter_map(move |mapping| {
                let generated =
                    mapping.generated.start.max(range.start)..mapping.generated.end.min(range.end);
                if generated.is_empty() {
                    return None;
                }
                let original = match mapping.kind {
                    MappingKind::Copied => {
                        mapping.original.start + generated.start - mapping.generated.start
                            ..mapping.original.start + generated.end - mapping.generated.start
                    }
                    MappingKind::Transformed => mapping.original.clone(),
                };
                Some(TextRangeMapping {
                    generated,
                    original,
                    kind: mapping.kind,
                })
            })
    }

    /// Locate an original byte offset in transformed text. An offset inside a transformed
    /// fragment selects its start; omitted source bytes have no match. If an original fragment
    /// was used more than once, return its first occurrence in transformed-text order.
    pub fn generated_offset(&self, original_offset: usize) -> Option<usize> {
        self.mappings.iter().find_map(|mapping| {
            if !mapping.original.contains(&original_offset) {
                return None;
            }
            Some(match mapping.kind {
                MappingKind::Copied => {
                    mapping.generated.start + original_offset - mapping.original.start
                }
                MappingKind::Transformed => mapping.generated.start,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{TextRangeMap, TextRangeMapping};

    #[test]
    fn projects_copied_text_and_decoded_escapes() {
        // `#[doc = "Pr\u{6f}file"]` contributes the Markdown `Profile`.
        let mut map = TextRangeMap::default();
        map.push(TextRangeMapping::copied(0..1, 9..10));
        map.push(TextRangeMapping::copied(1..2, 10..11));
        map.push(TextRangeMapping::transformed(2..3, 11..17));
        map.push(TextRangeMapping::copied(3..7, 17..21));

        let ranges = [
            (0..2, vec![(0..2, 9..11)], "adjacent copied text"),
            (
                1..5,
                vec![(1..2, 10..11), (2..3, 11..17), (3..5, 17..19)],
                "selection across an escape",
            ),
            (2..3, vec![(2..3, 11..17)], "whole escape spelling"),
        ];
        for (range, expected, description) in ranges {
            let projected = map
                .project(range)
                .map(|mapping| (mapping.generated, mapping.original))
                .collect::<Vec<_>>();
            assert_eq!(projected, expected, "{description}");
        }
        for (original, generated) in [(9, 0), (10, 1), (11, 2), (14, 2), (16, 2), (17, 3), (20, 6)]
        {
            assert_eq!(
                map.generated_offset(original),
                Some(generated),
                "{original}"
            );
        }
    }

    #[test]
    fn maps_fragments_with_gaps_and_different_source_order() {
        let mut map = TextRangeMap::default();
        map.push(TextRangeMapping::copied(2..5, 10..13));
        map.push(TextRangeMapping::copied(7..10, 0..3));

        let projected = map
            .project(0..12)
            .map(|mapping| (mapping.generated, mapping.original))
            .collect::<Vec<_>>();
        assert_eq!(projected, vec![(2..5, 10..13), (7..10, 0..3)]);
        assert_eq!(map.generated_offset(1), Some(8));
        assert_eq!(map.generated_offset(11), Some(3));
        assert_eq!(
            map.generated_offset(5),
            None,
            "omitted source has no position"
        );
        assert_eq!(map.project(5..7).count(), 0, "generated gap has no source");
    }

    #[test]
    fn retains_transformation_when_ranges_have_equal_lengths() {
        let mut map = TextRangeMap::default();
        map.push(TextRangeMapping::transformed(0..7, 10..17));
        let projected = map.project(2..4).collect::<Vec<_>>();
        assert_eq!(projected, vec![TextRangeMapping::transformed(2..4, 10..17)]);
        assert_eq!(map.generated_offset(13), Some(0));
    }
}
