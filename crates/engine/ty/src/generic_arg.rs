//! Semantic generic arguments and the trait predicates built from them.
//!
//! Every instantiated item carries a full argument list in [`Generics`](rg_semantic_ir::Generics)
//! order. Trait applications keep positional inputs in that list and associated-type equalities
//! beside it, so inference and trait solving read the same unambiguous shape.

use std::fmt;

use rg_ir_model::{ConstParamRef, LifetimeParamRef, TraitDefRef, TypeAliasRef};
use rg_std::{MemorySize, Shrink};
use wincode::{SchemaRead, SchemaWrite};

use crate::{ProjectionTy, Ty};

/// Lifetime argument retained by the semantic type model.
///
/// rust-glancer does not solve regions. Parameter identity and `'static` remain meaningful, while
/// every other concrete lifetime is deliberately erased instead of using source text as semantic
/// identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[shrink(leaf)]
pub enum Lifetime {
    Static,
    Param(LifetimeParamRef),
    Erased,
}

impl fmt::Display for Lifetime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Static => f.write_str("'static"),
            Self::Param(_) | Self::Erased => f.write_str("'_"),
        }
    }
}

/// Const argument retained by the semantic type model.
///
/// Literal integers are enough for the array/generic identities rust-glancer already models.
/// Paths and expressions remain explicitly unknown until const evaluation is in scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[shrink(leaf)]
pub enum ConstValue {
    Scalar(u128),
    Param(ConstParamRef),
    Unknown,
}

impl ConstValue {
    pub fn from_syntax(text: &str) -> Self {
        let normalized = text.replace('_', "");
        let digits = normalized
            .chars()
            .take_while(char::is_ascii_digit)
            .collect::<String>();
        if digits.is_empty() {
            return Self::Unknown;
        }

        digits.parse().map(Self::Scalar).unwrap_or(Self::Unknown)
    }
}

impl From<Option<String>> for ConstValue {
    fn from(value: Option<String>) -> Self {
        value
            .as_deref()
            .map(Self::from_syntax)
            .unwrap_or(Self::Unknown)
    }
}

impl fmt::Display for ConstValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Scalar(value) => value.fmt(f),
            Self::Param(_) | Self::Unknown => f.write_str("_"),
        }
    }
}

/// Full-arity semantic arguments in the order defined by [`Generics`](rg_semantic_ir::Generics).
///
/// Full-arity means that inherited, omitted, and defaulted parameters still have a position. For
/// `trait Lookup<T, const N: usize>`, the application `Table: Lookup<User, 4>` stores
/// `[Table, User, 4]`: implicit `Self` followed by the two written inputs. Consumers can therefore
/// zip arguments with parameters without reconstructing missing slots.
#[derive(
    Debug, Clone, Default, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink,
)]
pub struct GenericArgs(
    #[wincode(with = "rg_wincode_utils::WincodeDynamic<Vec<GenericArg>>")] Vec<GenericArg>,
);

impl GenericArgs {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn as_slice(&self) -> &[GenericArg] {
        &self.0
    }

    pub fn iter(&self) -> std::slice::Iter<'_, GenericArg> {
        self.0.iter()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub(crate) fn into_vec(self) -> Vec<GenericArg> {
        self.0
    }
}

impl From<Vec<GenericArg>> for GenericArgs {
    fn from(args: Vec<GenericArg>) -> Self {
        Self(args)
    }
}

impl FromIterator<GenericArg> for GenericArgs {
    fn from_iter<T: IntoIterator<Item = GenericArg>>(iter: T) -> Self {
        Self(iter.into_iter().collect())
    }
}

impl std::ops::Deref for GenericArgs {
    type Target = [GenericArg];

    fn deref(&self) -> &Self::Target {
        self.as_slice()
    }
}

impl<'a> IntoIterator for &'a GenericArgs {
    type Item = &'a GenericArg;
    type IntoIter = std::slice::Iter<'a, GenericArg>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

/// Generic argument as understood by the shared type vocabulary.
#[derive(Debug, Clone, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub enum GenericArg {
    Type(#[wincode(with = "rg_wincode_utils::WincodeDynamic<Box<Ty>>")] Box<Ty>),
    Lifetime(Lifetime),
    Const(ConstValue),
}

impl GenericArg {
    pub fn as_ty(&self) -> Option<&Ty> {
        match self {
            Self::Type(ty) => Some(ty),
            Self::Lifetime(_) | Self::Const(_) => None,
        }
    }

    /// Returns true when this generic argument contains `Ty::Unknown`.
    pub fn has_unknown(&self) -> bool {
        match self {
            Self::Type(ty) => ty.has_unknown(),
            Self::Lifetime(_) | Self::Const(_) => false,
        }
    }

    pub(crate) fn has_projection(&self) -> bool {
        match self {
            Self::Type(ty) => ty.has_projection(),
            Self::Lifetime(_) | Self::Const(_) => false,
        }
    }

    pub(crate) fn has_closure(&self) -> bool {
        match self {
            Self::Type(ty) => ty.has_closure(),
            Self::Lifetime(_) | Self::Const(_) => false,
        }
    }

    pub(crate) fn is_projectable(&self) -> bool {
        match self {
            Self::Type(ty) => ty.is_projectable(),
            Self::Lifetime(_) | Self::Const(_) => true,
        }
    }
}

/// A trait definition applied to its full semantic argument list.
///
/// The first argument is the trait `Self` parameter. Associated type equalities are represented
/// separately and therefore cannot accidentally change positional arity.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TraitApplication {
    pub def: TraitDefRef,
    pub args: GenericArgs,
}

impl TraitApplication {
    pub fn self_ty(&self) -> Option<&Ty> {
        self.args.first()?.as_ty()
    }
}

/// One resolved associated-type equality written beside a trait application.
///
/// In `Iterator<Item = User>`, this records the semantic `Iterator::Item` declaration and `User`.
/// The `Self: Iterator` application remains positional and is stored separately.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AssocTypeBinding {
    pub associated_ty: TypeAliasRef,
    pub ty: Ty,
}

/// A lowered trait application together with the associated equalities written beside it.
///
/// For `Iterator<Item = User>`, `application` represents `Self: Iterator` and
/// `associated_types` contains `Iterator::Item = User`. Keeping these parts separate prevents an
/// associated binding from shifting the positional generic arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraitRefLowering {
    pub application: TraitApplication,
    pub associated_types: Vec<AssocTypeBinding>,
}

impl TraitRefLowering {
    pub fn into_clauses(self) -> impl Iterator<Item = Clause> {
        let application = self.application;
        let associated_types = self.associated_types;
        std::iter::once(Clause::Implemented(application.clone())).chain(
            associated_types
                .into_iter()
                .map(move |binding| Clause::AliasEq {
                    alias: ProjectionTy {
                        associated_ty: binding.associated_ty,
                        args: application.args.clone(),
                    },
                    ty: binding.ty,
                }),
        )
    }
}

/// Flat predicate vocabulary consumed by declaration queries and the compiler solver adapter.
///
/// A bound such as `T: Iterator<Item = User>` becomes one `Implemented` clause for
/// `T: Iterator` and one `AliasEq` clause for `<T as Iterator>::Item = User`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Clause {
    Implemented(TraitApplication),
    AliasEq { alias: ProjectionTy, ty: Ty },
}
