use rg_std::MemorySize;
use wincode::{SchemaRead, SchemaWrite};

/// Rust primitive type known without resolving a module-scope definition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize)]
#[memsize(leaf)]
pub enum PrimitiveTy {
    Bool,
    Char,
    Str,
    SignedInt(SignedIntTy),
    UnsignedInt(UnsignedIntTy),
    Float(FloatTy),
}

/// Signed integer primitive width.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize)]
#[memsize(leaf)]
pub enum SignedIntTy {
    I8,
    I16,
    I32,
    I64,
    I128,
    Isize,
}

/// Unsigned integer primitive width.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize)]
#[memsize(leaf)]
pub enum UnsignedIntTy {
    U8,
    U16,
    U32,
    U64,
    U128,
    Usize,
}

/// Floating-point primitive width.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize)]
#[memsize(leaf)]
pub enum FloatTy {
    F32,
    F64,
}

impl PrimitiveTy {
    pub const DEFAULT_INT: Self = Self::SignedInt(SignedIntTy::I32);
    pub const DEFAULT_FLOAT: Self = Self::Float(FloatTy::F64);

    pub const ALL: &'static [Self] = &[
        Self::Bool,
        Self::Char,
        Self::Str,
        Self::SignedInt(SignedIntTy::I8),
        Self::SignedInt(SignedIntTy::I16),
        Self::SignedInt(SignedIntTy::I32),
        Self::SignedInt(SignedIntTy::I64),
        Self::SignedInt(SignedIntTy::I128),
        Self::SignedInt(SignedIntTy::Isize),
        Self::UnsignedInt(UnsignedIntTy::U8),
        Self::UnsignedInt(UnsignedIntTy::U16),
        Self::UnsignedInt(UnsignedIntTy::U32),
        Self::UnsignedInt(UnsignedIntTy::U64),
        Self::UnsignedInt(UnsignedIntTy::U128),
        Self::UnsignedInt(UnsignedIntTy::Usize),
        Self::Float(FloatTy::F32),
        Self::Float(FloatTy::F64),
    ];

    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "bool" => Self::Bool,
            "char" => Self::Char,
            "str" => Self::Str,
            "i8" => Self::SignedInt(SignedIntTy::I8),
            "i16" => Self::SignedInt(SignedIntTy::I16),
            "i32" => Self::SignedInt(SignedIntTy::I32),
            "i64" => Self::SignedInt(SignedIntTy::I64),
            "i128" => Self::SignedInt(SignedIntTy::I128),
            "isize" => Self::SignedInt(SignedIntTy::Isize),
            "u8" => Self::UnsignedInt(UnsignedIntTy::U8),
            "u16" => Self::UnsignedInt(UnsignedIntTy::U16),
            "u32" => Self::UnsignedInt(UnsignedIntTy::U32),
            "u64" => Self::UnsignedInt(UnsignedIntTy::U64),
            "u128" => Self::UnsignedInt(UnsignedIntTy::U128),
            "usize" => Self::UnsignedInt(UnsignedIntTy::Usize),
            "f32" => Self::Float(FloatTy::F32),
            "f64" => Self::Float(FloatTy::F64),
            _ => return None,
        })
    }

    pub fn from_integer_suffix(suffix: Option<&str>) -> Option<Self> {
        match suffix {
            Some(suffix) => Self::from_name(suffix).filter(|ty| ty.is_integral()),
            None => Some(Self::DEFAULT_INT),
        }
    }

    pub fn from_float_suffix(suffix: Option<&str>) -> Option<Self> {
        match suffix {
            Some(suffix) => Self::from_name(suffix).filter(|ty| ty.is_float()),
            None => Some(Self::DEFAULT_FLOAT),
        }
    }

    pub fn is_bool(self) -> bool {
        matches!(self, Self::Bool)
    }

    pub fn is_integral(self) -> bool {
        matches!(self, Self::SignedInt(_) | Self::UnsignedInt(_))
    }

    pub fn is_float(self) -> bool {
        matches!(self, Self::Float(_))
    }

    pub fn is_numeric(self) -> bool {
        self.is_integral() || self.is_float()
    }

    pub fn is_signed_numeric(self) -> bool {
        matches!(self, Self::SignedInt(_) | Self::Float(_))
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

impl SignedIntTy {
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

impl UnsignedIntTy {
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

impl FloatTy {
    pub fn label(self) -> &'static str {
        match self {
            Self::F32 => "f32",
            Self::F64 => "f64",
        }
    }
}
