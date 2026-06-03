//! Visible module-scope definitions produced by DefMap lookup.

use rg_ir_model::DefId;

use super::scope::ModuleScopeBuilder;

/// Where a visible definition came from during unqualified lookup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisibleScopeOrigin {
    ModuleScope,
    Prelude,
    ExternRoot,
}

/// Namespace slot occupied by a visible module-scope definition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ScopeNamespace {
    Types,
    Values,
    Macros,
}

impl ScopeNamespace {
    fn sort_rank(self) -> u8 {
        match self {
            Self::Types => 0,
            Self::Values => 1,
            Self::Macros => 2,
        }
    }
}

/// One definition visible from a module through another module's scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisibleScopeDef {
    pub label: String,
    pub namespace: ScopeNamespace,
    pub def: DefId,
    /// Lookup source used by unqualified completions to rank familiar names first.
    pub origin: VisibleScopeOrigin,
}

/// Ordered visible definitions plus the namespace-aware shadowing rules used while collecting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisibleScopeDefs(Vec<VisibleScopeDef>);

impl VisibleScopeDefs {
    pub fn new(
        scope: &ModuleScopeBuilder,
        origin: VisibleScopeOrigin,
        skip_shadowed: bool,
    ) -> Self {
        let mut self_ = Self(Vec::new());
        self_.extend(scope, origin, skip_shadowed);
        self_
    }

    pub fn extend(
        &mut self,
        scope: &ModuleScopeBuilder,
        origin: VisibleScopeOrigin,
        skip_shadowed: bool,
    ) {
        // The visibility-aware builder keeps namespace buckets separate. Analysis filters those
        // buckets according to the syntactic context where completion was requested.
        for (name, entry) in scope.entries() {
            for binding in entry.types() {
                self.push(
                    VisibleScopeDef {
                        label: name.to_string(),
                        namespace: ScopeNamespace::Types,
                        def: binding.def,
                        origin,
                    },
                    skip_shadowed,
                );
            }
            for binding in entry.values() {
                self.push(
                    VisibleScopeDef {
                        label: name.to_string(),
                        namespace: ScopeNamespace::Values,
                        def: binding.def,
                        origin,
                    },
                    skip_shadowed,
                );
            }
            for binding in entry.macros() {
                self.push(
                    VisibleScopeDef {
                        label: name.to_string(),
                        namespace: ScopeNamespace::Macros,
                        def: binding.def,
                        origin,
                    },
                    skip_shadowed,
                );
            }
        }
    }

    pub fn push(&mut self, def: VisibleScopeDef, skip_shadowed: bool) {
        if skip_shadowed
            && self
                .0
                .iter()
                .any(|existing| existing.label == def.label && existing.namespace == def.namespace)
        {
            return;
        }

        self.0.push(def);
    }

    pub fn sort(&mut self) {
        self.0.sort_by(|left, right| {
            left.label
                .cmp(&right.label)
                .then(left.namespace.sort_rank().cmp(&right.namespace.sort_rank()))
                .then(format!("{:?}", left.def).cmp(&format!("{:?}", right.def)))
        });
    }

    pub fn as_slice(&self) -> &[VisibleScopeDef] {
        &self.0
    }

    pub fn into_vec(self) -> Vec<VisibleScopeDef> {
        self.0
    }
}

impl IntoIterator for VisibleScopeDefs {
    type Item = VisibleScopeDef;
    type IntoIter = std::vec::IntoIter<VisibleScopeDef>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'a> IntoIterator for &'a VisibleScopeDefs {
    type Item = &'a VisibleScopeDef;
    type IntoIter = std::slice::Iter<'a, VisibleScopeDef>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}
