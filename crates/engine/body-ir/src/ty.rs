use rg_item_tree::TypeRef;
use rg_semantic_ir::TypeDefRef;
use rg_text::Name;

use crate::ids::BodyItemRef;

/// Small type vocabulary for the first Body IR pass.
#[derive(Debug, Clone, PartialEq, Eq, Default, wincode::SchemaRead, wincode::SchemaWrite)]
pub enum BodyTy {
    Unit,
    Never,
    Syntax(TypeRef),
    Reference(#[wincode(with = "rg_text::WincodeDynamic<Box<BodyTy>>")] Box<BodyTy>),
    LocalNominal(
        #[wincode(with = "rg_text::WincodeDynamic<Vec<BodyLocalNominalTy>>")]
        Vec<BodyLocalNominalTy>,
    ),
    Nominal(#[wincode(with = "rg_text::WincodeDynamic<Vec<BodyNominalTy>>")] Vec<BodyNominalTy>),
    SelfTy(#[wincode(with = "rg_text::WincodeDynamic<Vec<BodyNominalTy>>")] Vec<BodyNominalTy>),
    #[default]
    Unknown,
}

/// Body-local nominal type together with the generic arguments visible at use site.
#[derive(Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite)]
pub struct BodyLocalNominalTy {
    pub item: BodyItemRef,
    #[wincode(with = "rg_text::WincodeDynamic<Vec<BodyGenericArg>>")]
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
#[derive(Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite)]
pub struct BodyNominalTy {
    pub def: TypeDefRef,
    #[wincode(with = "rg_text::WincodeDynamic<Vec<BodyGenericArg>>")]
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

/// Generic argument as understood by the intentionally small Body IR type model.
#[derive(Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite)]
pub enum BodyGenericArg {
    Type(#[wincode(with = "rg_text::WincodeDynamic<Box<BodyTy>>")] Box<BodyTy>),
    Lifetime(String),
    Const(String),
    AssocType {
        name: Name,
        #[wincode(with = "rg_text::WincodeDynamic<Option<Box<BodyTy>>>")]
        ty: Option<Box<BodyTy>>,
    },
    Unsupported(String),
}

impl BodyTy {
    pub fn reference(inner: BodyTy) -> Self {
        if matches!(inner, Self::Unknown) {
            return Self::Unknown;
        }

        Self::Reference(Box::new(inner))
    }

    pub fn peel_references(&self) -> &Self {
        match self {
            Self::Reference(inner) => inner.peel_references(),
            Self::Unit
            | Self::Never
            | Self::Syntax(_)
            | Self::LocalNominal(_)
            | Self::Nominal(_)
            | Self::SelfTy(_)
            | Self::Unknown => self,
        }
    }

    pub fn local_nominals(&self) -> &[BodyLocalNominalTy] {
        match self.peel_references() {
            Self::LocalNominal(types) => types,
            Self::Unit
            | Self::Never
            | Self::Syntax(_)
            | Self::Reference(_)
            | Self::Nominal(_)
            | Self::SelfTy(_)
            | Self::Unknown => &[],
        }
    }

    pub fn nominal_tys(&self) -> &[BodyNominalTy] {
        match self.peel_references() {
            Self::Nominal(types) | Self::SelfTy(types) => types,
            Self::Unit
            | Self::Never
            | Self::Syntax(_)
            | Self::Reference(_)
            | Self::LocalNominal(_)
            | Self::Unknown => &[],
        }
    }

    pub fn local_items(&self) -> Vec<BodyItemRef> {
        self.local_nominals().iter().map(|ty| ty.item).collect()
    }

    pub fn type_defs(&self) -> Vec<TypeDefRef> {
        self.nominal_tys().iter().map(|ty| ty.def).collect()
    }

    pub(crate) fn shrink_to_fit(&mut self) {
        match self {
            Self::Syntax(ty) => ty.shrink_to_fit(),
            Self::Reference(inner) => inner.shrink_to_fit(),
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
            Self::Unit | Self::Never | Self::Unknown => {}
        }
    }
}

impl BodyGenericArg {
    fn shrink_to_fit(&mut self) {
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
