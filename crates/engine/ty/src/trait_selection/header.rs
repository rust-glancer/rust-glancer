use rg_ir_model::hir::items::ImplData;

/// Controls how much of an impl header the shallow selector is allowed to accept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TraitSelectionOptions {
    where_predicates_allowed: bool,
}

impl Default for TraitSelectionOptions {
    fn default() -> Self {
        Self::new()
    }
}

impl TraitSelectionOptions {
    /// Keep trait selection strict by default: impl `where` predicates require more solving.
    pub fn new() -> Self {
        Self {
            where_predicates_allowed: false,
        }
    }

    /// Accept the impl header while leaving `where` predicates for a caller to solve separately.
    pub fn ignore_where_predicates(mut self) -> Self {
        self.where_predicates_allowed = true;
        self
    }

    pub(super) fn accepts_impl_header(self, impl_data: &ImplData) -> bool {
        (self.where_predicates_allowed || impl_data.generics.where_predicates.is_empty())
            && impl_data
                .generics
                .lifetimes
                .iter()
                .all(|param| param.bounds.is_empty())
            && impl_data
                .generics
                .types
                .iter()
                .all(|param| param.bounds.is_empty() && param.default.is_none())
            && impl_data.generics.consts.is_empty()
    }
}
