use rg_memsize::Shrink;
use rg_text::Name;

use crate::{Ty, TypeRepr};

/// Generic argument as understood by the shared type vocabulary.
#[derive(
    Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite, rg_memsize::MemorySize,
)]
pub enum GenericArg<R>
where
    R: TypeRepr,
{
    Type(#[wincode(with = "rg_wincode_utils::WincodeDynamic<Box<Ty<R>>>")] Box<Ty<R>>),
    Lifetime(String),
    Const(String),
    AssocType {
        name: Name,
        #[wincode(with = "rg_wincode_utils::WincodeDynamic<Option<Box<Ty<R>>>>")]
        ty: Option<Box<Ty<R>>>,
    },
    Unsupported(String),
}

impl<R> GenericArg<R>
where
    R: TypeRepr,
{
    pub fn as_ty(&self) -> Option<&Ty<R>> {
        match self {
            Self::Type(ty) => Some(ty),
            Self::Lifetime(_) | Self::Const(_) | Self::AssocType { .. } | Self::Unsupported(_) => {
                None
            }
        }
    }

    pub fn shrink_to_fit(&mut self) {
        match self {
            Self::Type(ty) => ty.shrink_to_fit(),
            Self::Lifetime(text) | Self::Const(text) | Self::Unsupported(text) => {
                text.shrink_to_fit();
            }
            Self::AssocType { name, ty } => {
                name.shrink_to_fit();
                if let Some(ty) = ty {
                    ty.shrink_to_fit();
                }
            }
        }
    }
}

impl<R> Shrink for GenericArg<R>
where
    R: TypeRepr,
{
    fn shrink_to_fit(&mut self) {
        GenericArg::shrink_to_fit(self);
    }
}
