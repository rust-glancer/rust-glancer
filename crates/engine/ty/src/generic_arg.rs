use rg_ir_model::items::GenericArg as ItemGenericArg;
use rg_std::{MemorySize, Shrink};
use rg_text::Name;
use wincode::{SchemaRead, SchemaWrite};

use crate::Ty;

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

    /// Returns whether this generic argument still carries inference variables.
    pub fn has_var(&self) -> bool {
        match self {
            Self::Type(ty) => ty.has_var(),
            Self::FnTraitArgs { params, ret } => params.iter().any(Ty::has_var) || ret.has_var(),
            Self::AssocType { ty, .. } => ty.as_deref().is_some_and(Ty::has_var),
            Self::Lifetime(_) | Self::Const(_) | Self::Unsupported(_) => false,
        }
    }

    /// Returns true when this generic argument contains `Ty::Unknown`.
    pub fn has_unknown(&self) -> bool {
        match self {
            Self::Type(ty) => ty.has_unknown(),
            Self::FnTraitArgs { params, ret } => {
                params.iter().any(Ty::has_unknown) || ret.has_unknown()
            }
            Self::AssocType { ty, .. } => ty.as_deref().is_some_and(Ty::has_unknown),
            Self::Lifetime(_) | Self::Const(_) | Self::Unsupported(_) => false,
        }
    }

    /// Returns true when this generic argument contains `Ty::Unknown` or unresolved syntax.
    pub fn has_unknown_or_syntax(&self) -> bool {
        match self {
            Self::Type(ty) => ty.has_unknown_or_syntax(),
            Self::FnTraitArgs { params, ret } => {
                params.iter().any(Ty::has_unknown_or_syntax) || ret.has_unknown_or_syntax()
            }
            Self::AssocType { ty, .. } => ty.as_deref().is_some_and(Ty::has_unknown_or_syntax),
            Self::Lifetime(_) | Self::Const(_) | Self::Unsupported(_) => false,
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

/// Return whether item-tree generic args align with resolved type generic args.
///
/// Rust code often omits lifetime arguments on paths even when the impl header writes them, e.g.
/// `Builder::new()` selecting `impl<'a, T> Builder<'a, T>`. Such omitted impl lifetime parameters
/// should not shift the type and const arguments that actually select the impl. If a lifetime is
/// written explicitly on the resolved side, it is still consumed and checked against the item side.
///
/// Returns `Ok(true)` when the whole list aligns, `Ok(false)` when the candidate should be
/// rejected, and `Err(_)` for errors reported by the caller's non-lifetime matching policy.
pub(crate) fn item_generic_args_align<'item_arg, 'ty_arg, E, F>(
    item_args: impl IntoIterator<Item = &'item_arg ItemGenericArg>,
    ty_args: impl IntoIterator<Item = &'ty_arg GenericArg>,
    impl_lifetime_params: &[&str],
    mut match_non_lifetime_arg: F,
) -> Result<bool, E>
where
    F: FnMut(&'item_arg ItemGenericArg, &'ty_arg GenericArg) -> Result<bool, E>,
{
    let mut ty_args = ty_args.into_iter().peekable();

    for item_arg in item_args {
        let ItemGenericArg::Lifetime(item_lifetime) = item_arg else {
            let Some(ty_arg) = ty_args.next() else {
                return Ok(false);
            };
            if !match_non_lifetime_arg(item_arg, ty_arg)? {
                return Ok(false);
            }
            continue;
        };

        let Some(ty_arg) = ty_args.peek().copied() else {
            if impl_lifetime_params.contains(&item_lifetime.as_str()) {
                continue;
            }
            return Ok(false);
        };

        if let GenericArg::Lifetime(ty_lifetime) = ty_arg {
            if !impl_lifetime_params.contains(&item_lifetime.as_str())
                && item_lifetime != ty_lifetime
            {
                return Ok(false);
            }
            ty_args.next();
            continue;
        }

        if !impl_lifetime_params.contains(&item_lifetime.as_str()) {
            return Ok(false);
        }
    }

    if ty_args.next().is_some() {
        return Ok(false);
    }

    Ok(true)
}
