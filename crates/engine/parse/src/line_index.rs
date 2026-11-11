use rg_ir_model::Span;
use rg_std::{MemorySize, Shrink};
use wincode::{SchemaRead, SchemaWrite};

/// A half-open range of zero-based line/byte-column positions within a source file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[shrink(leaf)]
pub struct LineColumnSpan {
    pub start: Position,
    pub end: Position,
}

/// A zero-based line/column coordinate.
///
/// [`LineIndex::position`] counts UTF-8 bytes in the column; [`LineIndex::utf16_position`] counts
/// UTF-16 code units.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[shrink(leaf)]
pub struct Position {
    pub line: u32,
    pub column: u32,
}

/// Newline sequence used by source text when it contains at least one line break.
///
/// Mixed source is treated as CRLF when any CRLF sequence is present. Generated edits then use one
/// predictable style instead of extending the mixture with new LF-only text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize)]
#[memsize(leaf)]
pub enum LineEndings {
    Lf,
    Crlf,
}

impl LineEndings {
    /// Rewrites actual line breaks to this source style without changing escaped `\\n` text.
    pub fn normalize_text(self, text: String) -> String {
        match self {
            Self::Lf if text.contains("\r\n") => text.replace("\r\n", "\n"),
            Self::Lf => text,
            Self::Crlf if text.contains('\n') => text.replace("\r\n", "\n").replace('\n', "\r\n"),
            Self::Crlf => text,
        }
    }
}

#[derive(Debug, Clone, MemorySize)]
pub struct LineIndex {
    line_endings: LineEndings,
    pub(crate) lines: Vec<LineInfo>,
    pub(crate) non_ascii_lines: Vec<LineUtf16Metrics>,
    pub(crate) non_ascii_ranges: Vec<LineCharRange>,
}

impl LineIndex {
    /// Builds a fast line-start index for repeated offset-to-position lookups.
    pub fn new(source: &str) -> Self {
        let mut index = Self {
            line_endings: LineEndings::Lf,
            lines: Vec::new(),
            non_ascii_lines: Vec::new(),
            non_ascii_ranges: Vec::new(),
        };
        let mut line_start = 0;
        let mut line = 0;
        let source_bytes = source.as_bytes();

        for (idx, byte) in source_bytes.iter().enumerate() {
            if *byte == b'\n' {
                if idx > 0 && source_bytes[idx - 1] == b'\r' {
                    index.line_endings = LineEndings::Crlf;
                }
                index.push_line(source, line, line_start, idx + 1);
                line_start = idx + 1;
                line += 1;
            }
        }
        index.push_line(source, line, line_start, source.len());

        index
    }

    pub fn line_endings(&self) -> LineEndings {
        self.line_endings
    }

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

    /// Converts a byte span into zero-based line/byte-column coordinates.
    pub fn line_column_span(&self, span: Span) -> LineColumnSpan {
        LineColumnSpan {
            start: self.position(span.start),
            end: self.position(span.end),
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

    /// A display range can outlive edits that shorten its document or change its characters.
    /// Clamp missing lines to EOF, long columns to line end, and split surrogate pairs to their
    /// character's start before converting to a source offset.
    pub fn offset_from_utf16_position_clamped(&self, position: Position) -> u32 {
        let lines = self.lines.as_slice();
        let Some(line) = lines.get(position.line as usize) else {
            let last = lines
                .last()
                .expect("a line index always contains its final line");
            return last.start + last.byte_len;
        };
        let byte_column = if let Some(metrics) = self.utf16_metrics(position.line as usize) {
            let ranges = self.non_ascii_ranges_for(metrics);
            let mut column = position.column.min(metrics.utf16_len);
            if let Some(range) = ranges
                .iter()
                .find(|range| range.utf16_start < column && column < range.utf16_end)
            {
                column = range.utf16_start;
            }
            metrics
                .byte_column_for_utf16(ranges, column)
                .expect("clamped column is a UTF-16 boundary within the line")
        } else {
            position.column.min(line.byte_len)
        };
        line.start + byte_column
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

        if let Some(metrics) = LineUtf16Metrics::new(line, line_text, &mut self.non_ascii_ranges) {
            self.non_ascii_lines.push(metrics);
        }
    }
}

/// Per-line byte facts needed for offset conversion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize)]
pub(crate) struct LineInfo {
    pub(crate) start: u32,
    pub(crate) byte_len: u32,
}

/// Sparse per-line mapping between UTF-8 byte columns and UTF-16 code-unit columns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize)]
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
    use super::{LineEndings, LineIndex};
    use crate::Position;

    #[test]
    fn records_source_line_endings() {
        let cases = [
            ("no line breaks", "fn main() {}", LineEndings::Lf),
            ("LF", "fn main() {}\n", LineEndings::Lf),
            ("CRLF", "fn main() {}\r\n", LineEndings::Crlf),
            ("mixed", "first\nsecond\r\n", LineEndings::Crlf),
        ];

        for (case, source, expected) in cases {
            let index = LineIndex::new(source);
            assert_eq!(index.line_endings(), expected, "{case}");
        }
    }

    #[test]
    fn normalizes_text_to_source_line_endings() {
        let text = "first\r\nsecond\n".to_string();

        assert_eq!(
            LineEndings::Lf.normalize_text(text.clone()),
            "first\nsecond\n"
        );
        assert_eq!(
            LineEndings::Crlf.normalize_text(text),
            "first\r\nsecond\r\n"
        );
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
