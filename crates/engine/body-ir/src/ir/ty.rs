use rg_item_tree::TypeRef;
use rg_semantic_ir::TypeDefRef;
use rg_text::Name;

use super::ids::BodyItemRef;

/// Small type vocabulary for the first Body IR pass.
#[derive(Debug, Clone, PartialEq, Eq, Default, wincode::SchemaRead, wincode::SchemaWrite)]
pub enum BodyTy {
    Unit,
    Never,
    Primitive(BodyPrimitiveTy),
    Syntax(TypeRef),
    Reference(#[wincode(with = "rg_wincode_utils::WincodeDynamic<Box<BodyTy>>")] Box<BodyTy>),
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
    #[default]
    Unknown,
}

/// Rust primitive type known without resolving a module-scope definition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, wincode::SchemaRead, wincode::SchemaWrite)]
pub enum BodyPrimitiveTy {
    Bool,
    Char,
    Str,
    SignedInt(BodySignedIntTy),
    UnsignedInt(BodyUnsignedIntTy),
    Float(BodyFloatTy),
}

/// Signed integer primitive width.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, wincode::SchemaRead, wincode::SchemaWrite)]
pub enum BodySignedIntTy {
    I8,
    I16,
    I32,
    I64,
    I128,
    Isize,
}

/// Unsigned integer primitive width.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, wincode::SchemaRead, wincode::SchemaWrite)]
pub enum BodyUnsignedIntTy {
    U8,
    U16,
    U32,
    U64,
    U128,
    Usize,
}

/// Floating-point primitive width.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, wincode::SchemaRead, wincode::SchemaWrite)]
pub enum BodyFloatTy {
    F32,
    F64,
}

impl BodyPrimitiveTy {
    pub const ALL: &'static [Self] = &[
        Self::Bool,
        Self::Char,
        Self::Str,
        Self::SignedInt(BodySignedIntTy::I8),
        Self::SignedInt(BodySignedIntTy::I16),
        Self::SignedInt(BodySignedIntTy::I32),
        Self::SignedInt(BodySignedIntTy::I64),
        Self::SignedInt(BodySignedIntTy::I128),
        Self::SignedInt(BodySignedIntTy::Isize),
        Self::UnsignedInt(BodyUnsignedIntTy::U8),
        Self::UnsignedInt(BodyUnsignedIntTy::U16),
        Self::UnsignedInt(BodyUnsignedIntTy::U32),
        Self::UnsignedInt(BodyUnsignedIntTy::U64),
        Self::UnsignedInt(BodyUnsignedIntTy::U128),
        Self::UnsignedInt(BodyUnsignedIntTy::Usize),
        Self::Float(BodyFloatTy::F32),
        Self::Float(BodyFloatTy::F64),
    ];

    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "bool" => Self::Bool,
            "char" => Self::Char,
            "str" => Self::Str,
            "i8" => Self::SignedInt(BodySignedIntTy::I8),
            "i16" => Self::SignedInt(BodySignedIntTy::I16),
            "i32" => Self::SignedInt(BodySignedIntTy::I32),
            "i64" => Self::SignedInt(BodySignedIntTy::I64),
            "i128" => Self::SignedInt(BodySignedIntTy::I128),
            "isize" => Self::SignedInt(BodySignedIntTy::Isize),
            "u8" => Self::UnsignedInt(BodyUnsignedIntTy::U8),
            "u16" => Self::UnsignedInt(BodyUnsignedIntTy::U16),
            "u32" => Self::UnsignedInt(BodyUnsignedIntTy::U32),
            "u64" => Self::UnsignedInt(BodyUnsignedIntTy::U64),
            "u128" => Self::UnsignedInt(BodyUnsignedIntTy::U128),
            "usize" => Self::UnsignedInt(BodyUnsignedIntTy::Usize),
            "f32" => Self::Float(BodyFloatTy::F32),
            "f64" => Self::Float(BodyFloatTy::F64),
            _ => return None,
        })
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Bool => "bool",
            Self::Char => "char",
            Self::Str => "str",
            Self::SignedInt(kind) => kind.label(),
            Self::UnsignedInt(kind) => kind.label(),
            Self::Float(kind) => kind.label(),
        }
    }
}

impl BodySignedIntTy {
    pub fn label(self) -> &'static str {
        match self {
            Self::I8 => "i8",
            Self::I16 => "i16",
            Self::I32 => "i32",
            Self::I64 => "i64",
            Self::I128 => "i128",
            Self::Isize => "isize",
        }
    }
}

impl BodyUnsignedIntTy {
    pub fn label(self) -> &'static str {
        match self {
            Self::U8 => "u8",
            Self::U16 => "u16",
            Self::U32 => "u32",
            Self::U64 => "u64",
            Self::U128 => "u128",
            Self::Usize => "usize",
        }
    }
}

impl BodyFloatTy {
    pub fn label(self) -> &'static str {
        match self {
            Self::F32 => "f32",
            Self::F64 => "f64",
        }
    }
}

/// Body-local nominal type together with the generic arguments visible at use site.
#[derive(Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite)]
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
#[derive(Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite)]
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

/// Generic argument as understood by the intentionally small Body IR type model.
#[derive(Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite)]
pub enum BodyGenericArg {
    Type(#[wincode(with = "rg_wincode_utils::WincodeDynamic<Box<BodyTy>>")] Box<BodyTy>),
    Lifetime(String),
    Const(String),
    AssocType {
        name: Name,
        #[wincode(with = "rg_wincode_utils::WincodeDynamic<Option<Box<BodyTy>>>")]
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
            | Self::Primitive(_)
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
            | Self::Primitive(_)
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
            | Self::Primitive(_)
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
            Self::Unit | Self::Never | Self::Primitive(_) | Self::Unknown => {}
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
