use rg_memsize::Shrink;
use rg_text::Name;

use crate::{PrimitiveTy, RefMutability};

/// Storage-specific payload carried by the common type vocabulary.
pub trait TypeRepr: Shrink {}

/// Mapping from a generic type parameter name to the concrete type known at a use site.
pub type TypeSubst<R> = Vec<(Name, Ty<R>)>;

/// Small type vocabulary shared by IR layers.
#[derive(
    Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite, rg_memsize::MemorySize,
)]
pub enum Ty<R>
where
    R: TypeRepr,
{
    Unit,
    Never,
    Primitive(PrimitiveTy),
    Reference {
        mutability: RefMutability,
        #[wincode(with = "rg_wincode_utils::WincodeDynamic<Box<Ty<R>>>")]
        inner: Box<Ty<R>>,
    },
    Repr(#[wincode(with = "rg_wincode_utils::WincodeDynamic<R>")] R),
    Unknown,
}

impl<R> Default for Ty<R>
where
    R: TypeRepr,
{
    fn default() -> Self {
        Self::Unknown
    }
}

impl<R> Ty<R>
where
    R: TypeRepr,
{
    pub fn reference(mutability: RefMutability, inner: Self) -> Self {
        if matches!(inner, Self::Unknown) {
            return Self::Unknown;
        }

        Self::Reference {
            mutability,
            inner: Box::new(inner),
        }
    }

    pub fn repr(repr: R) -> Self {
        Self::Repr(repr)
    }

    pub fn as_repr(&self) -> Option<&R> {
        match self {
            Self::Repr(repr) => Some(repr),
            Self::Unit
            | Self::Never
            | Self::Primitive(_)
            | Self::Reference { .. }
            | Self::Unknown => None,
        }
    }

    pub fn reference_inner(&self) -> Option<(&Self, RefMutability)> {
        match self {
            Self::Reference { mutability, inner } => Some((inner, *mutability)),
            Self::Unit | Self::Never | Self::Primitive(_) | Self::Repr(_) | Self::Unknown => None,
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
            Self::Repr(repr) => repr.shrink_to_fit(),
            Self::Unit | Self::Never | Self::Primitive(_) | Self::Unknown => {}
        }
    }
}

impl<R> Shrink for Ty<R>
where
    R: TypeRepr,
{
    fn shrink_to_fit(&mut self) {
        Ty::shrink_to_fit(self);
    }
}
