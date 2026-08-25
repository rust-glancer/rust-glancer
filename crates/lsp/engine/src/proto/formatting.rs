//! Normalizes formatter output into the shape expected by LSP formatting.
//!
//! The formatter produces document text, while LSP formatting returns text edits to apply to the
//! current document. This module owns that translation.

use anyhow::Context as _;
use dissimilar::Chunk;
use ls_types::{Position, Range, TextEdit};
use rg_parse::LineIndex;

use crate::proto::text_edit;

pub(crate) fn document_edits(
    old_text: &str,
    formatted_text: String,
    line_index: &LineIndex,
) -> anyhow::Result<Vec<TextEdit>> {
    if formatted_text == old_text {
        return Ok(Vec::new());
    }

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
                edits.push(text_edit::new(
                    line_index,
                    range(line_index, start, end)?,
                    String::new(),
                ));
                old_offset = end;
            }
            Chunk::Insert(text) => {
                edits.push(text_edit::new(
                    line_index,
                    range(line_index, old_offset, old_offset)?,
                    text.to_owned(),
                ));
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
    use rg_parse::LineIndex;

    use super::document_edits;

    #[test]
    fn unchanged_text_returns_no_edits() {
        let source = "fn main() {}\n";
        let edits = document_edits(source, source.to_string(), &LineIndex::new(source))
            .expect("unchanged formatting should succeed");

        assert!(edits.is_empty());
    }

    #[test]
    fn inserted_text_becomes_an_insert_edit() {
        let source = "fn main() {\n}\n";
        let edits = document_edits(
            source,
            "fn main() {\n    work();\n}\n".to_string(),
            &LineIndex::new(source),
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
    fn inserted_text_uses_crlf_for_crlf_source() {
        let source = "fn main() {\r\n}\r\n";
        let edits = document_edits(
            source,
            "fn main() {\r\n    work();\r\n}\r\n".to_string(),
            &LineIndex::new(source),
        )
        .expect("changed formatting should succeed");

        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].range.start.line, 1);
        assert_eq!(edits[0].range.start.character, 0);
        assert_eq!(edits[0].new_text, "    work();\r\n");
    }

    #[test]
    fn deleted_text_becomes_a_delete_edit() {
        let source = "fn main() {\n    work();\n}\n";
        let edits = document_edits(
            source,
            "fn main() {\n}\n".to_string(),
            &LineIndex::new(source),
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
        let source = "🦀value";
        let edits = document_edits(source, "🦀value2".to_string(), &LineIndex::new(source))
            .expect("changed formatting should succeed");

        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].range.start.line, 0);
        assert_eq!(edits[0].range.start.character, 7);
        assert_eq!(edits[0].range.end.line, 0);
        assert_eq!(edits[0].range.end.character, 7);
        assert_eq!(edits[0].new_text, "2");
    }
}
