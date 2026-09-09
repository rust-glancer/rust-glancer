use crate::item::{Documentation, DocumentationPlacement, DocumentationSource};
use rg_syntax::ast;

use super::MaybeFromAst;

pub struct OuterDocs;
pub struct InnerDocs;

impl MaybeFromAst<OuterDocs> for Documentation {
    type AstNode = dyn ast::HasDocComments;
    type Context<'a> = OuterDocs;

    fn maybe_from_ast(item: &Self::AstNode, _ctx: Self::Context<'_>) -> Option<Self> {
        DocumentationSource::from_node(item.syntax(), DocumentationPlacement::Outer)
            .into_documentation()
    }
}

impl MaybeFromAst<InnerDocs> for Documentation {
    type AstNode = dyn ast::HasAttrs;
    type Context<'a> = InnerDocs;

    fn maybe_from_ast(item: &Self::AstNode, _ctx: Self::Context<'_>) -> Option<Self> {
        DocumentationSource::from_node(
            &item.inner_attributes_node()?,
            DocumentationPlacement::Inner,
        )
        .into_documentation()
    }
}

#[cfg(test)]
mod tests {
    use crate::item::Documentation;
    use rg_syntax::{AstNode as _, Edition, SourceFile, ast};

    use super::{InnerDocs, OuterDocs};
    use crate::item::MaybeFromAst;

    #[test]
    fn extracts_outer_docs_in_source_order() {
        let file = SourceFile::parse(
            r#"
            /// User account.
            #[doc = "Stores the display name."]
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
