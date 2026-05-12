use std::sync::Arc;

use ra_syntax::TextRange;

/// Span representation in UTF-8 byte offsets from the beginning of the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite)]
pub struct Span {
    pub text: TextSpan,
}

impl Span {
    /// Converts a syntax-level text range into the internal span representation.
    pub fn from_text_range(text_range: TextRange) -> Self {
        let start = u32::from(text_range.start());
        let end = u32::from(text_range.end());

        Self {
            text: TextSpan { start, end },
        }
    }

    /// Converts this byte span into zero-based line/column coordinates on demand.
    pub fn line_column(self, line_index: &LineIndex) -> LineColumnSpan {
        LineColumnSpan {
            start: line_index.position(self.text.start),
            end: line_index.position(self.text.end),
        }
    }

    /// Returns true when `offset` is inside the half-open text range.
    pub fn contains(self, offset: u32) -> bool {
        self.text.contains(offset)
    }

    /// Returns true when `offset` is inside the text range or exactly at its end.
    pub fn touches(self, offset: u32) -> bool {
        self.text.touches(offset)
    }

    /// Returns the byte length of the text range.
    pub fn len(self) -> u32 {
        self.text.len()
    }

    /// Returns true when the text range has no bytes.
    pub fn is_empty(self) -> bool {
        self.text.is_empty()
    }
}

/// A half-open byte-offset range within a source file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite)]
pub struct TextSpan {
    pub start: u32,
    pub end: u32,
}

impl TextSpan {
    /// Returns true when `offset` is inside the half-open range: `start <= offset < end`.
    pub fn contains(self, offset: u32) -> bool {
        self.start <= offset && offset < self.end
    }

    /// Returns true when `offset` is inside the range or exactly at its end.
    pub fn touches(self, offset: u32) -> bool {
        self.start <= offset && offset <= self.end
    }

    /// Returns the byte length of the range, saturating if invalid input ever appears.
    pub fn len(self) -> u32 {
        self.end.saturating_sub(self.start)
    }

    /// Returns true when the half-open range has no bytes.
    pub fn is_empty(self) -> bool {
        self.start >= self.end
    }
}

/// A half-open line/column range within a source file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite)]
pub struct LineColumnSpan {
    pub start: Position,
    pub end: Position,
}

/// A zero-based line/column coordinate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite)]
pub struct Position {
    pub line: u32,
    pub column: u32,
}

#[derive(Debug, Clone)]
pub struct LineIndex {
    pub(crate) lines: LineIndexStorage<LineInfo>,
    pub(crate) non_ascii_lines: LineIndexStorage<LineUtf16Metrics>,
    pub(crate) non_ascii_ranges: LineIndexStorage<LineCharRange>,
}

impl LineIndex {
    /// Builds a fast line-start index for repeated offset-to-position lookups.
    pub fn new(source: &str) -> Self {
        let mut index = Self {
            lines: LineIndexStorage::new(),
            non_ascii_lines: LineIndexStorage::new(),
            non_ascii_ranges: LineIndexStorage::new(),
        };
        let mut line_start = 0;
        let mut line = 0;

        for (idx, byte) in source.as_bytes().iter().enumerate() {
            if *byte == b'\n' {
                index.push_line(source, line, line_start, idx + 1);
                line_start = idx + 1;
                line += 1;
            }
        }
        index.push_line(source, line, line_start, source.len());

        index
    }

    pub(crate) fn pack_many(indexes: &mut [&mut Self]) {
        let mut lines = Vec::new();
        let mut non_ascii_lines = Vec::new();
        let mut non_ascii_ranges = Vec::new();
        let mut ranges = Vec::with_capacity(indexes.len());

        // Copy every file-local line table into package-local buffers. The resulting shared
        // buffers keep file offsets stable while replacing many small per-file allocations with a
        // few package-level allocations.
        for index in indexes.iter() {
            ranges.push(index.append_to_packed_buffers(
                &mut lines,
                &mut non_ascii_lines,
                &mut non_ascii_ranges,
            ));
        }

        let lines = Arc::<[LineInfo]>::from(lines.into_boxed_slice());
        let non_ascii_lines = Arc::<[LineUtf16Metrics]>::from(non_ascii_lines.into_boxed_slice());
        let non_ascii_ranges = Arc::<[LineCharRange]>::from(non_ascii_ranges.into_boxed_slice());

        for (index, range) in indexes.iter_mut().zip(ranges) {
            **index = Self {
                lines: LineIndexStorage::shared(lines.clone(), range.lines),
                non_ascii_lines: LineIndexStorage::shared(
                    non_ascii_lines.clone(),
                    range.non_ascii_lines,
                ),
                non_ascii_ranges: LineIndexStorage::shared(
                    non_ascii_ranges.clone(),
                    range.non_ascii_ranges,
                ),
            };
        }
    }

    pub fn to_snapshot(&self) -> LineIndexSnapshot {
        LineIndexSnapshot {
            lines: self.lines.as_slice().to_vec(),
            non_ascii_lines: self.non_ascii_lines.as_slice().to_vec(),
            non_ascii_ranges: self.non_ascii_ranges.as_slice().to_vec(),
        }
    }

    pub fn from_snapshot(snapshot: LineIndexSnapshot) -> Self {
        Self {
            lines: LineIndexStorage::Owned(snapshot.lines),
            non_ascii_lines: LineIndexStorage::Owned(snapshot.non_ascii_lines),
            non_ascii_ranges: LineIndexStorage::Owned(snapshot.non_ascii_ranges),
        }
    }

    /// Converts a byte offset into a zero-based line/column position.
    pub fn position(&self, offset: u32) -> Position {
        let offset = usize::try_from(offset).expect("offset should fit into usize");
        let line_index = self.line_for_offset(offset);
        let lines = self.lines.as_slice();
        let line_start =
            usize::try_from(lines[line_index].start).expect("line start should fit into usize");
        let column = offset.saturating_sub(line_start);

        Position {
            line: u32::try_from(line_index).expect("line index should fit into u32"),
            column: u32::try_from(column).expect("column should fit into u32"),
        }
    }

    /// Converts a byte offset into a zero-based line/UTF-16-column position.
    pub fn utf16_position(&self, offset: u32) -> Position {
        let offset = usize::try_from(offset).expect("offset should fit into usize");
        let line_index = self.line_for_offset(offset);
        let line = self.lines.as_slice()[line_index];
        let line_start = usize::try_from(line.start).expect("line start should fit into usize");
        let byte_column = offset.saturating_sub(line_start);
        let byte_column = u32::try_from(byte_column).unwrap_or(u32::MAX);

        Position {
            line: u32::try_from(line_index).expect("line index should fit into u32"),
            column: self
                .utf16_metrics(line_index)
                .map(|metrics| {
                    metrics.utf16_column_for_byte(self.non_ascii_ranges_for(metrics), byte_column)
                })
                .unwrap_or_else(|| byte_column.min(line.byte_len)),
        }
    }

    /// Converts a zero-based line/UTF-16-column position into a byte offset.
    pub fn offset_from_utf16_position(&self, position: Position) -> Option<u32> {
        let line_index = usize::try_from(position.line).ok()?;
        let line = *self.lines.as_slice().get(line_index)?;
        let byte_column = match self.utf16_metrics(line_index) {
            Some(metrics) => metrics
                .byte_column_for_utf16(self.non_ascii_ranges_for(metrics), position.column)?,
            None if position.column <= line.byte_len => position.column,
            None => return None,
        };

        line.start.checked_add(byte_column)
    }

    fn line_for_offset(&self, offset: usize) -> usize {
        let offset = u32::try_from(offset).unwrap_or(u32::MAX);
        match self
            .lines
            .as_slice()
            .binary_search_by_key(&offset, |line| line.start)
        {
            Ok(line) => line,
            Err(line) => line.saturating_sub(1),
        }
    }

    fn utf16_metrics(&self, line_index: usize) -> Option<&LineUtf16Metrics> {
        let line_index = u32::try_from(line_index).ok()?;
        self.non_ascii_lines
            .as_slice()
            .binary_search_by_key(&line_index, |metrics| metrics.line)
            .ok()
            .map(|idx| &self.non_ascii_lines.as_slice()[idx])
    }

    fn non_ascii_ranges_for(&self, metrics: &LineUtf16Metrics) -> &[LineCharRange] {
        metrics.ranges(self.non_ascii_ranges.as_slice())
    }

    fn line_text_end(bytes: &[u8], start: usize, next_line_start: usize) -> usize {
        let mut end = next_line_start;
        if end > start && bytes[end - 1] == b'\n' {
            end -= 1;
            if end > start && bytes[end - 1] == b'\r' {
                end -= 1;
            }
        }

        end
    }

    fn push_line(&mut self, source: &str, line: u32, line_start: usize, next_line_start: usize) {
        let line_end = Self::line_text_end(source.as_bytes(), line_start, next_line_start);
        let line_text = &source[line_start..line_end];

        self.lines.push(LineInfo {
            start: u32::try_from(line_start).expect("source offsets should fit into u32"),
            byte_len: u32::try_from(line_text.len()).expect("line length should fit into u32"),
        });

        if let Some(metrics) =
            LineUtf16Metrics::new(line, line_text, self.non_ascii_ranges.owned_vec_mut())
        {
            self.non_ascii_lines.push(metrics);
        }
    }

    pub(crate) fn shrink_to_fit(&mut self) {
        self.lines.shrink_to_fit();
        self.non_ascii_lines.shrink_to_fit();
        self.non_ascii_ranges.shrink_to_fit();
    }

    fn append_to_packed_buffers(
        &self,
        lines: &mut Vec<LineInfo>,
        non_ascii_lines: &mut Vec<LineUtf16Metrics>,
        non_ascii_ranges: &mut Vec<LineCharRange>,
    ) -> PackedLineIndexRanges {
        let line_range = LineIndexRange::new(lines.len(), self.lines.len());
        lines.extend_from_slice(self.lines.as_slice());

        let non_ascii_range =
            LineIndexRange::new(non_ascii_ranges.len(), self.non_ascii_ranges.len());
        non_ascii_ranges.extend_from_slice(self.non_ascii_ranges.as_slice());

        let metrics_range = LineIndexRange::new(non_ascii_lines.len(), self.non_ascii_lines.len());
        let old_non_ascii_start = self.non_ascii_ranges.start();
        for metrics in self.non_ascii_lines.as_slice() {
            let mut metrics = *metrics;
            let local_range_start = usize::try_from(metrics.range_start)
                .expect("range start should fit into usize")
                .saturating_sub(old_non_ascii_start);
            metrics.range_start = u32::try_from(local_range_start)
                .expect("local non-ASCII range start should fit into u32");
            non_ascii_lines.push(metrics);
        }

        PackedLineIndexRanges {
            lines: line_range,
            non_ascii_lines: metrics_range,
            non_ascii_ranges: non_ascii_range,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite)]
pub struct LineIndexSnapshot {
    pub(crate) lines: Vec<LineInfo>,
    pub(crate) non_ascii_lines: Vec<LineUtf16Metrics>,
    pub(crate) non_ascii_ranges: Vec<LineCharRange>,
}

#[derive(Debug, Clone)]
pub(crate) enum LineIndexStorage<T> {
    Owned(Vec<T>),
    Shared {
        items: Arc<[T]>,
        range: LineIndexRange,
    },
}

impl<T> LineIndexStorage<T> {
    fn new() -> Self {
        Self::Owned(Vec::new())
    }

    fn shared(items: Arc<[T]>, range: LineIndexRange) -> Self {
        Self::Shared { items, range }
    }

    pub(crate) fn as_slice(&self) -> &[T] {
        match self {
            Self::Owned(items) => items.as_slice(),
            Self::Shared { items, range } => {
                let start = range.start;
                &items[start..start + range.len]
            }
        }
    }

    pub(crate) fn start(&self) -> usize {
        match self {
            Self::Owned(_) => 0,
            Self::Shared { range, .. } => range.start,
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.as_slice().len()
    }

    fn push(&mut self, item: T) {
        match self {
            Self::Owned(items) => items.push(item),
            Self::Shared { .. } => {
                unreachable!("shared line index storage should not be mutated")
            }
        }
    }

    fn owned_vec_mut(&mut self) -> &mut Vec<T> {
        match self {
            Self::Owned(items) => items,
            Self::Shared { .. } => {
                unreachable!("shared line index storage should not be mutated")
            }
        }
    }

    fn shrink_to_fit(&mut self) {
        if let Self::Owned(items) = self {
            items.shrink_to_fit();
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct LineIndexRange {
    pub(crate) start: usize,
    pub(crate) len: usize,
}

impl LineIndexRange {
    fn new(start: usize, len: usize) -> Self {
        Self { start, len }
    }
}

#[derive(Debug, Clone, Copy)]
struct PackedLineIndexRanges {
    lines: LineIndexRange,
    non_ascii_lines: LineIndexRange,
    non_ascii_ranges: LineIndexRange,
}

/// Per-line byte facts needed for offset conversion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite)]
pub(crate) struct LineInfo {
    pub(crate) start: u32,
    pub(crate) byte_len: u32,
}

/// Sparse per-line mapping between UTF-8 byte columns and UTF-16 code-unit columns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite)]
pub(crate) struct LineUtf16Metrics {
    pub(crate) line: u32,
    pub(crate) utf16_len: u32,
    pub(crate) range_start: u32,
    pub(crate) range_len: u32,
}

impl LineUtf16Metrics {
    fn new(line: u32, line_text: &str, non_ascii_ranges: &mut Vec<LineCharRange>) -> Option<Self> {
        let mut utf16_offset = 0_u32;
        let range_start = non_ascii_ranges.len();

        for (byte_offset, ch) in line_text.char_indices() {
            let byte_start =
                u32::try_from(byte_offset).expect("line byte offset should fit into u32");
            let byte_width = u32::try_from(ch.len_utf8()).expect("UTF-8 width should fit into u32");
            let utf16_width =
                u32::try_from(ch.len_utf16()).expect("UTF-16 width should fit into u32");

            if byte_width != utf16_width {
                non_ascii_ranges.push(LineCharRange {
                    byte_start,
                    byte_end: byte_start + byte_width,
                    utf16_start: utf16_offset,
                    utf16_end: utf16_offset + utf16_width,
                });
            }

            utf16_offset += utf16_width;
        }

        let range_len = non_ascii_ranges.len().saturating_sub(range_start);
        (range_len > 0).then_some(Self {
            line,
            utf16_len: utf16_offset,
            range_start: u32::try_from(range_start)
                .expect("non-ASCII range start should fit into u32"),
            range_len: u32::try_from(range_len)
                .expect("non-ASCII range length should fit into u32"),
        })
    }

    fn utf16_column_for_byte(&self, ranges: &[LineCharRange], byte_column: u32) -> u32 {
        if byte_column >= self.byte_len(ranges) {
            return self.utf16_len;
        }

        let mut adjustment = 0;
        for range in ranges {
            if byte_column < range.byte_start {
                return byte_column.saturating_sub(adjustment);
            }
            if byte_column < range.byte_end {
                return range.utf16_start;
            }

            adjustment += range.byte_width().saturating_sub(range.utf16_width());
        }

        byte_column.saturating_sub(adjustment)
    }

    fn byte_column_for_utf16(&self, ranges: &[LineCharRange], utf16_column: u32) -> Option<u32> {
        if utf16_column > self.utf16_len {
            return None;
        }
        if utf16_column == self.utf16_len {
            return Some(self.byte_len(ranges));
        }

        let mut adjustment = 0;
        for range in ranges {
            if utf16_column < range.utf16_start {
                return Some(utf16_column + adjustment);
            }
            if utf16_column < range.utf16_end {
                return (utf16_column == range.utf16_start).then_some(range.byte_start);
            }

            adjustment += range.byte_width().saturating_sub(range.utf16_width());
        }

        Some(utf16_column + adjustment)
    }

    fn byte_len(&self, ranges: &[LineCharRange]) -> u32 {
        let adjustment = ranges
            .iter()
            .map(|range| range.byte_width().saturating_sub(range.utf16_width()))
            .sum::<u32>();
        self.utf16_len + adjustment
    }

    fn ranges<'a>(&self, ranges: &'a [LineCharRange]) -> &'a [LineCharRange] {
        let start = usize::try_from(self.range_start).expect("range start should fit into usize");
        let len = usize::try_from(self.range_len).expect("range length should fit into usize");
        &ranges[start..start + len]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite)]
pub(crate) struct LineCharRange {
    pub(crate) byte_start: u32,
    pub(crate) byte_end: u32,
    pub(crate) utf16_start: u32,
    pub(crate) utf16_end: u32,
}

impl LineCharRange {
    fn byte_width(self) -> u32 {
        self.byte_end - self.byte_start
    }

    fn utf16_width(self) -> u32 {
        self.utf16_end - self.utf16_start
    }
}

#[cfg(test)]
mod tests {
    use super::{LineIndex, Position, TextSpan};

    #[test]
    fn checks_half_open_span_containment() {
        let cases = [
            ("before start", TextSpan { start: 10, end: 20 }, 9, false),
            ("at start", TextSpan { start: 10, end: 20 }, 10, true),
            ("inside", TextSpan { start: 10, end: 20 }, 15, true),
            ("at end", TextSpan { start: 10, end: 20 }, 20, false),
            ("after end", TextSpan { start: 10, end: 20 }, 21, false),
        ];

        for (label, span, offset, expected) in cases {
            assert_eq!(span.contains(offset), expected, "{label}");
        }
    }

    #[test]
    fn checks_cursor_friendly_span_touches() {
        let cases = [
            ("before start", TextSpan { start: 10, end: 20 }, 9, false),
            ("at start", TextSpan { start: 10, end: 20 }, 10, true),
            ("inside", TextSpan { start: 10, end: 20 }, 15, true),
            ("at end", TextSpan { start: 10, end: 20 }, 20, true),
            ("after end", TextSpan { start: 10, end: 20 }, 21, false),
            ("empty at start", TextSpan { start: 10, end: 10 }, 10, true),
        ];

        for (label, span, offset, expected) in cases {
            assert_eq!(span.touches(offset), expected, "{label}");
        }
    }

    #[test]
    fn reports_saturating_span_lengths() {
        let cases = [
            ("normal", TextSpan { start: 10, end: 20 }, 10),
            ("empty", TextSpan { start: 10, end: 10 }, 0),
            ("invalid", TextSpan { start: 20, end: 10 }, 0),
        ];

        for (label, span, expected) in cases {
            assert_eq!(span.len(), expected, "{label}");
        }
    }

    #[test]
    fn converts_ascii_offsets_to_utf16_positions() {
        let index = LineIndex::new("let user = User;\nuser.id();");
        let cases = [
            ("start", 0, Position { line: 0, column: 0 }),
            ("same line", 4, Position { line: 0, column: 4 }),
            ("next line", 17, Position { line: 1, column: 0 }),
            ("inside next line", 21, Position { line: 1, column: 4 }),
        ];

        for (label, offset, expected) in cases {
            assert_eq!(index.utf16_position(offset), expected, "{label}");
            assert_eq!(
                index.offset_from_utf16_position(expected),
                Some(offset),
                "{label}"
            );
        }
    }

    #[test]
    fn converts_non_ascii_offsets_to_utf16_positions() {
        let index = LineIndex::new("é\n𝄞a");
        let cases = [
            ("accent start", 0, Position { line: 0, column: 0 }),
            ("after accent", 2, Position { line: 0, column: 1 }),
            ("second line start", 3, Position { line: 1, column: 0 }),
            ("after surrogate pair", 7, Position { line: 1, column: 2 }),
            ("after ascii", 8, Position { line: 1, column: 3 }),
        ];

        for (label, offset, expected) in cases {
            assert_eq!(index.utf16_position(offset), expected, "{label}");
            assert_eq!(
                index.offset_from_utf16_position(expected),
                Some(offset),
                "{label}"
            );
        }
    }

    #[test]
    fn packed_line_indexes_preserve_offset_conversion() {
        let mut first = LineIndex::new("é\n𝄞a");
        let mut second = LineIndex::new("a\r\nbb\n");
        LineIndex::pack_many(&mut [&mut first, &mut second]);

        assert!(matches!(
            &first.lines,
            super::LineIndexStorage::Shared { .. }
        ));
        assert!(matches!(
            &second.non_ascii_ranges,
            super::LineIndexStorage::Shared { .. }
        ));

        let first_cases = [
            ("accent start", 0, Position { line: 0, column: 0 }),
            ("after accent", 2, Position { line: 0, column: 1 }),
            ("second line start", 3, Position { line: 1, column: 0 }),
            ("after surrogate pair", 7, Position { line: 1, column: 2 }),
            ("after ascii", 8, Position { line: 1, column: 3 }),
        ];
        for (label, offset, expected) in first_cases {
            assert_eq!(first.utf16_position(offset), expected, "{label}");
            assert_eq!(
                first.offset_from_utf16_position(expected),
                Some(offset),
                "{label}"
            );
        }

        let second_cases = [
            ("first line end", Position { line: 0, column: 1 }, Some(1)),
            (
                "second line start",
                Position { line: 1, column: 0 },
                Some(3),
            ),
            ("second line end", Position { line: 1, column: 2 }, Some(5)),
            (
                "trailing empty line",
                Position { line: 2, column: 0 },
                Some(6),
            ),
        ];
        for (label, position, expected) in second_cases {
            assert_eq!(
                second.offset_from_utf16_position(position),
                expected,
                "{label}"
            );
        }
    }

    #[test]
    fn packed_line_indexes_keep_non_ascii_ranges_file_local() {
        let mut first = LineIndex::new("é");
        let mut second = LineIndex::new("prefix\n𝄞a");
        LineIndex::pack_many(&mut [&mut first, &mut second]);

        let cases = [
            ("second file start", 0, Position { line: 0, column: 0 }),
            (
                "second file non-ascii line",
                7,
                Position { line: 1, column: 0 },
            ),
            ("after surrogate pair", 11, Position { line: 1, column: 2 }),
            ("after ascii", 12, Position { line: 1, column: 3 }),
        ];

        for (label, offset, expected) in cases {
            assert_eq!(second.utf16_position(offset), expected, "{label}");
            assert_eq!(
                second.offset_from_utf16_position(expected),
                Some(offset),
                "{label}"
            );
        }
    }

    #[test]
    fn rejects_invalid_utf16_positions() {
        let index = LineIndex::new("𝄞a");
        let cases = [
            ("inside surrogate pair", Position { line: 0, column: 1 }),
            ("past line end", Position { line: 0, column: 4 }),
            ("past last line", Position { line: 1, column: 0 }),
        ];

        for (label, position) in cases {
            assert_eq!(index.offset_from_utf16_position(position), None, "{label}");
        }
    }

    #[test]
    fn treats_line_endings_as_line_boundaries() {
        let index = LineIndex::new("a\r\nbb\n");
        let cases = [
            ("first line end", Position { line: 0, column: 1 }, Some(1)),
            (
                "second line start",
                Position { line: 1, column: 0 },
                Some(3),
            ),
            ("second line end", Position { line: 1, column: 2 }, Some(5)),
            (
                "trailing empty line",
                Position { line: 2, column: 0 },
                Some(6),
            ),
        ];

        for (label, position, expected) in cases {
            assert_eq!(
                index.offset_from_utf16_position(position),
                expected,
                "{label}"
            );
        }
    }

    #[test]
    fn clamps_utf16_positions_inside_line_endings_to_line_end() {
        let index = LineIndex::new("a\r\nbb\n");
        let cases = [
            ("at carriage return", 1, Position { line: 0, column: 1 }),
            ("at newline", 2, Position { line: 0, column: 1 }),
            ("at trailing newline", 5, Position { line: 1, column: 2 }),
        ];

        for (label, offset, expected) in cases {
            assert_eq!(index.utf16_position(offset), expected, "{label}");
        }
    }
}
