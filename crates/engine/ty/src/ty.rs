use rg_ir_model::TypeDefRef;
use rg_item_tree::TypeRef;
use rg_memsize::Shrink;
use rg_text::Name;

use crate::{GenericArg, PrimitiveTy, RefMutability};

/// Mapping from a generic type parameter name to the concrete type known at a use site.
// TODO: Probably deserves more than an alias?
pub type TypeSubst = Vec<(Name, Ty)>;

/// Small type vocabulary shared by IR layers.
#[derive(
    Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite, rg_memsize::MemorySize,
)]
pub enum Ty {
    Unit,
    Never,
    Primitive(PrimitiveTy),
    Reference {
        mutability: RefMutability,
        #[wincode(with = "rg_wincode_utils::WincodeDynamic<Box<Ty>>")]
        inner: Box<Ty>,
    },
    Syntax(TypeRef),
    Nominal(#[wincode(with = "rg_wincode_utils::WincodeDynamic<Vec<NominalTy>>")] Vec<NominalTy>),
    SelfTy(#[wincode(with = "rg_wincode_utils::WincodeDynamic<Vec<NominalTy>>")] Vec<NominalTy>),
    Unknown,
}

/// Module-level nominal type together with the generic arguments visible at use site.
#[derive(
    Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite, rg_memsize::MemorySize,
)]
pub struct NominalTy {
    pub def: TypeDefRef,
    #[wincode(with = "rg_wincode_utils::WincodeDynamic<Vec<GenericArg>>")]
    pub args: Vec<GenericArg>,
}

impl NominalTy {
    pub fn bare(def: TypeDefRef) -> Self {
        Self {
            def,
            args: Vec::new(),
        }
    }

    fn shrink_to_fit(&mut self) {
        self.args.shrink_to_fit();
        for arg in &mut self.args {
            arg.shrink_to_fit();
        }
    }
}

impl Ty {
    pub fn reference(mutability: RefMutability, inner: Self) -> Self {
        if matches!(inner, Self::Unknown) {
            return Self::Unknown;
        }

        Self::Reference {
            mutability,
            inner: Box::new(inner),
        }
    }

    pub fn syntax(ty: TypeRef) -> Self {
        Self::Syntax(ty)
    }

    pub fn nominal(types: Vec<NominalTy>) -> Self {
        Self::Nominal(types)
    }

    pub fn self_ty(types: Vec<NominalTy>) -> Self {
        Self::SelfTy(types)
    }

    pub fn as_nominals(&self) -> &[NominalTy] {
        match self {
            Self::Nominal(types) | Self::SelfTy(types) => types,
            Self::Unit
            | Self::Never
            | Self::Primitive(_)
            | Self::Reference { .. }
            | Self::Syntax(_)
            | Self::Unknown => &[],
        }
    }

    pub fn reference_inner(&self) -> Option<(&Self, RefMutability)> {
        match self {
            Self::Reference { mutability, inner } => Some((inner, *mutability)),
            Self::Unit
            | Self::Never
            | Self::Primitive(_)
            | Self::Syntax(_)
            | Self::Nominal(_)
            | Self::SelfTy(_)
            | Self::Unknown => None,
        }
    }

    pub fn one_or_unknown(mut tys: Vec<Self>) -> Self {
        if tys.len() == 1 {
            tys.pop().expect("one type should exist")
        } else {
            Self::Unknown
        }
    }

    pub fn shrink_to_fit(&mut self) {
        match self {
            Self::Reference { inner, .. } => inner.shrink_to_fit(),
            Self::Syntax(ty) => ty.shrink_to_fit(),
            Self::Nominal(types) | Self::SelfTy(types) => {
                types.shrink_to_fit();
                for ty in types {
                    ty.shrink_to_fit();
                }
            }
            Self::Unit | Self::Never | Self::Primitive(_) | Self::Unknown => {}
        }
    }
}

impl Shrink for Ty {
    fn shrink_to_fit(&mut self) {
        Ty::shrink_to_fit(self);
    }
}
