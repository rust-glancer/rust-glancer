use rg_item_tree::TypeRef;
use rg_memsize::Shrink;
use rg_semantic_ir::TypeDefRef;

use super::ids::BodyItemRef;

pub type BodyTy = rg_ty::Ty<BodyTyRepr>;
pub type BodyGenericArg = rg_ty::GenericArg<BodyTyRepr>;
pub(crate) type BodyTypeSubst = rg_ty::TypeSubst<BodyTyRepr>;

/// Body IR's concrete payload for the shared type vocabulary.
///
/// The generic `rg_ty::Ty` layer only knows about common shells such as references and primitives.
/// This payload keeps the body-specific distinction between unresolved syntax, semantic nominal
/// definitions, body-local nominal definitions, and `Self` candidates.
#[derive(
    Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite, rg_memsize::MemorySize,
)]
pub enum BodyTyRepr {
    Syntax(TypeRef),
    LocalNominal(
        #[wincode(with = "rg_wincode_utils::WincodeDynamic<Vec<BodyLocalNominalTy>>")]
        Vec<BodyLocalNominalTy>,
    ),
    Nominal(
        #[wincode(with = "rg_wincode_utils::WincodeDynamic<Vec<BodyNominalTy>>")]
        Vec<BodyNominalTy>,
    ),
    SelfTy(
        #[wincode(with = "rg_wincode_utils::WincodeDynamic<Vec<BodyNominalTy>>")]
        Vec<BodyNominalTy>,
    ),
}

impl rg_ty::TypeRepr for BodyTyRepr {}

/// Body-local nominal type together with the generic arguments visible at use site.
#[derive(
    Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite, rg_memsize::MemorySize,
)]
pub struct BodyLocalNominalTy {
    pub item: BodyItemRef,
    #[wincode(with = "rg_wincode_utils::WincodeDynamic<Vec<BodyGenericArg>>")]
    pub args: Vec<BodyGenericArg>,
}

impl BodyLocalNominalTy {
    pub fn bare(item: BodyItemRef) -> Self {
        Self {
            item,
            args: Vec::new(),
        }
    }

    pub(crate) fn shrink_to_fit(&mut self) {
        self.args.shrink_to_fit();
        for arg in &mut self.args {
            arg.shrink_to_fit();
        }
    }
}

/// Module-level nominal type together with the generic arguments visible at use site.
#[derive(
    Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite, rg_memsize::MemorySize,
)]
pub struct BodyNominalTy {
    pub def: TypeDefRef,
    #[wincode(with = "rg_wincode_utils::WincodeDynamic<Vec<BodyGenericArg>>")]
    pub args: Vec<BodyGenericArg>,
}

impl BodyNominalTy {
    pub fn bare(def: TypeDefRef) -> Self {
        Self {
            def,
            args: Vec::new(),
        }
    }

    pub(crate) fn shrink_to_fit(&mut self) {
        self.args.shrink_to_fit();
        for arg in &mut self.args {
            arg.shrink_to_fit();
        }
    }
}

impl BodyTyRepr {
    pub fn syntax(ty: TypeRef) -> BodyTy {
        BodyTy::repr(Self::Syntax(ty))
    }

    pub fn local_nominal(types: Vec<BodyLocalNominalTy>) -> BodyTy {
        BodyTy::repr(Self::LocalNominal(types))
    }

    pub fn nominal(types: Vec<BodyNominalTy>) -> BodyTy {
        BodyTy::repr(Self::Nominal(types))
    }

    pub fn self_ty(types: Vec<BodyNominalTy>) -> BodyTy {
        BodyTy::repr(Self::SelfTy(types))
    }

    pub(crate) fn shrink_to_fit(&mut self) {
        match self {
            Self::Syntax(ty) => ty.shrink_to_fit(),
            Self::LocalNominal(types) => {
                types.shrink_to_fit();
                for ty in types {
                    ty.shrink_to_fit();
                }
            }
            Self::Nominal(types) | Self::SelfTy(types) => {
                types.shrink_to_fit();
                for ty in types {
                    ty.shrink_to_fit();
                }
            }
        }
    }

    pub(crate) fn as_local_nominals(&self) -> &[BodyLocalNominalTy] {
        match self {
            Self::LocalNominal(types) => types,
            Self::Syntax(_) | Self::Nominal(_) | Self::SelfTy(_) => &[],
        }
    }

    pub(crate) fn as_nominals(&self) -> &[BodyNominalTy] {
        match self {
            Self::Nominal(types) | Self::SelfTy(types) => types,
            Self::Syntax(_) | Self::LocalNominal(_) => &[],
        }
    }
}

impl Shrink for BodyTyRepr {
    fn shrink_to_fit(&mut self) {
        BodyTyRepr::shrink_to_fit(self);
    }
}

/// Body-specific helpers for the shared type vocabulary.
pub trait BodyTyExt {
    fn as_local_nominals(&self) -> &[BodyLocalNominalTy];
    fn as_nominals(&self) -> &[BodyNominalTy];
}

impl BodyTyExt for BodyTy {
    fn as_local_nominals(&self) -> &[BodyLocalNominalTy] {
        self.as_repr()
            .map(BodyTyRepr::as_local_nominals)
            .unwrap_or(&[])
    }

    fn as_nominals(&self) -> &[BodyNominalTy] {
        self.as_repr().map(BodyTyRepr::as_nominals).unwrap_or(&[])
    }
}
