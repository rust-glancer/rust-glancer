//! Stable method identities and their applicability for member-query consumers.
//!
//! Discovery and collection policies belong to body queries, where lexical trait scope and
//! caller assumptions are available. Results retain declarations and any requested type match.

use rg_ir_model::{FunctionRef, TraitApplicability};

/// One callable member selected for a receiver type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemberMethodCandidateRef {
    function: FunctionRef,
    origin: MemberMethodOrigin,
    return_type_matches: bool,
}

impl MemberMethodCandidateRef {
    pub fn new(
        function: FunctionRef,
        origin: MemberMethodOrigin,
        return_type_matches: bool,
    ) -> Self {
        Self {
            function,
            origin,
            return_type_matches,
        }
    }

    pub fn function(self) -> FunctionRef {
        self.function
    }

    pub fn origin(self) -> MemberMethodOrigin {
        self.origin
    }

    pub fn return_type_matches(self) -> bool {
        self.return_type_matches
    }
}

/// Why a method candidate is visible on a receiver. Inherent candidates sort first when merging
/// duplicate declarations, followed by trait candidates in applicability order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MemberMethodOrigin {
    Inherent,
    Trait { applicability: TraitApplicability },
}
