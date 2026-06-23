//! Normalizes formatter output into the shape expected by LSP formatting.
//!
//! The formatter produces document text, while LSP formatting returns text edits to apply to the
//! current document. This module owns that translation.

use anyhow::Context as _;
use ls_types::{Position, Range, TextEdit};
use rg_parse::LineIndex;

pub(crate) fn document_edits(
    old_text: &str,
    formatted_text: String,
) -> anyhow::Result<Vec<TextEdit>> {
    if formatted_text == old_text {
        return Ok(Vec::new());
    }

    Ok(vec![TextEdit {
        range: full_document_range(old_text)?,
        new_text: formatted_text,
    }])
}

fn full_document_range(text: &str) -> anyhow::Result<Range> {
    let end_offset =
        u32::try_from(text.len()).context("while attempting to convert document length")?;
    let end = LineIndex::new(text).utf16_position(end_offset);

    Ok(Range {
        start: Position::new(0, 0),
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
    fn changed_text_replaces_whole_document_with_utf16_end_position() {
        let edits = document_edits("let _ = \"🦀\";", "let _ = \"formatted\";\n".to_string())
            .expect("changed formatting should succeed");

        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].range.start.line, 0);
        assert_eq!(edits[0].range.start.character, 0);
        assert_eq!(edits[0].range.end.line, 0);
        assert_eq!(edits[0].range.end.character, 13);
        assert_eq!(edits[0].new_text, "let _ = \"formatted\";\n");
    }
}
