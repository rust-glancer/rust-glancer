//! Evaluates lowered cfg predicates in the context of one target.
//!
//! Item-tree keeps cfg attributes target-independent because the same parsed items can be reused
//! by several targets. Def-map collection is target-specific, so this is where disabled items stop
//! contributing names, imports, modules, and macro calls.

use rg_item_tree::{CfgExpr, CfgGate, CfgPredicate};
use rg_workspace::{CfgOptions, TargetKind};

/// Target-specific cfg environment used while collecting real definitions.
pub(super) struct CfgEvaluator<'a> {
    options: &'a CfgOptions,
    target_kind: &'a TargetKind,
}

impl<'a> CfgEvaluator<'a> {
    pub(super) fn new(options: &'a CfgOptions, target_kind: &'a TargetKind) -> Self {
        Self {
            options,
            target_kind,
        }
    }

    /// Returns whether all item-level gates admit the item for this target.
    pub(super) fn is_enabled(&self, cfg: &CfgExpr) -> bool {
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

    pub(super) fn is_predicate_enabled(&self, predicate: &CfgPredicate) -> bool {
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
        // Cargo test/bench targets see `#[cfg(test)]`; ordinary lib/bin targets do not.
        if atom == "test" {
            return matches!(self.target_kind, TargetKind::Test | TargetKind::Bench);
        }

        self.options.contains_atom(atom)
    }
}
