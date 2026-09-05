//! Conversion from syntax folds to LSP folding ranges.

use ls_types::{FoldingRange, FoldingRangeKind};
use rg_analysis::{Fold, FoldKind};
use rg_parse::LineIndex;

use crate::proto::position;

/// Convert a fold using the same text and line index that produced its source span.
pub(crate) fn folding_range(
    text: &str,
    line_index: &LineIndex,
    line_folding_only: bool,
    fold: Fold,
) -> Option<FoldingRange> {
    let kind = match fold.kind {
        FoldKind::Code => None,
        FoldKind::Comment => Some(FoldingRangeKind::Comment),
        FoldKind::Imports => Some(FoldingRangeKind::Imports),
    };
    let start = position::position(line_index, fold.span.text.start);
    let end = position::position(line_index, fold.span.text.end);

    if line_folding_only {
        // A line-only client hides the complete end line. Exclude it when source outside this fold
        // would otherwise disappear, as in `} else {` or `);`.
        let end_offset =
            usize::try_from(fold.span.text.end).expect("fold end offset should fit into usize");
        let source_after_fold = text
            .get(end_offset..)
            .expect("fold span should belong to its source text");
        let has_trailing_source = source_after_fold
            .chars()
            .take_while(|character| *character != '\n')
            .any(|character| !character.is_whitespace());
        let end_line = if has_trailing_source {
            end.line.saturating_sub(1)
        } else {
            end.line
        };

        if start.line >= end_line {
            return None;
        }

        return Some(FoldingRange {
            start_line: start.line,
            start_character: None,
            end_line,
            end_character: None,
            kind,
            collapsed_text: None,
        });
    }

    (start.line < end.line).then_some(FoldingRange {
        start_line: start.line,
        start_character: Some(start.character),
        end_line: end.line,
        end_character: Some(end.character),
        kind,
        collapsed_text: None,
    })
}

#[cfg(test)]
mod tests {
    use ls_types::FoldingRangeKind;
    use rg_analysis::{Fold, FoldKind};
    use rg_parse::{LineIndex, Span, TextSpan};

    use super::folding_range;

    #[test]
    fn returns_utf16_characters_to_character_capable_clients() {
        let source = "🦀 /* first\nsecond */\n";
        let start = source.find("/*").expect("comment should exist");
        let end = source.find("*/").expect("comment end should exist") + 2;
        let fold = Fold {
            span: Span {
                text: TextSpan {
                    start: u32::try_from(start).expect("start should fit into u32"),
                    end: u32::try_from(end).expect("end should fit into u32"),
                },
            },
            kind: FoldKind::Comment,
        };

        let range = folding_range(source, &LineIndex::new(source), false, fold)
            .expect("multiline comment should produce a range");

        assert_eq!(range.start_line, 0);
        assert_eq!(range.start_character, Some(3));
        assert_eq!(range.end_line, 1);
        assert_eq!(range.end_character, Some(9));
        assert_eq!(range.kind, Some(FoldingRangeKind::Comment));
    }

    #[test]
    fn protects_trailing_source_for_line_only_clients() {
        let source = "fn demo() {\n    value\n} tail\n";
        let start = source.find('{').expect("block start should exist");
        let end = source.find('}').expect("block end should exist") + 1;
        let fold = Fold {
            span: Span {
                text: TextSpan {
                    start: u32::try_from(start).expect("start should fit into u32"),
                    end: u32::try_from(end).expect("end should fit into u32"),
                },
            },
            kind: FoldKind::Code,
        };

        let range = folding_range(source, &LineIndex::new(source), true, fold)
            .expect("block should retain one collapsible line");

        assert_eq!(range.start_line, 0);
        assert_eq!(range.start_character, None);
        assert_eq!(range.end_line, 1);
        assert_eq!(range.end_character, None);
    }

    #[test]
    fn drops_line_only_ranges_emptied_by_the_end_line_adjustment() {
        let source = "/* first\nsecond */ tail\n";
        let end = source.find("*/").expect("comment end should exist") + 2;
        let fold = Fold {
            span: Span {
                text: TextSpan {
                    start: 0,
                    end: u32::try_from(end).expect("end should fit into u32"),
                },
            },
            kind: FoldKind::Comment,
        };

        assert_eq!(
            folding_range(source, &LineIndex::new(source), true, fold),
            None
        );
    }

    #[test]
    fn maps_standardized_fold_kinds() {
        let source = "start\nend\n";
        let cases = [
            (FoldKind::Code, None),
            (FoldKind::Comment, Some(FoldingRangeKind::Comment)),
            (FoldKind::Imports, Some(FoldingRangeKind::Imports)),
        ];

        for (kind, expected) in cases {
            let fold = Fold {
                span: Span {
                    text: TextSpan { start: 0, end: 9 },
                },
                kind,
            };
            let range = folding_range(source, &LineIndex::new(source), false, fold)
                .expect("multiline fold should produce a range");

            assert_eq!(range.kind, expected);
        }
    }
}
