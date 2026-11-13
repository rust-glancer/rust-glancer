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

pub use self::{
    span::{Edition, Span, TokenOrigins, TokenOwner},
    tt::TopSubtree,
};

#[cfg(test)]
mod tests {
    use rg_syntax::{AstNode as _, ast};

    use crate::{
        Edition, TokenOrigins, TokenOwner, TopSubtree,
        syntax_bridge::{SpanFactory, TokenSourceMap, syntax_node_to_token_tree},
    };

    #[test]
    fn roundtrips_nested_tokens_with_distinct_symbols_and_extended_spans() {
        use crate::tt::{
            Delimiter, DelimiterKind, Ident, Leaf, LitKind, Literal, TopSubtreeBuilder,
        };

        let root =
            SpanFactory::new(0, 0, TokenOwner::Declaration(0), Edition::Edition2015).span_for(0);
        let mut builder = TopSubtreeBuilder::new(Delimiter::invisible_spanned(root));
        for index in 0..80 {
            let mut span = SpanFactory::new(
                index,
                index + 7,
                TokenOwner::Declaration(index),
                Edition::Edition2024,
            )
            .span_for(0x100_0000 + index * 100);
            // Reordered endpoints and synthetic origins must survive the compact span codec too.
            span.origins = match index % 4 {
                0 => span.origins.join(TokenOrigins::single(index)),
                1 => TokenOrigins::synthetic(span.origins.first()),
                2 => span
                    .origins
                    .join(TokenOrigins::single(span.origins.first() - 1)),
                _ => span.origins,
            };
            builder.open(DelimiterKind::Bracket, span);
            builder.push(Leaf::Ident(Ident::new(&format!("r#field_{index}"), span)));
            builder.push(Leaf::Literal(Literal::new(
                "42",
                span,
                LitKind::Integer,
                "usize",
            )));
            builder.push(Leaf::Literal(Literal::new_no_suffix(
                "café",
                span,
                LitKind::StrRaw(3),
            )));
            builder.close(span);
        }
        let tree = builder.build();
        let bytes = wincode::serialize(&tree).expect("nested tokens should serialize");
        let mut decoded: TopSubtree =
            wincode::deserialize(&bytes).expect("nested tokens should load");
        assert_eq!(
            tree.as_token_trees().iter_flat_tokens().collect::<Vec<_>>(),
            decoded
                .as_token_trees()
                .iter_flat_tokens()
                .collect::<Vec<_>>(),
        );

        decoded.set_top_subtree_delimiter_kind(DelimiterKind::Brace);
        decoded.set_top_subtree_delimiter_span(crate::tt::DelimSpan::from_single(root));
        assert_eq!(decoded.top_subtree().delimiter.kind, DelimiterKind::Brace);
        assert_eq!(
            tree.token_trees().iter_flat_tokens().collect::<Vec<_>>(),
            decoded.token_trees().iter_flat_tokens().collect::<Vec<_>>(),
            "changing the root delimiter must preserve every nested token and source span",
        );
    }

    #[test]
    fn preserves_delimiter_origins_through_compression_and_serialization() {
        use crate::tt::{Delimiter, DelimiterKind, Subtree, TokenTree};

        let root = SpanFactory::new(0, 0, TokenOwner::Declaration(0), Edition::CURRENT).span_for(0);
        let reversed = TokenOrigins::single(2).join(TokenOrigins::single(1));
        for (open, close) in [
            (TokenOrigins::synthetic(1), reversed),
            (reversed, TokenOrigins::synthetic(3)),
            (TokenOrigins::single(1), TokenOrigins::single(2)),
            (TokenOrigins::synthetic(1), TokenOrigins::synthetic(2)),
        ] {
            let expected = vec![
                TokenTree::Subtree(Subtree {
                    delimiter: Delimiter::invisible_spanned(root),
                    len: 1,
                }),
                TokenTree::Subtree(Subtree {
                    delimiter: Delimiter {
                        open: crate::Span {
                            origins: open,
                            ..root
                        },
                        close: crate::Span {
                            origins: close,
                            ..root
                        },
                        kind: DelimiterKind::Bracket,
                    },
                    len: 0,
                }),
            ];
            let tree = TopSubtree::from_serialized(expected.clone());
            assert_eq!(
                tree.as_token_trees().iter_flat_tokens().collect::<Vec<_>>(),
                expected
            );
            let bytes = wincode::serialize(&tree).expect("tokens serialize");
            let decoded: TopSubtree = wincode::deserialize(&bytes).expect("tokens deserialize");
            assert_eq!(
                decoded
                    .as_token_trees()
                    .iter_flat_tokens()
                    .collect::<Vec<_>>(),
                expected
            );
        }
    }

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

        let subtree = syntax_node_to_token_tree(
            &body,
            SpanFactory::new(0, 0, TokenOwner::Declaration(0), Edition::CURRENT),
            &mut TokenSourceMap::default(),
        );
        let bytes = wincode::serialize(&subtree).expect("top subtree should serialize");
        let decoded: TopSubtree =
            wincode::deserialize(&bytes).expect("top subtree should deserialize");

        assert_eq!(subtree, decoded);
    }
}
