//! Selected call targets and the receiver evidence retained for them.

use rg_ir_model::{FunctionRef, ScopeId, identity::DeclarationRef};
use rg_item_tree::GenericArg as ItemGenericArg;
use rg_std::{ExpectedUnique, UniqueVec};
use rg_ty::{Substitution, Ty, trait_selection::TraitSelection};

use crate::body::facts::BodyResolution;

/// Semantic function selected for one written call before body-local inference.
///
/// The target retains explicit generic syntax and the call-site scope so type arguments can be
/// lowered later against the correct body context. Receiver or type-prefix evidence is kept
/// separately from function-owned generics. Trait candidates also retain their trial selection so
/// inference can commit its table only after lookup finds one definite target.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ResolvedCallTarget {
    function: FunctionRef,
    explicit_args: Vec<ItemGenericArg>,
    site_scope: ScopeId,
    pub(crate) self_source: CallSelfSource,
    pub(crate) trait_selection: Option<TraitSelection>,
}

impl ResolvedCallTarget {
    /// Build target data for an ordinary function call.
    pub(crate) fn function_call(
        function: FunctionRef,
        site_scope: ScopeId,
        explicit_args: &[ItemGenericArg],
    ) -> Self {
        Self {
            function,
            explicit_args: explicit_args.to_vec(),
            site_scope,
            self_source: CallSelfSource::None,
            trait_selection: None,
        }
    }

    /// Build target data for a method call with receiver facts.
    pub(crate) fn method_call(
        function: FunctionRef,
        site_scope: ScopeId,
        explicit_args: &[ItemGenericArg],
        receiver: CallSelf,
        trait_selection: Option<TraitSelection>,
    ) -> Self {
        Self {
            function,
            explicit_args: explicit_args.to_vec(),
            site_scope,
            self_source: CallSelfSource::Receiver(receiver),
            trait_selection,
        }
    }

    /// Build target data for an associated function call with selected `Self`.
    pub(crate) fn associated_function_call(
        function: FunctionRef,
        site_scope: ScopeId,
        explicit_args: &[ItemGenericArg],
        self_context: CallSelf,
        trait_selection: Option<TraitSelection>,
    ) -> Self {
        Self {
            function,
            explicit_args: explicit_args.to_vec(),
            site_scope,
            self_source: CallSelfSource::TypePrefix(self_context),
            trait_selection,
        }
    }

    /// Return the selected function.
    pub(crate) fn function(&self) -> FunctionRef {
        self.function
    }

    /// Return explicit generic arguments written at the call site.
    pub(crate) fn explicit_args(&self) -> &[ItemGenericArg] {
        &self.explicit_args
    }

    /// Return the body scope where explicit call arguments were written.
    pub(crate) fn site_scope(&self) -> ScopeId {
        self.site_scope
    }

    /// Return the first signature param matched by written call args.
    pub(crate) fn first_written_param_idx(&self) -> usize {
        self.self_source.first_written_param_idx()
    }
}

/// How `Self` entered a selected call and whether syntax supplied an implicit receiver argument.
///
/// `Type::make(value)` contributes a `Self` substitution but its written arguments still begin at
/// signature parameter zero. `value.method(arg)` contributes the same substitution and consumes
/// parameter zero as the implicit receiver.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CallSelfSource {
    None,
    TypePrefix(CallSelf),
    Receiver(CallSelf),
}

impl CallSelfSource {
    /// Skip implicit receiver params when projecting written arguments.
    fn first_written_param_idx(&self) -> usize {
        match self {
            Self::None => 0,
            Self::TypePrefix(_) => 0,
            Self::Receiver(_) => 1,
        }
    }
}

/// Concrete `Self` evidence recovered together with its owner-scoped substitution.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct CallSelf {
    pub(crate) self_ty: Ty,
    pub(crate) subst: Substitution,
}

/// Call targets selected for one call expression.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ResolvedCallTargets {
    targets: UniqueVec<ResolvedCallTarget>,
}

impl ResolvedCallTargets {
    /// Start with no selected call targets.
    pub(crate) fn new() -> Self {
        Self {
            targets: UniqueVec::new(),
        }
    }

    /// Return whether call lookup found no targets.
    pub(crate) fn is_empty(&self) -> bool {
        self.targets.is_empty()
    }

    /// Return function declarations for the selected call targets.
    pub(crate) fn resolution(&self) -> BodyResolution {
        let mut functions = UniqueVec::new();
        for target in &self.targets {
            functions.push(target.function());
        }

        if functions.is_empty() {
            BodyResolution::Unknown
        } else {
            BodyResolution::Declarations(functions.into_iter().map(DeclarationRef::from).collect())
        }
    }

    /// Add one target, preserving uniqueness.
    pub(crate) fn push(&mut self, target: ResolvedCallTarget) {
        self.targets.push(target);
    }

    /// Return the unique target whose trait predicates were fully proved.
    pub(crate) fn single_proven(self) -> Option<ResolvedCallTarget> {
        let mut target = ExpectedUnique::new();
        for candidate in self.targets {
            // Ordinary and inherent functions need no trait proof. Trait functions must have a
            // definite selection; `Maybe` remains useful to editor lookup but cannot own call
            // inference or associated projection facts.
            if candidate.trait_selection.as_ref().is_none_or(|selection| {
                selection.applicability == rg_ir_model::TraitApplicability::Yes
            }) {
                target.push(candidate);
            }
        }
        target.into_option()
    }
}
