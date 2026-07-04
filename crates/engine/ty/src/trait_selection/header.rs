use rg_ir_model::hir::items::ImplData;

/// Controls where impl predicates are solved during trait selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TraitSelectionOptions {
    predicate_policy: PredicatePolicy,
    candidate_policy: CandidatePolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PredicatePolicy {
    SolveWithChalk,
    RejectAll,
    CallerSolvesWherePredicates,
    CallerSolvesImplPredicates,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CandidatePolicy {
    PreferDefinite,
    KeepAllApplicable,
}

impl Default for TraitSelectionOptions {
    fn default() -> Self {
        Self::new()
    }
}

impl TraitSelectionOptions {
    /// Keep trait selection strict by default: impl predicates must be proved by Chalk.
    pub fn new() -> Self {
        Self {
            predicate_policy: PredicatePolicy::SolveWithChalk,
            candidate_policy: CandidatePolicy::PreferDefinite,
        }
    }

    /// Match only the direct impl header, rejecting impls that carry unresolved predicates.
    pub fn header_only(mut self) -> Self {
        self.predicate_policy = PredicatePolicy::RejectAll;
        self
    }

    /// Match the direct impl header while leaving explicit `where` predicates to the caller.
    ///
    /// This is narrower than a blanket predicate skip: type-parameter and lifetime-parameter
    /// bounds still reject the impl because callers using this mode only inspect explicit
    /// where-clauses.
    pub fn caller_solves_where_predicates(mut self) -> Self {
        self.predicate_policy = PredicatePolicy::CallerSolvesWherePredicates;
        self
    }

    /// Match the direct impl header while leaving all impl predicates to the caller.
    ///
    /// This is for body-local paths that immediately inspect both inline type-parameter bounds and
    /// explicit where-clauses. It still rejects lifetime-parameter bounds because those are not part
    /// of that body-local predicate stream.
    pub fn caller_solves_impl_predicates(mut self) -> Self {
        self.predicate_policy = PredicatePolicy::CallerSolvesImplPredicates;
        self
    }

    /// Return every applicable candidate, including speculative `Maybe` matches.
    ///
    /// Commit-style inference normally wants the default policy, because an unsupported
    /// header should not drown out a concrete match. Exploratory callers can opt into this mode
    /// when seeing the speculative candidates is more useful than making one committed choice.
    pub fn keep_maybe_candidates(mut self) -> Self {
        self.candidate_policy = CandidatePolicy::KeepAllApplicable;
        self
    }

    pub(super) fn accepts_impl_header(self, impl_data: &ImplData) -> bool {
        match self.predicate_policy {
            PredicatePolicy::SolveWithChalk => true,
            PredicatePolicy::RejectAll => {
                !Self::has_generic_param_bounds(impl_data)
                    && impl_data.generics.where_predicates.is_empty()
            }
            PredicatePolicy::CallerSolvesWherePredicates => {
                !Self::has_generic_param_bounds(impl_data)
            }
            PredicatePolicy::CallerSolvesImplPredicates => {
                !Self::has_lifetime_param_bounds(impl_data)
            }
        }
    }

    pub(super) fn should_solve_where_predicates(self) -> bool {
        self.predicate_policy == PredicatePolicy::SolveWithChalk
    }

    pub(super) fn prefers_definite_candidates(self) -> bool {
        self.candidate_policy == CandidatePolicy::PreferDefinite
    }

    fn has_generic_param_bounds(impl_data: &ImplData) -> bool {
        Self::has_lifetime_param_bounds(impl_data)
            || impl_data
                .generics
                .types
                .iter()
                .any(|param| !param.bounds.is_empty())
    }

    fn has_lifetime_param_bounds(impl_data: &ImplData) -> bool {
        impl_data
            .generics
            .lifetimes
            .iter()
            .any(|param| !param.bounds.is_empty())
    }
}
