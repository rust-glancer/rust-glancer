use rg_ir_model::TypeDefRef;
use rg_item_tree::TypeRef;
use rg_memsize::Shrink;

use crate::{GenericArg, Ty, TypeRepr, TypeSubst};

pub type IndexedTy = Ty<IndexedTyRepr>;
pub type IndexedGenericArg = GenericArg<IndexedTyRepr>;
pub type IndexedTypeSubst = TypeSubst<IndexedTyRepr>;

/// Concrete payload for types whose resolved leaves point back into indexed declarations.
#[derive(
    Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite, rg_memsize::MemorySize,
)]
pub enum IndexedTyRepr {
    Syntax(TypeRef),
    Nominal(
        #[wincode(with = "rg_wincode_utils::WincodeDynamic<Vec<IndexedNominalTy>>")]
        Vec<IndexedNominalTy>,
    ),
    SelfTy(
        #[wincode(with = "rg_wincode_utils::WincodeDynamic<Vec<IndexedNominalTy>>")]
        Vec<IndexedNominalTy>,
    ),
}

impl TypeRepr for IndexedTyRepr {}

/// Module-level nominal type together with the generic arguments visible at use site.
#[derive(
    Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite, rg_memsize::MemorySize,
)]
pub struct IndexedNominalTy {
    pub def: TypeDefRef,
    #[wincode(with = "rg_wincode_utils::WincodeDynamic<Vec<IndexedGenericArg>>")]
    pub args: Vec<IndexedGenericArg>,
}

impl IndexedNominalTy {
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

impl IndexedTyRepr {
    pub fn syntax(ty: TypeRef) -> IndexedTy {
        IndexedTy::repr(Self::Syntax(ty))
    }

    pub fn nominal(types: Vec<IndexedNominalTy>) -> IndexedTy {
        IndexedTy::repr(Self::Nominal(types))
    }

    pub fn self_ty(types: Vec<IndexedNominalTy>) -> IndexedTy {
        IndexedTy::repr(Self::SelfTy(types))
    }

    fn shrink_to_fit(&mut self) {
        match self {
            Self::Syntax(ty) => ty.shrink_to_fit(),
            Self::Nominal(types) | Self::SelfTy(types) => {
                types.shrink_to_fit();
                for ty in types {
                    ty.shrink_to_fit();
                }
            }
        }
    }

    pub fn as_nominals(&self) -> &[IndexedNominalTy] {
        match self {
            Self::Nominal(types) | Self::SelfTy(types) => types,
            Self::Syntax(_) => &[],
        }
    }
}

impl Shrink for IndexedTyRepr {
    fn shrink_to_fit(&mut self) {
        IndexedTyRepr::shrink_to_fit(self);
    }
}

/// Helpers for the concrete indexed type vocabulary.
pub trait IndexedTyExt {
    fn as_nominals(&self) -> &[IndexedNominalTy];
}

impl IndexedTyExt for IndexedTy {
    fn as_nominals(&self) -> &[IndexedNominalTy] {
        self.as_repr()
            .map(IndexedTyRepr::as_nominals)
            .unwrap_or(&[])
    }
}
