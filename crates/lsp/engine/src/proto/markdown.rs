//! Markdown rendering helpers for LSP clients.

/// Render documentation comments as Markdown that regular LSP clients can display.
///
/// Rustdoc treats Rust code fences as examples and supports extra fence attributes plus
/// hidden setup lines. LSP clients mostly see ordinary Markdown, so keep the document text
/// intact while normalizing only the Rust example fences that need rustdoc-specific handling.
pub(crate) fn render_rustdoc_markdown(markdown: &str) -> Option<String> {
    let markdown = markdown.trim();
    (!markdown.is_empty()).then(|| RustdocMarkdown::new(markdown).render())
}

struct RustdocMarkdown<'a> {
    markdown: &'a str,
}

impl<'a> RustdocMarkdown<'a> {
    fn new(markdown: &'a str) -> Self {
        Self { markdown }
    }

    fn render(&self) -> String {
        let mut rendered = Vec::new();
        let mut open_fence: Option<MarkdownFence> = None;

        for line in self.markdown.lines() {
            if let Some(fence) = open_fence {
                // Once inside a fence, only the matching closing marker returns us to
                // regular Markdown. Non-Rust blocks pass through without inspection.
                if fence.closes_with(line) {
                    rendered.push(line.to_string());
                    open_fence = None;
                    continue;
                }

                if fence.is_rust
                    && let Some(line) = Self::rustdoc_example_line(line)
                {
                    rendered.push(line);
                } else if !fence.is_rust {
                    rendered.push(line.to_string());
                }
                continue;
            }

            // Outside code fences we preserve Markdown verbatim, except for Rust example
            // openings whose rustdoc attributes need to be hidden from the LSP client.
            if let Some(fence) = MarkdownFence::opening(line) {
                rendered.push(fence.render_opening(line));
                open_fence = Some(fence);
            } else {
                rendered.push(line.to_string());
            }
        }

        rendered.join("\n")
    }

    fn rustdoc_example_line(line: &str) -> Option<String> {
        // Rustdoc hides setup lines that begin with `#` inside Rust examples. Doubling the
        // marker escapes a visible leading hash, so `##[allow(...)]` renders as `#[allow(...)]`.
        let content_start = line
            .find(|ch| ch != ' ' && ch != '\t')
            .unwrap_or(line.len());
        let (indent, content) = line.split_at(content_start);

        if let Some(content) = content.strip_prefix("##") {
            return Some(format!("{indent}#{content}"));
        }

        (!content.starts_with('#')).then(|| line.to_string())
    }
}

#[derive(Debug, Clone, Copy)]
struct MarkdownFence {
    // The original marker shape is enough to find the close and keep long fences valid.
    marker: char,
    len: usize,
    is_rust: bool,
}

impl MarkdownFence {
    fn opening(line: &str) -> Option<Self> {
        let (_, rest) = Self::split_fence_indent(line)?;
        let marker = rest.chars().next()?;
        if marker != '`' && marker != '~' {
            return None;
        }

        let len = rest.chars().take_while(|ch| *ch == marker).count();
        if len < 3 {
            return None;
        }

        let info = rest[len..].trim();
        Some(Self {
            marker,
            len,
            is_rust: Self::is_rustdoc_rust_info(info),
        })
    }

    fn closes_with(self, line: &str) -> bool {
        let Some((_, rest)) = Self::split_fence_indent(line) else {
            return false;
        };
        let len = rest.chars().take_while(|ch| *ch == self.marker).count();
        len >= self.len && rest[len..].trim().is_empty()
    }

    fn render_opening(self, original: &str) -> String {
        if !self.is_rust {
            return original.to_string();
        }

        let indent_len = original
            .chars()
            .take_while(|ch| *ch == ' ')
            .map(char::len_utf8)
            .sum::<usize>();
        format!(
            "{}{}rust",
            &original[..indent_len],
            self.marker.to_string().repeat(self.len)
        )
    }

    fn split_fence_indent(line: &str) -> Option<(&str, &str)> {
        let indent_len = line
            .chars()
            .take_while(|ch| *ch == ' ')
            .map(char::len_utf8)
            .sum::<usize>();
        (indent_len <= 3).then(|| line.split_at(indent_len))
    }

    fn is_rustdoc_rust_info(info: &str) -> bool {
        // Rustdoc treats bare fences and modifier-only fences as Rust examples. Unknown
        // first attributes are left untouched so non-Rust Markdown keeps its original shape.
        if info.is_empty() {
            return true;
        }

        let Some(first_attribute) = info
            .split(|ch: char| ch == ',' || ch.is_whitespace())
            .find(|part| !part.is_empty())
        else {
            return true;
        };

        matches!(
            first_attribute,
            "rust"
                | "rs"
                | "allow_fail"
                | "compile_fail"
                | "edition2015"
                | "edition2018"
                | "edition2021"
                | "edition2024"
                | "ignore"
                | "no_crate_inject"
                | "no_run"
                | "should_panic"
                | "standalone_crate"
        )
    }
}

#[cfg(test)]
mod tests {
    use super::render_rustdoc_markdown;

    #[test]
    fn preserves_regular_markdown() {
        let rendered = render_rustdoc_markdown(
            r#"User account.

- Stores profile data.

```text
# this is visible text
```"#,
        );

        assert_eq!(
            rendered.as_deref(),
            Some(
                r#"User account.

- Stores profile data.

```text
# this is visible text
```"#
            )
        );
    }

    #[test]
    fn normalizes_rustdoc_rust_fences() {
        let rendered = render_rustdoc_markdown(
            r#"```rust,no_run
# use app::User;
let user = User::new();
##[allow(unused)]
```

```compile_fail
let value: u8 = "nope";
```"#,
        );

        assert_eq!(
            rendered.as_deref(),
            Some(
                r#"```rust
let user = User::new();
#[allow(unused)]
```

```rust
let value: u8 = "nope";
```"#
            )
        );
    }

    #[test]
    fn keeps_unknown_fences_untouched() {
        let rendered = render_rustdoc_markdown(
            r#"```mermaid
graph TD
# node comment
```"#,
        );

        assert_eq!(
            rendered.as_deref(),
            Some(
                r#"```mermaid
graph TD
# node comment
```"#
            )
        );
    }
}
