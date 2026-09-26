//! Stable method identities and their applicability for member-query consumers.
//!
//! Discovery and collection policies belong to body queries, where lexical trait scope and
//! caller assumptions are available. Results retain declarations rather than solver storage.

use rg_ir_model::{FunctionRef, TraitApplicability};

/// One callable member selected for a receiver type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemberMethodCandidateRef {
    function: FunctionRef,
    origin: MemberMethodOrigin,
}

impl MemberMethodCandidateRef {
    pub fn inherent(function: FunctionRef) -> Self {
        Self {
            function,
            origin: MemberMethodOrigin::Inherent,
        }
    }

    pub fn trait_method(function: FunctionRef, applicability: TraitApplicability) -> Self {
        Self {
            function,
            origin: MemberMethodOrigin::Trait { applicability },
        }
    }

    pub fn function(self) -> FunctionRef {
        self.function
    }

    pub fn origin(self) -> MemberMethodOrigin {
        self.origin
    }
}

/// Why a method candidate is visible on a receiver.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemberMethodOrigin {
    Inherent,
    Trait { applicability: TraitApplicability },
}
