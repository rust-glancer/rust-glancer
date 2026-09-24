//! Owned inputs for trait-selection and associated-type queries.

use crate::{AssocTypeBinding, GenericArg, GenericArgs, TraitApplication, TraitRefLowering, Ty};

/// A trait question expressed in owned types, including any associated-type equalities.
/// For `T: Iterator<Item = u8>`, the application supplies `T` as `Self`, and the separate equality
/// is `<T as Iterator>::Item = u8`. `Item` is not a positional argument of `Iterator`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TraitGoal {
    pub application: TraitApplication,
    pub associated_types: Vec<AssocTypeBinding>,
}

impl TraitGoal {
    /// Build a goal from positional arguments that do not include `Self`.
    pub fn new(
        self_ty: Ty,
        trait_ref: rg_ir_model::TraitDefRef,
        args: impl Into<GenericArgs>,
    ) -> Self {
        let args = args.into();
        let mut full_args = Vec::with_capacity(1 + args.len());
        full_args.push(GenericArg::Type(Box::new(self_ty)));
        full_args.extend(args.into_vec());
        Self {
            application: TraitApplication {
                def: trait_ref,
                args: full_args.into(),
            },
            associated_types: Vec::new(),
        }
    }

    pub fn from_lowering(lowering: TraitRefLowering) -> Self {
        Self {
            application: lowering.application,
            associated_types: lowering.associated_types,
        }
    }

    pub fn self_ty(&self) -> &Ty {
        self.application
            .self_ty()
            .expect("trait applications always contain the Self argument")
    }

    pub fn trait_ref(&self) -> rg_ir_model::TraitDefRef {
        self.application.def
    }

    /// Iterate trait input args without associated-type equality constraints.
    ///
    /// Rust syntax puts both shapes inside the same angle brackets:
    ///
    /// ```text
    /// Iterator<Item = User>
    /// Indexed<Key, Item = User>
    /// ```
    ///
    /// Only the positional inputs belong in the trait substitution represented as
    /// `Implemented(Self: Trait<...>)`. Associated equality args are separate projection
    /// constraints, such as `<Self as Iterator>::Item = User`.
    pub fn iter_positional_args(&self) -> impl Iterator<Item = &GenericArg> {
        self.application.args.iter().skip(1)
    }
}
