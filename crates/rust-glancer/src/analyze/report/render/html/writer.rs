//! Small HTML writer used by the report renderer.
//!
//! It escapes text and attributes by default. Raw HTML is only used for the fixed page shell,
//! stylesheet, and script.

/// Accumulates HTML into a string.
pub(super) struct HtmlWriter {
    html: String,
}

impl HtmlWriter {
    pub(super) fn new() -> Self {
        Self {
            html: String::new(),
        }
    }

    pub(super) fn finish(self) -> String {
        self.html
    }

    pub(super) fn raw(&mut self, html: &str) {
        self.html.push_str(html);
    }

    pub(super) fn text(&mut self, text: &str) {
        self.write_escaped(text);
    }

    pub(super) fn element(&mut self, name: &'static str) -> HtmlElement<'_> {
        HtmlElement {
            writer: self,
            name,
            attrs: Vec::new(),
            classes: Vec::new(),
        }
    }

    fn write_open_tag(&mut self, name: &str, attrs: &[(String, String)], classes: &[String]) {
        self.html.push('<');
        self.html.push_str(name);

        if !classes.is_empty() {
            self.html.push_str(" class=\"");
            for (index, class) in classes.iter().enumerate() {
                if index > 0 {
                    self.html.push(' ');
                }
                self.write_escaped(class);
            }
            self.html.push('"');
        }

        for (name, value) in attrs {
            self.html.push(' ');
            self.html.push_str(name);
            self.html.push_str("=\"");
            self.write_escaped(value);
            self.html.push('"');
        }

        self.html.push('>');
    }

    fn write_close_tag(&mut self, name: &str) {
        self.html.push_str("</");
        self.html.push_str(name);
        self.html.push('>');
    }

    fn write_escaped(&mut self, text: &str) {
        for character in text.chars() {
            match character {
                '&' => self.html.push_str("&amp;"),
                '<' => self.html.push_str("&lt;"),
                '>' => self.html.push_str("&gt;"),
                '"' => self.html.push_str("&quot;"),
                '\'' => self.html.push_str("&#39;"),
                _ => self.html.push(character),
            }
        }
    }
}

/// Pending HTML element.
///
/// The element is written only when one of `empty`, `text`, or `children` is called. That keeps
/// attributes and classes easy to chain.
pub(super) struct HtmlElement<'writer> {
    writer: &'writer mut HtmlWriter,
    name: &'static str,
    attrs: Vec<(String, String)>,
    classes: Vec<String>,
}

impl HtmlElement<'_> {
    pub(super) fn attr(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.attrs.push((name.into(), value.into()));
        self
    }

    pub(super) fn class(mut self, class: impl Into<String>) -> Self {
        let class = class.into();
        if !class.is_empty() {
            self.classes.push(class);
        }
        self
    }

    pub(super) fn empty(self) {
        self.writer
            .write_open_tag(self.name, &self.attrs, &self.classes);
        self.writer.raw("\n");
    }

    pub(super) fn text(self, text: &str) {
        self.writer
            .write_open_tag(self.name, &self.attrs, &self.classes);
        self.writer.write_escaped(text);
        self.writer.write_close_tag(self.name);
        self.writer.raw("\n");
    }

    pub(super) fn children<R>(self, render: impl FnOnce(&mut HtmlWriter) -> R) -> R {
        self.writer
            .write_open_tag(self.name, &self.attrs, &self.classes);
        self.writer.raw("\n");
        let result = render(self.writer);
        self.writer.write_close_tag(self.name);
        self.writer.raw("\n");
        result
    }
}
