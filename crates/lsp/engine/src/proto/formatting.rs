//! Normalizes formatter output into the shape expected by LSP formatting.
//!
//! The formatter produces document text, while LSP formatting returns text edits to apply to the
//! current document. This module owns that translation.

use anyhow::Context as _;
use dissimilar::Chunk;
use ls_types::{Position, Range, TextEdit};
use rg_parse::LineIndex;

pub(crate) fn document_edits(
    old_text: &str,
    formatted_text: String,
) -> anyhow::Result<Vec<TextEdit>> {
    if formatted_text == old_text {
        return Ok(Vec::new());
    }

    let line_index = LineIndex::new(old_text);
    let mut old_offset = 0;
    let mut edits = Vec::new();

    for chunk in dissimilar::diff(old_text, &formatted_text) {
        match chunk {
            Chunk::Equal(text) => {
                old_offset += text.len();
            }
            Chunk::Delete(text) => {
                let start = old_offset;
                let end = old_offset + text.len();
                edits.push(TextEdit {
                    range: range(&line_index, start, end)?,
                    new_text: String::new(),
                });
                old_offset = end;
            }
            Chunk::Insert(text) => {
                edits.push(TextEdit {
                    range: range(&line_index, old_offset, old_offset)?,
                    new_text: text.to_owned(),
                });
            }
        }
    }

    Ok(edits)
}

fn range(line_index: &LineIndex, start: usize, end: usize) -> anyhow::Result<Range> {
    let start = u32::try_from(start).context("while attempting to convert edit start offset")?;
    let end = u32::try_from(end).context("while attempting to convert edit end offset")?;
    let start = line_index.utf16_position(start);
    let end = line_index.utf16_position(end);

    Ok(Range {
        start: Position::new(start.line, start.column),
        end: Position::new(end.line, end.column),
    })
}

#[cfg(test)]
mod tests {
    use super::document_edits;

    #[test]
    fn unchanged_text_returns_no_edits() {
        let edits = document_edits("fn main() {}\n", "fn main() {}\n".to_string())
            .expect("unchanged formatting should succeed");

        assert!(edits.is_empty());
    }

    #[test]
    fn inserted_text_becomes_an_insert_edit() {
        let edits = document_edits(
            "fn main() {\n}\n",
            "fn main() {\n    work();\n}\n".to_string(),
        )
        .expect("changed formatting should succeed");

        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].range.start.line, 1);
        assert_eq!(edits[0].range.start.character, 0);
        assert_eq!(edits[0].range.end.line, 1);
        assert_eq!(edits[0].range.end.character, 0);
        assert_eq!(edits[0].new_text, "    work();\n");
    }

    #[test]
    fn deleted_text_becomes_a_delete_edit() {
        let edits = document_edits(
            "fn main() {\n    work();\n}\n",
            "fn main() {\n}\n".to_string(),
        )
        .expect("changed formatting should succeed");

        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].range.start.line, 1);
        assert_eq!(edits[0].range.start.character, 0);
        assert_eq!(edits[0].range.end.line, 2);
        assert_eq!(edits[0].range.end.character, 0);
        assert_eq!(edits[0].new_text, "");
    }

    #[test]
    fn edit_ranges_use_utf16_positions() {
        let edits = document_edits("🦀value", "🦀value2".to_string())
            .expect("changed formatting should succeed");

        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].range.start.line, 0);
        assert_eq!(edits[0].range.start.character, 7);
        assert_eq!(edits[0].range.end.line, 0);
        assert_eq!(edits[0].range.end.character, 7);
        assert_eq!(edits[0].new_text, "2");
    }
}
