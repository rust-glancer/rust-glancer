use rg_ir_model::items::Documentation;
use rg_syntax::{AstNode as _, ast};

use super::MaybeFromAst;

pub struct OuterDocs;
pub struct InnerDocs;

impl MaybeFromAst<OuterDocs> for Documentation {
    type AstNode = dyn ast::HasDocComments;
    type Context<'a> = OuterDocs;

    fn maybe_from_ast(item: &Self::AstNode, _ctx: Self::Context<'_>) -> Option<Self> {
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
}

impl MaybeFromAst<InnerDocs> for Documentation {
    type AstNode = dyn ast::HasAttrs;
    type Context<'a> = InnerDocs;

    fn maybe_from_ast(item: &Self::AstNode, _ctx: Self::Context<'_>) -> Option<Self> {
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
    use rg_ir_model::items::Documentation;
    use rg_syntax::{AstNode as _, Edition, SourceFile, ast};

    use super::{InnerDocs, OuterDocs};
    use crate::item::MaybeFromAst;

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
            .expect("fixture should contain struct");

        let docs = <Documentation as MaybeFromAst<OuterDocs>>::maybe_from_ast(&item, OuterDocs)
            .expect("docs should be extracted");

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
            .expect("fixture should contain struct");

        let docs = <Documentation as MaybeFromAst<OuterDocs>>::maybe_from_ast(&item, OuterDocs)
            .expect("docs should be extracted");

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

        let docs = <Documentation as MaybeFromAst<InnerDocs>>::maybe_from_ast(&file, InnerDocs)
            .expect("docs should be extracted");

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
            .expect("fixture should contain module");

        let docs = <Documentation as MaybeFromAst<InnerDocs>>::maybe_from_ast(&item, InnerDocs)
            .expect("docs should be extracted");

        assert_eq!(docs.as_str(), "Inline module overview.");
    }
}
