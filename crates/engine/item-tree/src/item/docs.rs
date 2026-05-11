use ra_syntax::{
    AstNode as _,
    ast::{self},
};

/// User-facing documentation attached to one source declaration.
///
/// The text is already stripped from Rust doc-comment/doc-attribute syntax, but otherwise remains
/// Markdown-like so editor features can render it without re-reading AST.
#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct Documentation {
    pub(crate) text: String,
}

impl Documentation {
    pub fn new(text: impl Into<String>) -> Option<Self> {
        let text = text.into();
        (!text.trim().is_empty()).then_some(Self { text })
    }

    pub fn concat(first: Option<Self>, second: Option<Self>) -> Option<Self> {
        let mut parts = Vec::new();
        if let Some(first) = first {
            parts.push(first.text);
        }
        if let Some(second) = second {
            parts.push(second.text);
        }

        Self::new(parts.join("\n"))
    }

    pub fn from_ast<T>(item: &T) -> Option<Self>
    where
        T: ast::HasDocComments,
    {
        let mut lines = Vec::new();

        for comment in item.doc_comments().filter(ast::Comment::is_outer) {
            if let Some((text, _)) = comment.doc_comment() {
                lines.push(normalize_doc_text(text));
            }
        }

        for attr in item.attrs().filter(|attr| attr.kind().is_outer()) {
            if let Some(text) = doc_attr_text(&attr) {
                lines.push(text);
            }
        }

        Self::new(lines.join("\n"))
    }

    pub fn inner_from_ast<T>(item: &T) -> Option<Self>
    where
        T: ast::HasAttrs,
    {
        let inner_node = item.inner_attributes_node()?;
        let mut lines = Vec::new();

        // Inner module docs live on the module body (`mod foo { //! ... }`) or file itself
        // (`//! ...`). They document the containing module rather than the next item.
        for comment in
            ast::DocCommentIter::from_syntax_node(&inner_node).filter(ast::Comment::is_inner)
        {
            if let Some((text, _)) = comment.doc_comment() {
                lines.push(normalize_doc_text(text));
            }
        }

        for attr in inner_node.children().filter_map(ast::Attr::cast) {
            if attr.kind().is_inner()
                && let Some(text) = doc_attr_text(&attr)
            {
                lines.push(text);
            }
        }

        Self::new(lines.join("\n"))
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }

    pub fn shrink_to_fit(&mut self) {
        self.text.shrink_to_fit();
    }
}

fn doc_attr_text(attr: &ast::Attr) -> Option<String> {
    let ast::Meta::KeyValueMeta(meta) = attr.meta()? else {
        return None;
    };
    let path = meta.path()?;
    if path.syntax().text() != "doc" {
        return None;
    }

    let ast::Expr::Literal(literal) = meta.expr()? else {
        return None;
    };
    let ast::LiteralKind::String(value) = literal.kind() else {
        return None;
    };

    value.value().ok().map(|value| normalize_doc_text(&value))
}

fn normalize_doc_text(text: &str) -> String {
    text.lines()
        .map(|line| line.strip_prefix(' ').unwrap_or(line))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use ra_syntax::{AstNode as _, Edition, SourceFile, ast};

    use super::Documentation;

    #[test]
    fn extracts_line_doc_comments() {
        let file = SourceFile::parse(
            r#"
            /// User account.
            /// Stores the display name.
            struct User;
            "#,
            Edition::CURRENT,
        )
        .ok()
        .expect("fixture should parse");
        let item = file
            .syntax()
            .descendants()
            .find_map(ast::Struct::cast)
            .unwrap();

        let docs = Documentation::from_ast(&item).expect("docs should be extracted");

        assert_eq!(docs.as_str(), "User account.\nStores the display name.");
    }

    #[test]
    fn extracts_doc_attributes() {
        let file = SourceFile::parse(
            r#"
            #[doc = "User account."]
            struct User;
            "#,
            Edition::CURRENT,
        )
        .ok()
        .expect("fixture should parse");
        let item = file
            .syntax()
            .descendants()
            .find_map(ast::Struct::cast)
            .unwrap();

        let docs = Documentation::from_ast(&item).expect("docs should be extracted");

        assert_eq!(docs.as_str(), "User account.");
    }

    #[test]
    fn extracts_inner_file_docs() {
        let file = SourceFile::parse(
            r#"
            //! Module overview.
            #![doc = "More module details."]
            struct User;
            "#,
            Edition::CURRENT,
        )
        .ok()
        .expect("fixture should parse");

        let docs = Documentation::inner_from_ast(&file).expect("docs should be extracted");

        assert_eq!(docs.as_str(), "Module overview.\nMore module details.");
    }

    #[test]
    fn extracts_inner_inline_module_docs() {
        let file = SourceFile::parse(
            r#"
            mod api {
                //! Inline module overview.
                pub struct User;
            }
            "#,
            Edition::CURRENT,
        )
        .ok()
        .expect("fixture should parse");
        let item = file
            .syntax()
            .descendants()
            .find_map(ast::Module::cast)
            .unwrap();

        let docs = Documentation::inner_from_ast(&item).expect("docs should be extracted");

        assert_eq!(docs.as_str(), "Inline module overview.");
    }
}
