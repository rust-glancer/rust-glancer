pub mod inline;

use std::fmt::Write as _;

use crate::{FixtureMarker, FixtureMarkers, FixtureSpec};

/// Source text with inline markers stripped and retained as byte offsets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkedText {
    text: String,
    markers: FixtureMarkers,
}

impl MarkedText {
    pub fn parse(text: &str) -> Self {
        let mut clean = String::new();
        let mut markers = FixtureMarkers::default();
        let mut offset = 0_u32;

        // Dirty-buffer fixtures are source snippets, not fixture files. Preserve the exact text
        // shape while reusing the same marker syntax as inline crate fixtures.
        for line in text.split_inclusive('\n') {
            let (line, has_newline) = line
                .strip_suffix('\n')
                .map(|line| (line, true))
                .unwrap_or((line, false));
            let cleaned =
                FixtureSpec::strip_markers_from_line(line, "<marked-text>", offset, &mut markers);
            offset += u32::try_from(cleaned.len()).expect("marked text length should fit into u32");
            clean.push_str(&cleaned);

            if has_newline {
                clean.push('\n');
                offset += 1;
            }
        }

        Self {
            text: clean,
            markers,
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn marker(&self, name: &str) -> &FixtureMarker {
        self.markers.position(name)
    }

    pub fn offset(&self, name: &str) -> usize {
        usize::try_from(self.marker(name).offset).expect("marked text offset should fit usize")
    }
}

/// Put short answers beside their titles and keep structured answers in separate blocks.
pub fn render_responses<'a>(responses: impl IntoIterator<Item = (&'a str, String)>) -> String {
    let mut dump = String::new();
    let mut previous_multiline = false;

    // Keep short answers together, but leave space around lists and structured responses so
    // their titles and indented details are easy to follow.
    for (title, response) in responses {
        let response = response.trim_end();
        let multiline = response.contains('\n');
        if !dump.is_empty() && (previous_multiline || multiline) {
            writeln!(dump).expect("string writes should not fail");
        }

        if response.is_empty() {
            writeln!(dump, "// {title}").expect("string writes should not fail");
        } else if multiline {
            writeln!(dump, "// {title}\n{response}").expect("string writes should not fail");
        } else {
            writeln!(dump, "{response} // {title}").expect("string writes should not fail");
        }
        previous_multiline = multiline;
    }

    dump
}

#[cfg(test)]
mod tests {
    use super::MarkedText;

    #[test]
    fn strips_markers_without_reshaping_source_text() {
        let marked = MarkedText::parse(
            r#"
pub fn use_it(user: User) {
    let local = loc$goto$al;
    user.$0id();
    let escaped = "\$0 and \$name$";
}
"#,
        );

        assert!(marked.text().starts_with('\n'));
        assert!(marked.text().contains("let local = local;"));
        assert!(marked.text().contains("user.id();"));
        assert!(marked.text().contains(r#""$0 and $name$""#));
        assert_eq!(
            marked.offset("goto"),
            marked
                .text()
                .find("local;")
                .expect("clean local binding should be present")
                + 3
        );
        assert_eq!(
            marked.offset("0"),
            marked
                .text()
                .find("id();")
                .expect("clean method call should be present")
        );
    }
}
