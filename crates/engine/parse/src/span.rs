use rg_syntax::TextRange;

use crate::LineIndex;
use rg_std::MemorySize;
use wincode::{SchemaRead, SchemaWrite};

/// Span representation in UTF-8 byte offsets from the beginning of the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize)]
pub struct LineColumnSpan {
    pub start: Position,
    pub end: Position,
}

/// A zero-based line/column coordinate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize)]
pub struct Position {
    pub line: u32,
    pub column: u32,
}

#[cfg(test)]
mod tests {
    use super::TextSpan;

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
}
