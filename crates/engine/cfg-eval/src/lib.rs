//! Lowered cfg predicates and target-specific cfg evaluation.
//!
//! Syntax lowering, workspace metadata, macro expansion, and def-map collection all need to talk
//! about the same cfg language. This crate owns that small domain so those phases can share
//! predicates and options without routing through item-tree or def-map internals.

use rg_std::{MemoryRecorder, MemorySize, Shrink};
use rg_syntax::{
    SyntaxToken,
    ast::{self, HasAttrs},
};
use wincode::{SchemaRead, SchemaWrite};

/// Active cfg facts for one package under one Cargo target selection.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Shrink)]
pub struct CfgOptions {
    atoms: Vec<String>,
    key_values: Vec<CfgKeyValue>,
}

impl CfgOptions {
    /// Builds cfg options for the host process when Cargo metadata is supplied without a target.
    pub fn current_host() -> Self {
        let mut options = Self::default();

        if cfg!(unix) {
            options.insert_atom("unix");
        }
        if cfg!(windows) {
            options.insert_atom("windows");
        }

        options.insert_key_value("target_arch", std::env::consts::ARCH);
        options.insert_key_value("target_os", std::env::consts::OS);
        options.insert_key_value("target_family", std::env::consts::FAMILY);
        options.insert_key_value("target_pointer_width", usize::BITS.to_string());

        #[cfg(target_env = "gnu")]
        options.insert_key_value("target_env", "gnu");
        #[cfg(target_env = "msvc")]
        options.insert_key_value("target_env", "msvc");
        #[cfg(target_env = "musl")]
        options.insert_key_value("target_env", "musl");
        #[cfg(target_env = "")]
        options.insert_key_value("target_env", "");

        #[cfg(target_vendor = "apple")]
        options.insert_key_value("target_vendor", "apple");
        #[cfg(target_vendor = "pc")]
        options.insert_key_value("target_vendor", "pc");
        #[cfg(target_vendor = "unknown")]
        options.insert_key_value("target_vendor", "unknown");

        options
    }

    /// Parses the line-oriented output of `rustc --print cfg`.
    pub fn from_rustc_cfg_output(output: &str) -> Self {
        let mut options = Self::default();

        for line in output
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
        {
            if let Some((key, value)) = line.split_once('=') {
                options.insert_key_value(key, cfg_value_from_rustc(value));
            } else {
                options.insert_atom(line);
            }
        }

        options
    }

    pub fn insert_atom(&mut self, atom: impl Into<String>) {
        let atom = atom.into();
        if !self.atoms.contains(&atom) {
            self.atoms.push(atom);
        }
    }

    pub fn insert_key_value(&mut self, key: impl Into<String>, value: impl Into<String>) {
        let key_value = CfgKeyValue {
            key: key.into(),
            value: value.into(),
        };
        if !self.key_values.contains(&key_value) {
            self.key_values.push(key_value);
        }
    }

    pub fn contains_atom(&self, atom: &str) -> bool {
        self.atoms.iter().any(|known| known == atom)
    }

    pub fn contains_key_value(&self, key: &str, value: &str) -> bool {
        self.key_values
            .iter()
            .any(|known| known.key == key && known.value == value)
    }

    pub fn atoms(&self) -> &[String] {
        &self.atoms
    }

    pub fn key_values(&self) -> &[CfgKeyValue] {
        &self.key_values
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Shrink)]
pub struct CfgKeyValue {
    key: String,
    value: String,
}

impl CfgKeyValue {
    pub fn key(&self) -> &str {
        &self.key
    }

    pub fn value(&self) -> &str {
        &self.value
    }
}

fn cfg_value_from_rustc(value: &str) -> String {
    value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .unwrap_or(value)
        .to_string()
}

/// Item-level cfg gates that later target-specific phases evaluate.
#[derive(Debug, Clone, PartialEq, Eq, Default, SchemaRead, SchemaWrite, Shrink)]
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
}

/// One top-level gate that can make an item unavailable.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, Shrink)]
pub enum CfgGate {
    /// A direct `#[cfg(...)]` item attribute.
    Direct(CfgPredicate),
    /// A `#[cfg_attr(predicate, cfg(...))]` attribute that may activate a nested cfg gate.
    CfgAttr {
        predicate: CfgPredicate,
        cfg: CfgPredicate,
    },
}

/// Lowered cfg predicate syntax used by target-specific evaluators.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, Shrink)]
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
    /// Lowers parsed cfg syntax into a stable, AST-independent predicate tree.
    pub fn from_ast(predicate: ast::CfgPredicate) -> Self {
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

/// Target-specific cfg environment used while collecting real definitions.
pub struct CfgEvaluator<'a> {
    options: &'a CfgOptions,
    test_enabled: bool,
}

impl<'a> CfgEvaluator<'a> {
    /// Creates an evaluator for one target's cfg options.
    ///
    /// `#[cfg(test)]` is not reported by rustc target cfg output; Cargo enables it for test and
    /// bench targets, so the target-owning phase passes that bit explicitly.
    pub fn new(options: &'a CfgOptions, test_enabled: bool) -> Self {
        Self {
            options,
            test_enabled,
        }
    }

    /// Returns whether all item-level gates admit the item for this target.
    pub fn is_enabled(&self, cfg: &CfgExpr) -> bool {
        cfg.gates.iter().all(|gate| self.evaluate_gate(gate))
    }

    fn evaluate_gate(&self, gate: &CfgGate) -> bool {
        match gate {
            CfgGate::Direct(predicate) => self.evaluate_predicate(predicate),
            CfgGate::CfgAttr { predicate, cfg } => {
                // `cfg_attr(p, cfg(q))` only contributes the nested cfg when `p` is active.
                !self.evaluate_predicate(predicate) || self.evaluate_predicate(cfg)
            }
        }
    }

    pub fn is_predicate_enabled(&self, predicate: &CfgPredicate) -> bool {
        self.evaluate_predicate(predicate)
    }

    fn evaluate_predicate(&self, predicate: &CfgPredicate) -> bool {
        match predicate {
            CfgPredicate::True => true,
            CfgPredicate::False => false,
            CfgPredicate::Atom(atom) => self.evaluate_atom(atom),
            CfgPredicate::KeyValue { key, value } => self.options.contains_key_value(key, value),
            CfgPredicate::All(predicates) => predicates
                .iter()
                .all(|predicate| self.evaluate_predicate(predicate)),
            CfgPredicate::Any(predicates) => predicates
                .iter()
                .any(|predicate| self.evaluate_predicate(predicate)),
            CfgPredicate::Not(predicates) => match predicates.as_slice() {
                [predicate] => !self.evaluate_predicate(predicate),
                _ => true,
            },
            // Invalid or unsupported cfg syntax is treated as enabled to avoid hiding real code
            // because our parser did not understand an edge case.
            CfgPredicate::Invalid => true,
        }
    }

    fn evaluate_atom(&self, atom: &str) -> bool {
        if atom == "test" {
            return self.test_enabled;
        }

        self.options.contains_atom(atom)
    }
}

rg_std::memsize::impl_memory_size_children! {
    CfgOptions => atoms, key_values;
    CfgKeyValue => key, value;
    CfgExpr => gates;
}

impl MemorySize for CfgGate {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        match self {
            Self::Direct(predicate) => predicate.record_memory_children(recorder),
            Self::CfgAttr { predicate, cfg } => {
                recorder.scope("predicate", |recorder| {
                    predicate.record_memory_children(recorder);
                });
                recorder.scope("cfg", |recorder| cfg.record_memory_children(recorder));
            }
        }
    }
}

impl MemorySize for CfgPredicate {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        match self {
            Self::Atom(atom) => atom.record_memory_children(recorder),
            Self::KeyValue { key, value } => {
                recorder.scope("key", |recorder| key.record_memory_children(recorder));
                recorder.scope("value", |recorder| value.record_memory_children(recorder));
            }
            Self::All(predicates) | Self::Any(predicates) | Self::Not(predicates) => {
                predicates.record_memory_children(recorder);
            }
            Self::True | Self::False | Self::Invalid => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CfgEvaluator, CfgOptions, CfgPredicate};

    #[test]
    fn parses_rustc_cfg_output() {
        let options = CfgOptions::from_rustc_cfg_output(
            r#"
            unix
            target_arch="x86_64"
            feature="std"
            "#,
        );

        assert!(options.contains_atom("unix"));
        assert!(options.contains_key_value("target_arch", "x86_64"));
        assert!(options.contains_key_value("feature", "std"));
    }

    #[test]
    fn evaluates_predicates_against_options_and_test_bit() {
        let mut options = CfgOptions::default();
        options.insert_atom("unix");
        options.insert_key_value("feature", "std");

        let cfg = CfgEvaluator::new(&options, false);
        assert!(cfg.is_predicate_enabled(&CfgPredicate::All(vec![
            CfgPredicate::Atom("unix".to_string()),
            CfgPredicate::KeyValue {
                key: "feature".to_string(),
                value: "std".to_string(),
            },
        ])));
        assert!(!cfg.is_predicate_enabled(&CfgPredicate::Atom("test".to_string())));

        let test_cfg = CfgEvaluator::new(&options, true);
        assert!(test_cfg.is_predicate_enabled(&CfgPredicate::Atom("test".to_string())));
    }
}
