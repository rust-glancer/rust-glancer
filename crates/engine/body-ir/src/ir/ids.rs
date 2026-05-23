use rg_def_map::TargetRef;

macro_rules! impl_arena_id {
    ($($id:ty),+ $(,)?) => {
        $(
            impl rg_arena::ArenaId for $id {
                fn from_index(index: usize) -> Self {
                    Self(index)
                }

                fn index(self) -> usize {
                    self.0
                }
            }
        )+
    };
}

/// Stable identifier for one lowered function body inside a target.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    wincode::SchemaRead,
    wincode::SchemaWrite,
    rg_memsize::MemorySize,
)]
#[memsize(leaf)]
pub struct BodyId(pub usize);

/// Stable reference to one lowered function body across the project.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    wincode::SchemaRead,
    wincode::SchemaWrite,
    rg_memsize::MemorySize,
)]
pub struct BodyRef {
    pub target: TargetRef,
    pub body: BodyId,
}

/// Stable identifier for one item declared inside a function body.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    wincode::SchemaRead,
    wincode::SchemaWrite,
    rg_memsize::MemorySize,
)]
#[memsize(leaf)]
pub struct BodyItemId(pub usize);

/// Stable reference to one item declared inside a function body.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    wincode::SchemaRead,
    wincode::SchemaWrite,
    rg_memsize::MemorySize,
)]
pub struct BodyItemRef {
    pub body: BodyRef,
    pub item: BodyItemId,
}

/// Stable identifier for one value item declared inside a function body.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    wincode::SchemaRead,
    wincode::SchemaWrite,
    rg_memsize::MemorySize,
)]
#[memsize(leaf)]
pub struct BodyValueItemId(pub usize);

/// Stable reference to one value item declared inside a function body.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    wincode::SchemaRead,
    wincode::SchemaWrite,
    rg_memsize::MemorySize,
)]
pub struct BodyValueItemRef {
    pub body: BodyRef,
    pub item: BodyValueItemId,
}

/// Stable reference to one local binding inside a body.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    wincode::SchemaRead,
    wincode::SchemaWrite,
    rg_memsize::MemorySize,
)]
pub struct BodyBindingRef {
    pub body: BodyRef,
    pub binding: BindingId,
}

/// Stable identifier for one impl block declared inside a function body.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    wincode::SchemaRead,
    wincode::SchemaWrite,
    rg_memsize::MemorySize,
)]
#[memsize(leaf)]
pub struct BodyImplId(pub usize);

/// Stable reference to one impl block declared inside a function body.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    wincode::SchemaRead,
    wincode::SchemaWrite,
    rg_memsize::MemorySize,
)]
pub struct BodyImplRef {
    pub body: BodyRef,
    pub impl_id: BodyImplId,
}

/// Stable reference to one field declared on a body-local item.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    wincode::SchemaRead,
    wincode::SchemaWrite,
    rg_memsize::MemorySize,
)]
pub struct BodyFieldRef {
    pub item: BodyItemRef,
    pub index: usize,
}

/// Stable reference to one variant declared on a body-local enum item.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    wincode::SchemaRead,
    wincode::SchemaWrite,
    rg_memsize::MemorySize,
)]
pub struct BodyEnumVariantRef {
    pub item: BodyItemRef,
    pub index: usize,
}

/// Stable identifier for one function-like declaration inside a function body.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    wincode::SchemaRead,
    wincode::SchemaWrite,
    rg_memsize::MemorySize,
)]
#[memsize(leaf)]
pub struct BodyFunctionId(pub usize);

/// Stable reference to one function-like declaration inside a function body.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    wincode::SchemaRead,
    wincode::SchemaWrite,
    rg_memsize::MemorySize,
)]
pub struct BodyFunctionRef {
    pub body: BodyRef,
    pub function: BodyFunctionId,
}

/// Stable reference to any declaration contributed by one lowered body.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    derive_more::From,
    wincode::SchemaRead,
    wincode::SchemaWrite,
    rg_memsize::MemorySize,
)]
pub enum BodyDeclarationRef {
    Binding(BodyBindingRef),
    Item(BodyItemRef),
    ValueItem(BodyValueItemRef),
    Impl(BodyImplRef),
    Field(BodyFieldRef),
    EnumVariant(BodyEnumVariantRef),
    Function(BodyFunctionRef),
}

impl BodyDeclarationRef {
    pub fn body(self) -> BodyRef {
        match self {
            Self::Binding(declaration) => declaration.body,
            Self::Item(declaration) => declaration.body,
            Self::ValueItem(declaration) => declaration.body,
            Self::Impl(declaration) => declaration.body,
            Self::Field(declaration) => declaration.item.body,
            Self::EnumVariant(declaration) => declaration.item.body,
            Self::Function(declaration) => declaration.body,
        }
    }
}

/// Stable identifier for one expression inside a body.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    wincode::SchemaRead,
    wincode::SchemaWrite,
    rg_memsize::MemorySize,
)]
#[memsize(leaf)]
pub struct ExprId(pub usize);

/// Stable identifier for one pattern inside a body.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    wincode::SchemaRead,
    wincode::SchemaWrite,
    rg_memsize::MemorySize,
)]
#[memsize(leaf)]
pub struct PatId(pub usize);

/// Stable identifier for one statement inside a body.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    wincode::SchemaRead,
    wincode::SchemaWrite,
    rg_memsize::MemorySize,
)]
#[memsize(leaf)]
pub struct StmtId(pub usize);

/// Stable identifier for one local binding inside a body.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    wincode::SchemaRead,
    wincode::SchemaWrite,
    rg_memsize::MemorySize,
)]
#[memsize(leaf)]
pub struct BindingId(pub usize);

/// Stable identifier for one lexical scope inside a body.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    wincode::SchemaRead,
    wincode::SchemaWrite,
    rg_memsize::MemorySize,
)]
#[memsize(leaf)]
pub struct ScopeId(pub usize);

impl_arena_id!(
    BodyId,
    BodyItemId,
    BodyValueItemId,
    BodyImplId,
    BodyFunctionId,
    ExprId,
    PatId,
    StmtId,
    BindingId,
    ScopeId,
);
