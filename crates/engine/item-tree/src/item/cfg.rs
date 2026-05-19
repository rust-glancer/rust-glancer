//! Target-independent representation of item-level cfg attributes.
//!
//! The item tree stores cfg gates without evaluating them so shared syntax can be reused by
//! multiple targets. Target-specific phases later decide whether an item exists in their cfg
//! environment.

use rg_syntax::{
    SyntaxToken,
    ast::{self, HasAttrs},
};

/// Item-level cfg gates that later target-specific phases evaluate.
#[derive(Debug, Clone, PartialEq, Eq, Default, wincode::SchemaRead, wincode::SchemaWrite)]
pub struct CfgExpr {
    pub gates: Vec<CfgGate>,
}

impl CfgExpr {
    /// Extracts only the cfg attributes that can decide whether an item exists.
    pub fn from_attrs(item: &impl HasAttrs) -> Self {
        let mut expr = Self::default();

        for attr in item.attrs().filter(|attr| attr.kind().is_outer()) {
            match attr.meta() {
                Some(ast::Meta::CfgMeta(meta)) => {
                    let predicate = meta
                        .cfg_predicate()
                        .map(CfgPredicate::from_ast)
                        .unwrap_or(CfgPredicate::Invalid);
                    expr.gates.push(CfgGate::Direct(predicate));
                }
                Some(ast::Meta::CfgAttrMeta(meta)) => {
                    let predicate = meta
                        .cfg_predicate()
                        .map(CfgPredicate::from_ast)
                        .unwrap_or(CfgPredicate::Invalid);

                    // Only cfg-bearing attributes change whether the item exists. Other attrs
                    // exposed by cfg_attr are left to later attribute-aware features.
                    for nested in meta.metas() {
                        if let ast::Meta::CfgMeta(nested) = nested {
                            let cfg = nested
                                .cfg_predicate()
                                .map(CfgPredicate::from_ast)
                                .unwrap_or(CfgPredicate::Invalid);
                            expr.gates.push(CfgGate::CfgAttr {
                                predicate: predicate.clone(),
                                cfg,
                            });
                        }
                    }
                }
                _ => {}
            }
        }

        expr
    }

    pub(crate) fn shrink_to_fit(&mut self) {
        self.gates.shrink_to_fit();
        for gate in &mut self.gates {
            gate.shrink_to_fit();
        }
    }
}

/// One top-level gate that can make an item unavailable.
#[derive(Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite)]
pub enum CfgGate {
    /// A direct `#[cfg(...)]` item attribute.
    Direct(CfgPredicate),
    /// A `#[cfg_attr(predicate, cfg(...))]` attribute that may activate a nested cfg gate.
    CfgAttr {
        predicate: CfgPredicate,
        cfg: CfgPredicate,
    },
}

impl CfgGate {
    fn shrink_to_fit(&mut self) {
        match self {
            Self::Direct(predicate) => predicate.shrink_to_fit(),
            Self::CfgAttr { predicate, cfg } => {
                predicate.shrink_to_fit();
                cfg.shrink_to_fit();
            }
        }
    }
}

/// Lowered cfg predicate syntax used by target-specific evaluators.
#[derive(Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite)]
pub enum CfgPredicate {
    True,
    False,
    Atom(String),
    KeyValue { key: String, value: String },
    All(Vec<CfgPredicate>),
    Any(Vec<CfgPredicate>),
    Not(Vec<CfgPredicate>),
    Invalid,
}

impl CfgPredicate {
    fn from_ast(predicate: ast::CfgPredicate) -> Self {
        match predicate {
            ast::CfgPredicate::CfgAtom(atom) => Self::from_atom(atom),
            ast::CfgPredicate::CfgComposite(composite) => Self::from_composite(composite),
        }
    }

    fn from_atom(atom: ast::CfgAtom) -> Self {
        if atom.true_token().is_some() {
            return Self::True;
        }
        if atom.false_token().is_some() {
            return Self::False;
        }

        let Some(key) = atom.ident_token().map(|token| token.text().to_string()) else {
            return Self::Invalid;
        };

        if atom.eq_token().is_some() {
            return match atom.string_token().and_then(string_token_value) {
                Some(value) => Self::KeyValue { key, value },
                None => Self::Invalid,
            };
        }

        Self::Atom(key)
    }

    fn from_composite(composite: ast::CfgComposite) -> Self {
        let predicates = composite
            .cfg_predicates()
            .map(Self::from_ast)
            .collect::<Vec<_>>();
        match composite.keyword().as_ref().map(SyntaxToken::text) {
            Some("all") => Self::All(predicates),
            Some("any") => Self::Any(predicates),
            Some("not") => Self::Not(predicates),
            _ => Self::Invalid,
        }
    }

    fn shrink_to_fit(&mut self) {
        match self {
            Self::Atom(atom) => atom.shrink_to_fit(),
            Self::KeyValue { key, value } => {
                key.shrink_to_fit();
                value.shrink_to_fit();
            }
            Self::All(predicates) | Self::Any(predicates) | Self::Not(predicates) => {
                predicates.shrink_to_fit();
                for predicate in predicates {
                    predicate.shrink_to_fit();
                }
            }
            Self::True | Self::False | Self::Invalid => {}
        }
    }
}

fn string_token_value(token: SyntaxToken) -> Option<String> {
    let text = token.text();
    if let Some(value) = text
        .strip_prefix('"')
        .and_then(|text| text.strip_suffix('"'))
    {
        return Some(value.to_string());
    }

    // Error-tolerant syntax can preserve surrounding token trivia. Pull out the quoted payload if
    // the token still contains a recognizable string literal.
    let first_quote = text.find('"')?;
    let last_quote = text.rfind('"')?;
    (first_quote < last_quote).then(|| text[first_quote + 1..last_quote].to_string())
}
