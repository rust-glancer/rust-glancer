use ls_types::{Position, Range};
use rg_parse::{LineIndex, Position as ParsePosition, Span};

pub(crate) fn parse_position(position: Position) -> ParsePosition {
    ParsePosition {
        line: position.line,
        column: position.character,
    }
}

pub(crate) fn range(line_index: &LineIndex, span: Span) -> Range {
    Range {
        start: position(line_index, span.text.start),
        end: position(line_index, span.text.end),
    }
}

pub(crate) fn position(line_index: &LineIndex, offset: u32) -> Position {
    let position = line_index.utf16_position(offset);

    Position {
        line: position.line,
        character: position.column,
    }
}

pub(crate) fn zero_range() -> Range {
    Range {
        start: Position::new(0, 0),
        end: Position::new(0, 0),
    }
}
