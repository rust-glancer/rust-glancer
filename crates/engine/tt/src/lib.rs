//! Token-tree primitives used by declarative macro expansion.
//!
//! The core token-tree representation is adapted from rust-analyzer. rust-glancer keeps it in a
//! shared crate because macro definitions are lowered in item-tree, expanded by `rg_macro_expand`,
//! and collected by def-map after parsing the generated token stream back into syntax.

extern crate ra_ap_rustc_lexer as rustc_lexer;

pub mod span;
pub mod syntax_bridge;
pub mod tt;
mod wincode_adapters;

pub use span::{Edition, Span};
pub use tt::TopSubtree;

#[cfg(test)]
mod tests {
    use rg_syntax::{AstNode as _, ast};

    use crate::{
        Edition, TopSubtree,
        syntax_bridge::{SpanFactory, syntax_node_to_token_tree},
    };

    #[test]
    fn roundtrips_top_subtree_through_wincode() {
        let file = ast::SourceFile::parse(
            r#"
macro_rules! make {
    ($path:path) => { pub use $path::Item; };
}
"#,
            Edition::CURRENT,
        )
        .ok()
        .expect("fixture should parse");
        let body = file
            .syntax()
            .descendants()
            .find_map(ast::MacroRules::cast)
            .and_then(|macro_rules| macro_rules.token_tree())
            .expect("fixture should contain macro_rules body");

        let subtree = syntax_node_to_token_tree(&body, SpanFactory::new(0, Edition::CURRENT));
        let bytes = wincode::serialize(&subtree).expect("top subtree should serialize");
        let decoded: TopSubtree =
            wincode::deserialize(&bytes).expect("top subtree should deserialize");

        assert_eq!(subtree, decoded);
    }
}
