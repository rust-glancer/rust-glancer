//! Small attribute subset retained for user-facing indexed searches.
//!
//! Resolution must keep every enabled definition, including compiler-internal public items.
//! Completion and symbol discovery need only two extra facts to avoid presenting those items as
//! ordinary APIs, so retaining the full attribute token trees would waste resident memory.

use rg_std::{MemorySize, Shrink};
use rg_syntax::{AstNode as _, ast};
use wincode::{SchemaRead, SchemaWrite};

/// Targeted declaration attributes that affect whether an item should be suggested to users.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink,
)]
#[memsize(leaf)]
#[shrink(leaf)]
pub struct UserFacingAttrs {
    doc_hidden: bool,
    unstable: bool,
}

impl UserFacingAttrs {
    pub fn is_doc_hidden(self) -> bool {
        self.doc_hidden
    }

    pub fn is_unstable(self) -> bool {
        self.unstable
    }

    pub fn from_ast(item: &dyn ast::HasAttrs) -> Self {
        let mut attrs = Self::default();

        for attr in item.attrs().filter(|attr| attr.kind().is_outer()) {
            let Some(meta) = attr.meta() else {
                continue;
            };
            match meta.simple_name().as_deref() {
                Some("unstable") => attrs.unstable = true,
                Some("doc") => {
                    let ast::Meta::TokenTreeMeta(meta) = meta else {
                        continue;
                    };
                    attrs.doc_hidden |= meta.token_tree().is_some_and(|tokens| {
                        tokens
                            .syntax()
                            .descendants_with_tokens()
                            .filter_map(|element| element.into_token())
                            .any(|token| {
                                token.kind().is_any_identifier() && token.text() == "hidden"
                            })
                    });
                }
                Some(_) | None => {}
            }
        }

        attrs
    }
}

#[cfg(test)]
mod tests {
    use rg_syntax::{Edition, SourceFile, ast, ast::HasModuleItem as _};

    use super::UserFacingAttrs;

    #[test]
    fn retains_only_user_facing_filter_attributes() {
        let source = r#"
            #[doc(hidden)]
            #[unstable(feature = "internal", issue = "none")]
            #[allow(dead_code)]
            pub struct Internal;
        "#;
        let file = SourceFile::parse(source, Edition::CURRENT)
            .ok()
            .expect("attribute fixture should parse");
        let item = file
            .items()
            .find_map(|item| match item {
                ast::Item::Struct(item) => Some(item),
                _ => None,
            })
            .expect("fixture should contain a struct");

        let attrs = UserFacingAttrs::from_ast(&item);
        assert!(attrs.is_doc_hidden());
        assert!(attrs.is_unstable());
    }

    #[test]
    fn ordinary_doc_text_does_not_become_doc_hidden() {
        let source = r#"#[doc = "mentions hidden"] pub struct Public;"#;
        let file = SourceFile::parse(source, Edition::CURRENT)
            .ok()
            .expect("attribute fixture should parse");
        let item = file
            .items()
            .find_map(|item| match item {
                ast::Item::Struct(item) => Some(item),
                _ => None,
            })
            .expect("fixture should contain a struct");

        assert_eq!(UserFacingAttrs::from_ast(&item), UserFacingAttrs::default());
    }
}
