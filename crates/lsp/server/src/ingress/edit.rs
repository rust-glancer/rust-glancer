//! Applies one `didChange` notification to complete document text.
//!
//! A notification can contain several edits, and every range is expressed against the text
//! produced by the preceding edit. This module applies them in that order and returns only the
//! final text, so another request cannot observe a partly changed document. It also records the
//! small position mapping needed to move a still-live request through those edits; it does not
//! retain old document text.

use std::{fmt, sync::Arc};

use tower_lsp_server::ls_types::{Position, Range, TextDocumentContentChangeEvent};

/// The complete text and position mapping produced by one `didChange` notification.
///
/// LSP ranges are applied one after another because each range refers to the text produced by the
/// previous edit. `EditorState` receives the result only after every edit succeeds, and therefore
/// replaces its document value once rather than exposing intermediate text.
#[derive(Debug)]
pub(super) struct AppliedDocumentChanges {
    pub(super) text: String,
    pub(super) position_transform: PositionTransform,
}

impl AppliedDocumentChanges {
    /// Apply every edit in one notification and record how positions from the old text move.
    ///
    /// A full replacement can restore exact text after a rejected incremental change. Unless the
    /// replacement is byte-for-byte identical, however, the server cannot know where positions
    /// from the older text moved. The new text remains usable, but its position mapping does not.
    pub(super) fn apply(
        current_text: Option<&str>,
        changes: &[TextDocumentContentChangeEvent],
    ) -> Result<Self, DocumentChangeError> {
        let mut text = current_text.map(str::to_owned);
        let mut position_edits = current_text.map(|_| Vec::new());

        for (change_index, change) in changes.iter().enumerate() {
            let Some(range) = change.range else {
                // A full replacement repairs an unavailable document, but it cannot generally
                // explain where a position from the missing or replaced value moved.
                if text.as_deref() != Some(change.text.as_str()) {
                    position_edits = None;
                }
                text = Some(change.text.clone());
                continue;
            };

            let Some(text) = text.as_mut() else {
                return Err(DocumentChangeError::new(
                    change_index,
                    "an incremental range followed an unavailable document without a full replacement",
                ));
            };
            if !PositionEdit::ordered(range.start, range.end) {
                return Err(DocumentChangeError::new(
                    change_index,
                    "the incremental range ends before it starts",
                ));
            }

            let start = Self::byte_offset(text, range.start).ok_or_else(|| {
                DocumentChangeError::new(
                    change_index,
                    "the incremental range start is not a UTF-16 boundary in the current text",
                )
            })?;
            let end = Self::byte_offset(text, range.end).ok_or_else(|| {
                DocumentChangeError::new(
                    change_index,
                    "the incremental range end is not a UTF-16 boundary in the current text",
                )
            })?;
            if start > end {
                return Err(DocumentChangeError::new(
                    change_index,
                    "the incremental range maps to reversed source offsets",
                ));
            }

            let position_edit = PositionEdit::new(range, &change.text).ok_or_else(|| {
                DocumentChangeError::new(
                    change_index,
                    "the inserted text exceeds the LSP position range",
                )
            })?;
            text.replace_range(start..end, &change.text);
            if let Some(position_edits) = &mut position_edits {
                position_edits.push(position_edit);
            }
        }

        let text = text.ok_or_else(|| {
            DocumentChangeError::new(
                0,
                "an unavailable document can only be repaired by a full replacement",
            )
        })?;
        let position_transform = match position_edits {
            Some(edits) => PositionTransform::Available(edits.into()),
            None => PositionTransform::Unavailable,
        };

        Ok(Self {
            text,
            position_transform,
        })
    }

    /// Convert one advertised UTF-16 position to a byte boundary in `text`.
    fn byte_offset(text: &str, position: Position) -> Option<usize> {
        let requested_line = usize::try_from(position.line).ok()?;
        let mut line = 0_usize;
        let mut line_start = 0_usize;

        while line < requested_line {
            let relative_newline = text
                .as_bytes()
                .get(line_start..)?
                .iter()
                .position(|byte| *byte == b'\n')?;
            line_start = line_start.checked_add(relative_newline)?.checked_add(1)?;
            line += 1;
        }

        let next_newline = text.as_bytes()[line_start..]
            .iter()
            .position(|byte| *byte == b'\n')
            .map(|relative| line_start + relative)
            .unwrap_or(text.len());
        let line_end =
            if next_newline > line_start && text.as_bytes().get(next_newline - 1) == Some(&b'\r') {
                next_newline - 1
            } else {
                next_newline
            };
        let line_text = text.get(line_start..line_end)?;

        let mut utf16_column = 0_u32;
        for (byte_column, character) in line_text.char_indices() {
            if utf16_column == position.character {
                return line_start.checked_add(byte_column);
            }
            utf16_column = utf16_column.checked_add(character.len_utf16() as u32)?;
            if utf16_column > position.character {
                return None;
            }
        }
        (utf16_column == position.character).then_some(line_end)
    }
}

/// A position mapping through one accepted document notification.
///
/// `Unavailable` does not mean that the new text was rejected. A full replacement can provide
/// exact new text while still making a position captured in the old text impossible to locate.
#[derive(Debug)]
pub(super) enum PositionTransform {
    Available(Arc<[PositionEdit]>),
    Unavailable,
}

impl PositionTransform {
    pub(super) fn rebase(&self, mut position: Position) -> Option<Position> {
        let Self::Available(edits) = self else {
            return None;
        };
        for edit in edits.iter() {
            position = edit.rebase(position)?;
        }
        Some(position)
    }
}

/// The part of an accepted incremental edit needed to move an older request's position later.
#[derive(Debug)]
pub(super) struct PositionEdit {
    range: Range,
    inserted_end: Position,
}

impl PositionEdit {
    fn new(range: Range, inserted_text: &str) -> Option<Self> {
        let mut inserted_end = range.start;
        for character in inserted_text.chars() {
            if character == '\n' {
                inserted_end.line = inserted_end.line.checked_add(1)?;
                inserted_end.character = 0;
            } else {
                inserted_end.character = inserted_end
                    .character
                    .checked_add(character.len_utf16() as u32)?;
            }
        }
        Some(Self {
            range,
            inserted_end,
        })
    }

    fn rebase(&self, position: Position) -> Option<Position> {
        if !Self::ordered(self.range.start, position) {
            return Some(position);
        }

        // Positions inside a replacement move to the end of the inserted text. For an empty
        // range this gives typing right affinity: inserting `ck` at `RwLo|` yields `RwLock|`.
        if Self::ordered(position, self.range.end) {
            return Some(self.inserted_end);
        }

        if position.line == self.range.end.line {
            let trailing_character = position.character.checked_sub(self.range.end.character)?;
            Some(Position::new(
                self.inserted_end.line,
                self.inserted_end
                    .character
                    .checked_add(trailing_character)?,
            ))
        } else {
            let trailing_lines = position.line.checked_sub(self.range.end.line)?;
            Some(Position::new(
                self.inserted_end.line.checked_add(trailing_lines)?,
                position.character,
            ))
        }
    }

    fn ordered(left: Position, right: Position) -> bool {
        (left.line, left.character) <= (right.line, right.character)
    }
}

/// Why one `didChange` notification could not produce complete document text.
#[derive(Debug)]
pub(crate) struct DocumentChangeError {
    change_index: usize,
    reason: &'static str,
}

impl DocumentChangeError {
    fn new(change_index: usize, reason: &'static str) -> Self {
        Self {
            change_index,
            reason,
        }
    }
}

impl fmt::Display for DocumentChangeError {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            fmt,
            "content change {} was rejected: {}",
            self.change_index, self.reason
        )
    }
}

#[cfg(test)]
mod tests {
    use tower_lsp_server::ls_types::{Position, Range, TextDocumentContentChangeEvent};

    use super::AppliedDocumentChanges;

    #[test]
    fn typing_at_the_tracked_position_uses_right_affinity() {
        let applied =
            AppliedDocumentChanges::apply(Some("RwLo"), &[incremental((0, 4), (0, 4), "ck")])
                .expect("valid typing should apply");

        assert_eq!(applied.text, "RwLock");
        assert_eq!(
            applied.position_transform.rebase(Position::new(0, 4)),
            Some(Position::new(0, 6))
        );
    }

    #[test]
    fn changes_are_applied_and_rebased_in_notification_order() {
        let applied = AppliedDocumentChanges::apply(
            Some("let value = RwLo;\nvalue"),
            &[
                incremental((0, 0), (0, 0), "// 😀\n"),
                incremental((1, 16), (1, 16), "ck"),
                incremental((2, 0), (2, 5), "lock"),
            ],
        )
        .expect("sequential edits should apply");

        assert_eq!(applied.text, "// 😀\nlet value = RwLock;\nlock");
        assert_eq!(
            applied.position_transform.rebase(Position::new(0, 16)),
            Some(Position::new(1, 18))
        );
    }

    #[test]
    fn multiline_deletion_before_the_position_moves_it_to_the_remaining_line() {
        let applied = AppliedDocumentChanges::apply(
            Some("prefix 😀\nimpl RwLo"),
            &[incremental((0, 0), (1, 0), "")],
        )
        .expect("multiline deletion should apply");

        assert_eq!(applied.text, "impl RwLo");
        assert_eq!(
            applied.position_transform.rebase(Position::new(1, 9)),
            Some(Position::new(0, 9))
        );
    }

    #[test]
    fn non_bmp_text_before_an_edit_uses_utf16_columns() {
        let applied =
            AppliedDocumentChanges::apply(Some("😀 RwLo"), &[incremental((0, 7), (0, 7), "ck")])
                .expect("UTF-16 range after an emoji should apply");

        assert_eq!(applied.text, "😀 RwLock");
        assert_eq!(
            applied.position_transform.rebase(Position::new(0, 7)),
            Some(Position::new(0, 9))
        );
    }

    #[test]
    fn utf16_ranges_reject_positions_inside_a_surrogate_pair() {
        let error =
            AppliedDocumentChanges::apply(Some("a😀b"), &[incremental((0, 2), (0, 2), "x")])
                .expect_err("a UTF-16 position inside the emoji should be rejected");

        assert!(error.to_string().contains("UTF-16 boundary"));
    }

    #[test]
    fn a_different_full_replacement_does_not_guess_the_old_position() {
        let applied =
            AppliedDocumentChanges::apply(Some("fn old() {}"), &[full("fn new_name() {}")])
                .expect("full replacement should still materialize exact text");

        assert_eq!(applied.text, "fn new_name() {}");
        assert_eq!(applied.position_transform.rebase(Position::new(0, 6)), None);
    }

    #[test]
    fn a_full_replacement_repairs_missing_text_without_claiming_a_mapping() {
        let applied = AppliedDocumentChanges::apply(None, &[full("fn repaired() {}")])
            .expect("full replacement should repair unavailable text");

        assert_eq!(applied.text, "fn repaired() {}");
        assert_eq!(applied.position_transform.rebase(Position::new(0, 2)), None);
    }

    fn incremental(
        start: (u32, u32),
        end: (u32, u32),
        text: &str,
    ) -> TextDocumentContentChangeEvent {
        TextDocumentContentChangeEvent {
            range: Some(Range::new(
                Position::new(start.0, start.1),
                Position::new(end.0, end.1),
            )),
            range_length: None,
            text: text.to_string(),
        }
    }

    fn full(text: &str) -> TextDocumentContentChangeEvent {
        TextDocumentContentChangeEvent {
            range: None,
            range_length: None,
            text: text.to_string(),
        }
    }
}
