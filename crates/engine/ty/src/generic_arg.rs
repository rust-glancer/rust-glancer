use rg_std::{MemorySize, Shrink};
use rg_text::Name;

use crate::Ty;
use wincode::{SchemaRead, SchemaWrite};

/// Generic argument as understood by the shared type vocabulary.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub enum GenericArg {
    Type(#[wincode(with = "rg_wincode_utils::WincodeDynamic<Box<Ty>>")] Box<Ty>),
    Lifetime(String),
    Const(String),
    /// Parenthesized argument syntax on function-trait paths, such as `FnOnce(T) -> R`.
    FnTraitArgs {
        #[wincode(with = "rg_wincode_utils::WincodeDynamic<Vec<Ty>>")]
        params: Vec<Ty>,
        #[wincode(with = "rg_wincode_utils::WincodeDynamic<Box<Ty>>")]
        ret: Box<Ty>,
    },
    AssocType {
        name: Name,
        #[wincode(with = "rg_wincode_utils::WincodeDynamic<Option<Box<Ty>>>")]
        ty: Option<Box<Ty>>,
    },
    Unsupported(String),
}

impl GenericArg {
    pub fn as_ty(&self) -> Option<&Ty> {
        match self {
            Self::Type(ty) => Some(ty),
            Self::Lifetime(_)
            | Self::Const(_)
            | Self::FnTraitArgs { .. }
            | Self::AssocType { .. }
            | Self::Unsupported(_) => None,
        }
    }

    pub(crate) fn is_projectable(&self) -> bool {
        match self {
            Self::Type(ty) => ty.is_projectable(),
            Self::Lifetime(_) | Self::Const(_) => true,
            Self::FnTraitArgs { params, ret } => {
                params.iter().all(Ty::is_projectable) && ret.is_projectable()
            }
            Self::AssocType { ty, .. } => ty.as_deref().is_none_or(Ty::is_projectable),
            Self::Unsupported(_) => false,
        }
    }
}
