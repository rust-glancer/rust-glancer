use rg_memsize::Shrink;
use rg_text::Name;

use crate::Ty;

/// Generic argument as understood by the shared type vocabulary.
#[derive(
    Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite, rg_memsize::MemorySize,
)]
pub enum GenericArg {
    Type(#[wincode(with = "rg_wincode_utils::WincodeDynamic<Box<Ty>>")] Box<Ty>),
    Lifetime(String),
    Const(String),
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

impl Shrink for GenericArg {
    fn shrink_to_fit(&mut self) {
        match self {
            Self::Type(ty) => ty.shrink_to_fit(),
            Self::Lifetime(text) | Self::Const(text) | Self::Unsupported(text) => {
                text.shrink_to_fit();
            }
            Self::FnTraitArgs { params, ret } => {
                params.shrink_to_fit();
                for param in params {
                    param.shrink_to_fit();
                }
                ret.shrink_to_fit();
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
