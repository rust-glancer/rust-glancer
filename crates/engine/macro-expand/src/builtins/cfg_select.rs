//! Small builtin expander for item-position `cfg_select!` calls.
//!
//! Rust accepts `cfg_select!` in expression and item positions. The macro engine exposes the item
//! form needed by def-map: pick the first active cfg arm, strip the arm braces, and parse the
//! resulting item stream.

use rg_cfg_eval::{CfgEvaluator, CfgPredicate};
use rg_tt::{
    TopSubtree,
    tt::{self, Delimiter, DelimiterKind, Leaf, TopSubtreeBuilder, TtElement, TtIter},
};

use crate::ExpansionSyntax;

/// Expands the supported item-position `cfg_select! { predicate => { ... } }` form.
pub fn expand(args: &TopSubtree, cfg: &CfgEvaluator<'_>) -> Option<ExpansionSyntax> {
    let mut parser = CfgSelectParser::new(args.view().token_trees().iter());
    parser
        .select_payload(cfg)
        .map(ExpansionSyntax::from_token_tree)
}

struct CfgSelectParser<'a> {
    iter: TtIter<'a>,
}

impl<'a> CfgSelectParser<'a> {
    fn new(iter: TtIter<'a>) -> Self {
        Self { iter }
    }

    fn select_payload(&mut self, cfg: &CfgEvaluator<'_>) -> Option<TopSubtree> {
        while !self.iter.is_empty() {
            // Each arm starts with ordinary cfg predicate syntax, or `_` for the fallback arm.
            let predicate = self.parse_arm_predicate()?;
            self.expect_arrow()?;

            // Item-position `cfg_select!` strips the selected arm's braces before parsing the
            // generated items.
            let payload = self.parse_braced_payload()?;

            if cfg.is_predicate_enabled(&predicate) {
                return Some(payload);
            }

            self.eat_optional_comma();
        }

        None
    }

    fn parse_arm_predicate(&mut self) -> Option<CfgPredicate> {
        let ident = self.iter.expect_ident_or_underscore().ok()?;
        match ident.sym.as_str() {
            // `_` is `cfg_select!`-specific fallback syntax. Once parsed, it behaves like an
            // always-enabled predicate.
            "_" => Some(CfgPredicate::True),
            "true" => Some(CfgPredicate::True),
            "false" => Some(CfgPredicate::False),
            "all" | "any" | "not" => self.parse_composite_predicate(ident.sym.as_str()),
            key if self.peek_key_value_equals() => {
                self.iter.expect_char('=').ok()?;
                let value = self.parse_string_literal()?;
                Some(CfgPredicate::KeyValue {
                    key: key.to_string(),
                    value,
                })
            }
            atom => Some(CfgPredicate::Atom(atom.to_string())),
        }
    }

    fn parse_composite_predicate(&mut self, keyword: &str) -> Option<CfgPredicate> {
        let (subtree, inner) = self.iter.expect_subtree().ok()?;
        if subtree.delimiter.kind != DelimiterKind::Parenthesis {
            return None;
        }

        let predicates = Self::parse_predicate_list(inner)?;
        let predicate = match keyword {
            "all" => CfgPredicate::All(predicates),
            "any" => CfgPredicate::Any(predicates),
            "not" => CfgPredicate::Not(predicates),
            _ => return None,
        };
        Some(predicate)
    }

    fn parse_predicate_list(mut iter: TtIter<'a>) -> Option<Vec<CfgPredicate>> {
        let mut predicates = Vec::new();

        while !iter.is_empty() {
            let mut parser = CfgSelectParser::new(iter);
            predicates.push(parser.parse_arm_predicate()?);
            if !parser.iter.is_empty() {
                parser.iter.expect_comma().ok()?;
            }
            iter = parser.iter;
        }

        Some(predicates)
    }

    fn parse_string_literal(&mut self) -> Option<String> {
        let Leaf::Literal(literal) = self.iter.expect_leaf().ok()? else {
            return None;
        };

        // `tt::Literal` already stores string literal payloads without quotes or raw-string hashes.
        matches!(literal.kind, tt::LitKind::Str | tt::LitKind::StrRaw(_))
            .then(|| literal.text().to_string())
    }

    fn parse_braced_payload(&mut self) -> Option<TopSubtree> {
        let (subtree, inner) = self.iter.expect_subtree().ok()?;
        if subtree.delimiter.kind != DelimiterKind::Brace {
            return None;
        }

        // Use an invisible delimiter so the selected arm behaves as a plain item stream while
        // keeping the arm span available for coarse diagnostics.
        let mut builder = TopSubtreeBuilder::new(Delimiter::invisible_delim_spanned(
            subtree.delimiter.delim_span(),
        ));
        builder.extend_with_tt(inner.remaining());
        Some(builder.build())
    }

    fn expect_arrow(&mut self) -> Option<()> {
        self.iter.expect_char('=').ok()?;
        self.iter.expect_char('>').ok()
    }

    fn eat_optional_comma(&mut self) {
        if matches!(
            self.iter.peek(),
            Some(TtElement::Leaf(Leaf::Punct(tt::Punct { char: ',', .. })))
        ) {
            let _ = self.iter.next();
        }
    }

    fn peek_key_value_equals(&self) -> bool {
        let mut lookahead = self.iter.clone();
        if lookahead.expect_char('=').is_err() {
            return false;
        }
        !matches!(
            lookahead.peek(),
            Some(TtElement::Leaf(Leaf::Punct(tt::Punct { char: '>', .. })))
        )
    }
}

#[cfg(test)]
mod tests {
    use rg_cfg_eval::{CfgEvaluator, CfgOptions};
    use rg_syntax::{AstNode as _, ast};
    use rg_tt::syntax_bridge::{SpanFactory, syntax_node_to_token_tree};

    use crate::Edition;

    #[test]
    fn expands_first_enabled_item_arm() {
        let mut options = CfgOptions::default();
        options.insert_atom("unix");

        let expanded = expand_fixture(
            r#"
cfg_select! {
    unix => { pub struct Unix; },
    _ => { pub struct Other; },
}
"#,
            &options,
        );

        assert_eq!(
            expanded.parse.syntax_node().text().to_string(),
            "pub struct Unix ;"
        );
    }

    #[test]
    fn expands_wildcard_fallback() {
        let expanded = expand_fixture(
            r#"
cfg_select! {
    windows => { pub struct Windows; },
    _ => { pub struct Other; },
}
"#,
            &CfgOptions::default(),
        );

        assert_eq!(
            expanded.parse.syntax_node().text().to_string(),
            "pub struct Other ;"
        );
    }

    fn expand_fixture(source: &str, options: &CfgOptions) -> crate::ExpansionSyntax {
        let file = ast::SourceFile::parse(source, Edition::CURRENT)
            .ok()
            .expect("test source should parse");
        let call = file
            .syntax()
            .descendants()
            .find_map(ast::MacroCall::cast)
            .expect("test source should contain a macro call");
        let args = call
            .token_tree()
            .expect("macro call fixture should have arguments");
        let args = syntax_node_to_token_tree(&args, SpanFactory::new(0, Edition::CURRENT));
        let cfg = CfgEvaluator::new(options, false);

        super::expand(&args, &cfg).expect("cfg_select fixture should expand")
    }
}
