//! Visible module-scope definitions produced by DefMap lookup.

use rg_ir_model::{DefId, ImportRef};
use rg_text::Name;

use crate::{NameBinding, Namespace};

/// Where a visible definition came from during unqualified lookup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisibleScopeOrigin {
    ItemScope,
    Prelude,
    ExternRoot,
}

/// One definition visible from a module through another module's scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisibleScopeDef {
    pub label: String,
    pub namespace: Namespace,
    pub def: DefId,
    /// Lookup source used by unqualified completions to rank familiar names first.
    pub origin: VisibleScopeOrigin,
    /// Selected outer import whose attributes describe this spelling.
    pub attribute_import: Option<ImportRef>,
    /// Whether a same-definition import deliberately re-exported the spelling.
    pub has_reexport: bool,
}

/// Ordered visible definitions plus the namespace-aware shadowing rules used while collecting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisibleScopeDefs(Vec<VisibleScopeDef>);

impl VisibleScopeDefs {
    pub fn empty() -> Self {
        Self(Vec::new())
    }

    pub(crate) fn push_binding(
        &mut self,
        name: &Name,
        namespace: Namespace,
        binding: &NameBinding,
        origin: VisibleScopeOrigin,
        skip_shadowed: bool,
    ) {
        self.push(
            VisibleScopeDef {
                label: name.to_string(),
                namespace,
                def: binding.def,
                origin,
                attribute_import: binding.attribute_import(),
                has_reexport: binding.has_reexport(),
            },
            skip_shadowed,
        );
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
