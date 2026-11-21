//! Small attribute subset retained for user-facing indexed searches.
//!
//! Resolution must keep every enabled definition, including compiler-internal public items.
//! Completion and symbol discovery keep a few flags to avoid presenting those items as
//! ordinary APIs, so retaining the full attribute token trees would waste resident memory.

use rg_ir_model::CrateRef;
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
    doc_inline: bool,
    unstable: bool,
}

impl UserFacingAttrs {
    pub fn is_doc_hidden(self) -> bool {
        self.doc_hidden
    }

    pub fn is_unstable(self) -> bool {
        self.unstable
    }

    pub fn is_doc_inline(self) -> bool {
        self.doc_inline
    }

    pub fn is_suggestible_from(self, defining_crate: CrateRef, use_crate: CrateRef) -> bool {
        defining_crate == use_crate || !self.is_restricted()
    }

    pub fn is_restricted(self) -> bool {
        self.doc_hidden || self.unstable
    }

    /// Check both the declaration and the spelling that exposed it. An explicit
    /// `doc(inline)` can expose a hidden definition, just as rustdoc does.
    pub fn is_suggestible_through(
        self,
        defining_crate: CrateRef,
        import: Option<(CrateRef, Self)>,
        use_crate: CrateRef,
    ) -> bool {
        let Some((import_crate, attrs)) = import else {
            return self.is_suggestible_from(defining_crate, use_crate);
        };
        attrs.is_suggestible_from(import_crate, use_crate)
            && (self.is_suggestible_from(defining_crate, use_crate)
                || attrs.doc_inline && !self.unstable)
    }

    /// Combine attributes on two parts of one declaration, such as a module
    /// declaration and the inner attributes in its file.
    pub fn merge(self, source: Self) -> Self {
        Self {
            doc_hidden: self.doc_hidden || source.doc_hidden,
            doc_inline: self.doc_inline || source.doc_inline,
            unstable: self.unstable || source.unstable,
        }
    }

    pub fn from_ast(item: &dyn ast::HasAttrs) -> Self {
        let mut attrs = Self::default();

        // TODO: Evaluate presentation attributes introduced by target-specific cfg_attr.
        for attr in ast::attrs_including_inner(item) {
            let Some(meta) = attr.meta() else {
                continue;
            };
            match meta.simple_name().as_deref() {
                Some("unstable") => attrs.unstable = true,
                Some("doc") => {
                    let ast::Meta::TokenTreeMeta(meta) = meta else {
                        continue;
                    };
                    let Some(tokens) = meta.token_tree() else {
                        continue;
                    };
                    // Only standalone outer doc arguments count. For example,
                    // the identifier in `doc(cfg(hidden))` is a cfg atom.
                    let mut argument = Vec::new();
                    for element in tokens.syntax().children_with_tokens() {
                        match element.as_token().map(|token| token.kind()) {
                            Some(
                                rg_syntax::SyntaxKind::L_PAREN | rg_syntax::SyntaxKind::R_PAREN,
                            ) => {}
                            Some(rg_syntax::SyntaxKind::COMMA) => {
                                attrs.record_doc_argument(&argument);
                                argument.clear();
                            }
                            Some(kind) if kind.is_trivia() => {}
                            _ => argument.push(element),
                        }
                    }
                    attrs.record_doc_argument(&argument);
                }
                Some(_) | None => {}
            }
        }

        attrs
    }

    fn record_doc_argument(&mut self, argument: &[rg_syntax::SyntaxElement]) {
        if let [element] = argument
            && let Some(token) = element.as_token()
        {
            match token.text() {
                "hidden" => self.doc_hidden = true,
                "inline" => self.doc_inline = true,
                _ => {}
            }
        }
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
