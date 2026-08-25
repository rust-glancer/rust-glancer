//! Shared conversion of source replacement text into LSP edits.
//!
//! Analysis and formatter integrations may produce LF text even for a CRLF document. Every edit
//! passes through this boundary so feature code does not need its own newline policy.

use ls_types::{Range, TextEdit};
use rg_parse::LineIndex;

pub(crate) fn new(line_index: &LineIndex, range: Range, new_text: String) -> TextEdit {
    TextEdit {
        range,
        new_text: line_index.line_endings().normalize_text(new_text),
    }
}

#[cfg(test)]
mod tests {
    use ls_types::Range;
    use rg_parse::LineIndex;

    use super::new;

    #[test]
    fn follows_the_document_line_endings_without_doubling_existing_crlf() {
        let generated_and_copied_text = "first\r\nsecond\n".to_string();

        let lf = new(
            &LineIndex::new("source\n"),
            Range::default(),
            generated_and_copied_text.clone(),
        );
        let crlf = new(
            &LineIndex::new("source\r\n"),
            Range::default(),
            generated_and_copied_text,
        );

        assert_eq!(lf.new_text, "first\nsecond\n");
        assert_eq!(crlf.new_text, "first\r\nsecond\r\n");
    }
}
