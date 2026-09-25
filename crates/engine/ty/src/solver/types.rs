//! The type representations expected by the compiler's inference and solver algorithms.
//!
//! Glancer's saved `crate::Ty` values own their contents. These types instead borrow an operation's
//! arena and can contain live inference variables, such as the `?T` in `Vec<?T>`. The wrappers let
//! rustc_type_ir use that storage while definition and parameter identities remain Glancer ids.
//!
//! The traversal and relation hooks below delegate to rustc_type_ir. In particular,
//! a type variable or an alias must reach the inference context instead of being compared as a
//! nominal type.

use std::{
    fmt,
    hash::{Hash, Hasher},
    ops::Deref,
};

use rg_ir_model::{
    FunctionRef, GenericParamRef, ImplRef, OpaqueTyRef, TraitDefRef, TypeAliasRef, TypeDefRef,
};
use rustc_type_ir::{
    self as ir, TypeFoldable, TypeSuperFoldable, TypeSuperVisitable, TypeVisitable, Upcast,
    inherent::{Const as _, GenericArg as _, IntoKind, SliceLike, Ty as _},
    relate::{Relate, RelateResult, TypeRelation},
};

use super::{SolverInterner, interner::ListElement};

type Interner<'s> = SolverInterner<'s>;
pub type GenericArgs<'s> = List<'s, GenericArg<'s>>;
pub type TyKind<'s> = ir::TyKind<Interner<'s>>;

// Types, constants, and predicates are interned by kind. Pointer equality is then enough for
// structural equality, while cached flags keep solver traversal proportional to changed data.
// For example, resolving variables can skip `Vec<u8>` but must visit `Vec<?T>`. These handles
// are compared within the shared storage that created them.
macro_rules! interned_value {
    ($name:ident, $kind:ty, $visit:ident, $fold:ident, $try_fold:ident) => {
        #[derive(Clone, Copy)]
        pub struct $name<'s>(pub(crate) &'s ir::WithCachedTypeInfo<$kind>);

        impl PartialEq for $name<'_> {
            fn eq(&self, other: &Self) -> bool {
                std::ptr::eq(self.0, other.0)
            }
        }

        impl Eq for $name<'_> {}

        impl Hash for $name<'_> {
            fn hash<H: Hasher>(&self, h: &mut H) {
                std::ptr::hash(self.0, h);
            }
        }

        impl fmt::Debug for $name<'_> {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.internee.fmt(f)
            }
        }

        impl<'s> IntoKind for $name<'s> {
            type Kind = $kind;
            fn kind(self) -> Self::Kind {
                self.0.internee
            }
        }

        impl ir::Flags for $name<'_> {
            fn flags(&self) -> ir::TypeFlags {
                self.0.flags
            }

            fn outer_exclusive_binder(&self) -> ir::DebruijnIndex {
                self.0.outer_exclusive_binder
            }
        }

        impl<'s> TypeVisitable<Interner<'s>> for $name<'s> {
            fn visit_with<V: ir::TypeVisitor<Interner<'s>>>(&self, v: &mut V) -> V::Result {
                v.$visit(*self)
            }
        }

        impl<'s> TypeFoldable<Interner<'s>> for $name<'s> {
            fn try_fold_with<F: ir::FallibleTypeFolder<Interner<'s>>>(
                self,
                f: &mut F,
            ) -> Result<Self, F::Error> {
                f.$try_fold(self)
            }

            fn fold_with<F: ir::TypeFolder<Interner<'s>>>(self, f: &mut F) -> Self {
                f.$fold(self)
            }
        }
    };
}

macro_rules! value_kind {
    ($name:ident, $kind:ident, $($variant:ident),+) => {
        impl<'s> IntoKind for $name<'s> {
            type Kind = ir::$kind<Interner<'s>>;

            fn kind(self) -> Self::Kind {
                self.0
            }
        }

        impl Hash for $name<'_> {
            fn hash<H: Hasher>(&self, h: &mut H) {
                std::mem::discriminant(&self.0).hash(h);
                match self.0 {
                    $( ir::$kind::$variant(v) => v.hash(h), )+
                }
            }
        }

        impl<'s> TypeVisitable<Interner<'s>> for $name<'s> {
            fn visit_with<V: ir::TypeVisitor<Interner<'s>>>(&self, v: &mut V) -> V::Result {
                match self.0 {
                    $( ir::$kind::$variant(t) => t.visit_with(v), )+
                }
            }
        }

        impl<'s> TypeFoldable<Interner<'s>> for $name<'s> {
            fn try_fold_with<F: ir::FallibleTypeFolder<Interner<'s>>>(
                self,
                f: &mut F,
            ) -> Result<Self, F::Error> {
                Ok(Self(match self.0 {
                    $( ir::$kind::$variant(t) => ir::$kind::$variant(t.try_fold_with(f)?), )+
                }))
            }

            fn fold_with<F: ir::TypeFolder<Interner<'s>>>(self, f: &mut F) -> Self {
                Self(match self.0 {
                    $( ir::$kind::$variant(t) => ir::$kind::$variant(t.fold_with(f)), )+
                })
            }
        }

        impl<'s> Relate<Interner<'s>> for $name<'s> {
            fn relate<R: TypeRelation<Interner<'s>>>(r: &mut R, a: Self, b: Self) -> RelateResult<Interner<'s>, Self> {
                Ok(Self(match (a.0, b.0) {
                    $(
                        (ir::$kind::$variant(a), ir::$kind::$variant(b)) =>
                            ir::$kind::$variant(r.relate(a, b)?),
                    )+
                    _ => return Err(ir::error::TypeError::Mismatch),
                }))
            }
        }
    };
}

macro_rules! predicate_upcast {
    ($source:ty, $convert:expr) => {
        impl<'s> ir::UpcastFrom<Interner<'s>, $source> for Clause<'s> {
            fn upcast_from(v: $source, cx: Interner<'s>) -> Self {
                ($convert)(v).upcast(cx)
            }
        }

        impl<'s> ir::UpcastFrom<Interner<'s>, ir::Binder<Interner<'s>, $source>> for Clause<'s> {
            fn upcast_from(v: ir::Binder<Interner<'s>, $source>, cx: Interner<'s>) -> Self {
                v.map_bound($convert).upcast(cx)
            }
        }

        impl<'s> ir::UpcastFrom<Interner<'s>, $source> for Predicate<'s> {
            fn upcast_from(v: $source, cx: Interner<'s>) -> Self {
                let c: Clause<'s> = v.upcast(cx);
                c.0
            }
        }

        impl<'s> ir::UpcastFrom<Interner<'s>, ir::Binder<Interner<'s>, $source>> for Predicate<'s> {
            fn upcast_from(v: ir::Binder<Interner<'s>, $source>, cx: Interner<'s>) -> Self {
                let c: Clause<'s> = v.upcast(cx);
                c.0
            }
        }
    };
}

impl<'s> ir::inherent::Span<Interner<'s>> for () {
    fn dummy() {}
}

// The compiler's error visitor must see this token as well as the cached HAS_ERROR flag.
// Using `()` would make traversal silently skip it and violate the solver's error contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ErrorGuaranteed;

impl<'s> TypeVisitable<Interner<'s>> for ErrorGuaranteed {
    fn visit_with<V: ir::TypeVisitor<Interner<'s>>>(&self, visitor: &mut V) -> V::Result {
        visitor.visit_error(*self)
    }
}

impl<'s> TypeFoldable<Interner<'s>> for ErrorGuaranteed {
    fn try_fold_with<F: ir::FallibleTypeFolder<Interner<'s>>>(
        self,
        _: &mut F,
    ) -> Result<Self, F::Error> {
        Ok(self)
    }

    fn fold_with<F: ir::TypeFolder<Interner<'s>>>(self, _: &mut F) -> Self {
        self
    }
}

/// The compiler asks for one definition-id type; retain our distinct source identities inside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DefId {
    Adt(TypeDefRef),
    Trait(TraitDefRef),
    Impl(ImplRef),
    Function(FunctionRef),
    Const(rg_ir_model::ConstRef),
    Static(rg_ir_model::StaticRef),
    TypeAlias(TypeAliasRef),
    Opaque(OpaqueTyRef),
    Closure(crate::ClosureTyId),
    /// A mandatory callback had no source declaration. The operation records that it is
    /// unavailable and discards any resulting proof, rather than inventing a source identity.
    Unavailable,
}

impl DefId {
    pub(crate) fn generic_owner(self) -> Option<rg_ir_model::GenericDefRef> {
        use rg_ir_model::GenericDefRef as G;
        Some(match self {
            Self::Adt(id) => G::TypeDef(id),
            Self::Trait(id) => G::Trait(id),
            Self::Impl(id) => G::Impl(id),
            Self::Function(id) => G::Function(id),
            Self::Const(id) => G::Const(id),
            Self::Static(id) => G::Static(id),
            Self::TypeAlias(id) => G::TypeAlias(id),
            Self::Opaque(id) => id.owner,
            Self::Closure(_) | Self::Unavailable => return None,
        })
    }
}

impl From<rg_ir_model::GenericDefRef> for DefId {
    fn from(owner: rg_ir_model::GenericDefRef) -> Self {
        use rg_ir_model::GenericDefRef as G;
        match owner {
            G::TypeDef(id) => Self::Adt(id),
            G::Trait(id) => Self::Trait(id),
            G::Impl(id) => Self::Impl(id),
            G::Function(id) => Self::Function(id),
            G::TypeAlias(id) => Self::TypeAlias(id),
            G::Const(id) => Self::Const(id),
            G::Static(id) => Self::Static(id),
        }
    }
}

impl<'s> ir::inherent::DefId<Interner<'s>> for DefId {
    // There is no coherence checking here. All supported IDs can be looked up by the provider.
    fn is_local(self) -> bool {
        true
    }

    fn as_local(self) -> Option<Self> {
        Some(self)
    }
}

ir::TrivialTypeTraversalImpls! { DefId, }

/// A parameter's position in the compiler argument list and its identity in source declarations.
/// The position supports compiler substitution; the source id keeps unrelated owners' `T`s
/// distinct when converting types or applying a Glancer substitution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Param {
    pub index: u32,
    pub source: GenericParamRef,
}

impl ir::inherent::ParamLike for Param {
    fn index(self) -> u32 {
        self.index
    }
}

ir::TrivialTypeTraversalImpls! { Param, }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Symbol<'s>(pub &'s str);

impl<'s> ir::inherent::Symbol<Interner<'s>> for Symbol<'s> {
    fn is_kw_underscore_lifetime(self) -> bool {
        self.0 == "'_"
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Safety(pub bool);

impl<'s> ir::inherent::Safety<Interner<'s>> for Safety {
    fn safe() -> Self {
        Self(true)
    }

    fn unsafe_mode() -> Self {
        Self(false)
    }

    fn is_safe(self) -> bool {
        self.0
    }

    fn prefix_str(self) -> &'static str {
        if self.0 { "" } else { "unsafe " }
    }
}

ir::TrivialTypeTraversalImpls! { Safety, }

// Pattern types and const expressions cannot be constructed by Glancer's type lowerer.
// TODO: Give these representations when support for the corresponding source forms is added.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Pattern<'s>(std::convert::Infallible, std::marker::PhantomData<&'s ()>);

impl<'s> Relate<Interner<'s>> for Pattern<'s> {
    fn relate<R: TypeRelation<Interner<'s>>>(
        _: &mut R,
        a: Self,
        _: Self,
    ) -> RelateResult<Interner<'s>, Self> {
        match a.0 {}
    }
}

impl ir::Flags for Pattern<'_> {
    fn flags(&self) -> ir::TypeFlags {
        match self.0 {}
    }

    fn outer_exclusive_binder(&self) -> ir::DebruijnIndex {
        match self.0 {}
    }
}

impl<'s> IntoKind for Pattern<'s> {
    type Kind = ir::PatternKind<Interner<'s>>;
    fn kind(self) -> Self::Kind {
        match self.0 {}
    }
}

impl<'s> TypeVisitable<Interner<'s>> for Pattern<'s> {
    fn visit_with<V: ir::TypeVisitor<Interner<'s>>>(&self, _: &mut V) -> V::Result {
        match self.0 {}
    }
}

impl<'s> TypeFoldable<Interner<'s>> for Pattern<'s> {
    fn try_fold_with<F: ir::FallibleTypeFolder<Interner<'s>>>(
        self,
        _: &mut F,
    ) -> Result<Self, F::Error> {
        match self.0 {}
    }

    fn fold_with<F: ir::TypeFolder<Interner<'s>>>(self, _: &mut F) -> Self {
        match self.0 {}
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConstExpr {}

ir::TrivialTypeTraversalImpls! { ConstExpr, }

impl<'s> Relate<Interner<'s>> for ConstExpr {
    fn relate<R: TypeRelation<Interner<'s>>>(
        _: &mut R,
        a: Self,
        _: Self,
    ) -> RelateResult<Interner<'s>, Self> {
        match a {}
    }
}

impl<'s> ir::inherent::ExprConst<Interner<'s>> for ConstExpr {
    fn args(self) -> GenericArgs<'s> {
        match self {}
    }
}

/// A slice owned by the operation's arena. Construction reuses equal sequences, and copying a
/// list shares its elements. A list may also be a view into another list, such as a signature's
/// inputs without its output, so equality and hashing still describe the sequence contents.
#[derive(Clone, Copy, Eq)]
pub struct List<'s, T: Copy>(pub(crate) &'s [T]);

impl<T: Copy + Eq> PartialEq for List<'_, T> {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self.0, other.0) || self.0 == other.0
    }
}

impl<T: Copy + Hash> Hash for List<'_, T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

impl<T: Copy> Default for List<'_, T> {
    fn default() -> Self {
        Self(&[])
    }
}

impl<T: Copy + fmt::Debug> fmt::Debug for List<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl<'s, T: Copy> List<'s, T> {
    pub fn new(cx: Interner<'s>, values: &[T]) -> Self
    where
        T: ListElement<'s>,
    {
        Self(T::intern(cx, values))
    }

    pub fn as_slice(self) -> &'s [T] {
        self.0
    }

    pub fn iter(self) -> std::iter::Copied<std::slice::Iter<'s, T>> {
        self.0.iter().copied()
    }
}

impl<T: Copy> Deref for List<'_, T> {
    type Target = [T];
    fn deref(&self) -> &Self::Target {
        self.0
    }
}

impl<'s, T: Copy> IntoIterator for List<'s, T> {
    type Item = T;
    type IntoIter = std::iter::Copied<std::slice::Iter<'s, T>>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<'s, T: Copy> SliceLike for List<'s, T> {
    type Item = T;
    type IntoIter = std::iter::Copied<std::slice::Iter<'s, T>>;
    fn iter(self) -> Self::IntoIter {
        self.iter()
    }

    fn as_slice(&self) -> &[T] {
        self.0
    }
}

impl<'s, T: Copy + TypeVisitable<Interner<'s>>> TypeVisitable<Interner<'s>> for List<'s, T> {
    fn visit_with<V: ir::TypeVisitor<Interner<'s>>>(&self, visitor: &mut V) -> V::Result {
        self.0.visit_with(visitor)
    }
}

impl<'s, T: ListElement<'s> + TypeFoldable<Interner<'s>>> TypeFoldable<Interner<'s>>
    for List<'s, T>
{
    fn try_fold_with<F: ir::FallibleTypeFolder<Interner<'s>>>(
        self,
        folder: &mut F,
    ) -> Result<Self, F::Error> {
        let cx = folder.cx();
        // The compiler collector keeps short lists on the stack. An unchanged fold can then
        // reuse this slice without any allocation; changed contents go through the interner.
        ir::CollectAndApply::collect_and_apply(
            self.iter().map(|v| v.try_fold_with(folder)),
            |values: &[T]| {
                if values == self.0 {
                    self
                } else {
                    Self::new(cx, values)
                }
            },
        )
    }

    fn fold_with<F: ir::TypeFolder<Interner<'s>>>(self, folder: &mut F) -> Self {
        let cx = folder.cx();
        ir::CollectAndApply::collect_and_apply(
            self.iter().map(|v| v.fold_with(folder)),
            |values: &[T]| {
                if values == self.0 {
                    self
                } else {
                    Self::new(cx, values)
                }
            },
        )
    }
}

impl<'s> ir::inherent::Clauses<Interner<'s>> for List<'s, Clause<'s>> {}

impl ir::Flags for List<'_, Clause<'_>> {
    fn flags(&self) -> ir::TypeFlags {
        ir::FlagComputation::<Interner<'_>>::for_clauses(self.0).flags
    }

    fn outer_exclusive_binder(&self) -> ir::DebruijnIndex {
        ir::FlagComputation::<Interner<'_>>::for_clauses(self.0).outer_exclusive_binder
    }
}

impl<'s> TypeSuperVisitable<Interner<'s>> for List<'s, Clause<'s>> {
    fn super_visit_with<V: ir::TypeVisitor<Interner<'s>>>(&self, v: &mut V) -> V::Result {
        self.visit_with(v)
    }
}

impl<'s> TypeSuperFoldable<Interner<'s>> for List<'s, Clause<'s>> {
    fn try_super_fold_with<F: ir::FallibleTypeFolder<Interner<'s>>>(
        self,
        f: &mut F,
    ) -> Result<Self, F::Error> {
        self.try_fold_with(f)
    }

    fn super_fold_with<F: ir::TypeFolder<Interner<'s>>>(self, f: &mut F) -> Self {
        self.fold_with(f)
    }
}

impl<'s> ir::inherent::Tys<Interner<'s>> for List<'s, Ty<'s>> {
    fn inputs(self) -> Self {
        Self(&self.0[..self.0.len() - 1])
    }

    fn output(self) -> Ty<'s> {
        *self.0.last().expect("signature includes its output")
    }
}

impl<'s> ir::inherent::BoundExistentialPredicates<Interner<'s>>
    for List<'s, ir::Binder<Interner<'s>, ir::ExistentialPredicate<Interner<'s>>>>
{
    fn principal_def_id(self) -> Option<DefId> {
        self.principal().map(|p| p.skip_binder().def_id)
    }

    fn principal(self) -> Option<ir::Binder<Interner<'s>, ir::ExistentialTraitRef<Interner<'s>>>> {
        self.iter().find_map(|p| match p.skip_binder() {
            ir::ExistentialPredicate::Trait(t) => Some(p.rebind(t)),
            _ => None,
        })
    }

    fn auto_traits(self) -> impl IntoIterator<Item = DefId> {
        self.iter().filter_map(|p| match p.skip_binder() {
            ir::ExistentialPredicate::AutoTrait(t) => Some(t),
            _ => None,
        })
    }

    fn projection_bounds(
        self,
    ) -> impl IntoIterator<Item = ir::Binder<Interner<'s>, ir::ExistentialProjection<Interner<'s>>>>
    {
        self.iter().filter_map(|p| match p.skip_binder() {
            ir::ExistentialPredicate::Projection(t) => Some(p.rebind(t)),
            _ => None,
        })
    }
}

impl<'s> Relate<Interner<'s>>
    for List<'s, ir::Binder<Interner<'s>, ir::ExistentialPredicate<Interner<'s>>>>
{
    fn relate<R: TypeRelation<Interner<'s>>>(
        r: &mut R,
        a: Self,
        b: Self,
    ) -> RelateResult<Interner<'s>, Self> {
        if a == b {
            return Ok(a);
        }

        // Dynamic types are not produced by the source lowerer yet. Record the limitation so
        // an enclosing query cannot turn this conservative mismatch into negative evidence.
        r.cx().unavailable("dynamic type relation");
        Err(ir::error::TypeError::Mismatch)
    }
}

// Relation errors are defined by the compiler integration contract.
#[allow(clippy::result_large_err)]
impl<'s> Relate<Interner<'s>> for GenericArgs<'s> {
    fn relate<R: TypeRelation<Interner<'s>>>(
        relation: &mut R,
        a: Self,
        b: Self,
    ) -> RelateResult<Interner<'s>, Self> {
        if a == b {
            return Ok(a);
        }
        if a.len() != b.len() {
            return Err(ir::error::TypeError::Mismatch);
        }
        let values = a
            .iter()
            .zip(b.iter())
            .map(|(a, b)| relation.relate(a, b))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self::new(relation.cx(), &values))
    }
}

impl<'s> ir::inherent::GenericArgs<Interner<'s>> for GenericArgs<'s> {
    fn rebase_onto(self, cx: Interner<'s>, source: DefId, target: Self) -> Self {
        let parent_count = cx.generics(source).params.len();
        let values = target
            .iter()
            .chain(self.iter().skip(parent_count))
            .collect::<Vec<_>>();
        Self::new(cx, &values)
    }

    fn type_at(self, i: usize) -> Ty<'s> {
        self.0[i].expect_ty()
    }

    fn region_at(self, i: usize) -> Region<'s> {
        self.0[i].expect_region()
    }

    fn const_at(self, i: usize) -> Const<'s> {
        self.0[i].expect_const()
    }

    fn identity_for_item(cx: Interner<'s>, id: DefId) -> Self {
        let params = cx.generics(id).params;
        let args = params
            .iter()
            .copied()
            .enumerate()
            .map(|(index, source)| {
                GenericArg::param(
                    cx,
                    Param {
                        index: index as u32,
                        source,
                    },
                )
            })
            .collect::<Vec<_>>();
        Self::new(cx, &args)
    }

    fn extend_with_error(cx: Interner<'s>, id: DefId, args: &[GenericArg<'s>]) -> Self {
        let params = cx.generics(id).params;
        let args = params
            .iter()
            .enumerate()
            .map(|(i, p)| {
                args.get(i).copied().unwrap_or_else(|| match p {
                    GenericParamRef::Type(_) => Ty::new_error(cx, ErrorGuaranteed).into(),
                    GenericParamRef::Lifetime(_) => Region(ir::ReError(ErrorGuaranteed)).into(),
                    GenericParamRef::Const(_) => Const::new_error(cx, ErrorGuaranteed).into(),
                })
            })
            .collect::<Vec<_>>();
        Self::new(cx, &args)
    }

    fn split_closure_args(self) -> ir::ClosureArgsParts<Interner<'s>> {
        let [parent_args @ .., kind, sig, captures] = self.0 else {
            unreachable!("closure arguments include kind, signature, and captures")
        };
        ir::ClosureArgsParts {
            parent_args: List(parent_args),
            closure_kind_ty: kind.expect_ty(),
            closure_sig_as_fn_ptr_ty: sig.expect_ty(),
            tupled_upvars_ty: captures.expect_ty(),
        }
    }

    fn split_coroutine_closure_args(self) -> ir::CoroutineClosureArgsParts<Interner<'s>> {
        let [parent_args @ .., kind, sig, captures, refs] = self.0 else {
            unreachable!("coroutine closure arguments include their signature and captures")
        };
        ir::CoroutineClosureArgsParts {
            parent_args: List(parent_args),
            closure_kind_ty: kind.expect_ty(),
            signature_parts_ty: sig.expect_ty(),
            tupled_upvars_ty: captures.expect_ty(),
            coroutine_captures_by_ref_ty: refs.expect_ty(),
        }
    }

    fn split_coroutine_args(self) -> ir::CoroutineArgsParts<Interner<'s>> {
        let [parent_args @ .., kind, resume, yielded, output, captures] = self.0 else {
            unreachable!("coroutine arguments include their signature and captures")
        };
        ir::CoroutineArgsParts {
            parent_args: List(parent_args),
            kind_ty: kind.expect_ty(),
            resume_ty: resume.expect_ty(),
            yield_ty: yielded.expect_ty(),
            return_ty: output.expect_ty(),
            tupled_upvars_ty: captures.expect_ty(),
        }
    }
}

interned_value!(Ty, TyKind<'s>, visit_ty, fold_ty, try_fold_ty);

impl<'s> Ty<'s> {
    pub fn new(cx: Interner<'s>, kind: TyKind<'s>) -> Self {
        let kind = match kind {
            ir::Adt(adt, args) => ir::Adt(adt, cx.complete_args(adt.id, args)),
            ir::FnDef(id, args) => ir::FnDef(id, cx.complete_args(id, args)),
            other => other,
        };
        cx.intern_ty(kind)
    }
}

impl<'s> Relate<Interner<'s>> for Ty<'s> {
    fn relate<R: TypeRelation<Interner<'s>>>(
        r: &mut R,
        a: Self,
        b: Self,
    ) -> RelateResult<Interner<'s>, Self> {
        r.tys(a, b)
    }
}

impl<'s> ir::inherent::Ty<Interner<'s>> for Ty<'s> {
    fn new_unit(cx: Interner<'s>) -> Self {
        Self::new(cx, ir::Tuple(List::default()))
    }

    fn new_bool(cx: Interner<'s>) -> Self {
        Self::new(cx, ir::Bool)
    }

    fn new_u8(cx: Interner<'s>) -> Self {
        Self::new(cx, ir::Uint(ir::UintTy::U8))
    }

    fn new_usize(cx: Interner<'s>) -> Self {
        Self::new(cx, ir::Uint(ir::UintTy::Usize))
    }

    fn new_infer(cx: Interner<'s>, var: ir::InferTy) -> Self {
        Self::new(cx, ir::Infer(var))
    }

    fn new_var(cx: Interner<'s>, var: ir::TyVid) -> Self {
        Self::new_infer(cx, ir::TyVar(var))
    }

    fn new_param(cx: Interner<'s>, param: Param) -> Self {
        Self::new(cx, ir::Param(param))
    }

    fn new_placeholder(cx: Interner<'s>, param: ir::PlaceholderType<Interner<'s>>) -> Self {
        Self::new(cx, ir::Placeholder(param))
    }

    fn new_bound(cx: Interner<'s>, d: ir::DebruijnIndex, var: ir::BoundTy<Interner<'s>>) -> Self {
        Self::new(cx, ir::Bound(ir::BoundVarIndexKind::Bound(d), var))
    }

    fn new_anon_bound(cx: Interner<'s>, d: ir::DebruijnIndex, var: ir::BoundVar) -> Self {
        Self::new_bound(
            cx,
            d,
            ir::BoundTy {
                var,
                kind: ir::BoundTyKind::Anon,
            },
        )
    }

    fn new_canonical_bound(cx: Interner<'s>, var: ir::BoundVar) -> Self {
        Self::new(
            cx,
            ir::Bound(
                ir::BoundVarIndexKind::Canonical,
                ir::BoundTy {
                    var,
                    kind: ir::BoundTyKind::Anon,
                },
            ),
        )
    }

    fn new_alias(cx: Interner<'s>, alias: ir::AliasTy<Interner<'s>>) -> Self {
        Self::new(cx, ir::Alias(alias))
    }

    fn new_error(cx: Interner<'s>, _: ErrorGuaranteed) -> Self {
        Self::new(cx, ir::Error(ErrorGuaranteed))
    }

    fn new_adt(cx: Interner<'s>, adt: AdtDef, args: GenericArgs<'s>) -> Self {
        Self::new(cx, ir::Adt(adt, args))
    }

    fn new_foreign(cx: Interner<'s>, id: DefId) -> Self {
        Self::new(cx, ir::Foreign(id))
    }

    fn new_dynamic(
        cx: Interner<'s>,
        preds: List<'s, ir::Binder<Interner<'s>, ir::ExistentialPredicate<Interner<'s>>>>,
        region: Region<'s>,
    ) -> Self {
        Self::new(cx, ir::Dynamic(preds, region))
    }

    fn new_coroutine(cx: Interner<'s>, id: DefId, args: GenericArgs<'s>) -> Self {
        Self::new(cx, ir::Coroutine(id, args))
    }

    fn new_coroutine_closure(cx: Interner<'s>, id: DefId, args: GenericArgs<'s>) -> Self {
        Self::new(cx, ir::CoroutineClosure(id, args))
    }

    fn new_closure(cx: Interner<'s>, id: DefId, args: GenericArgs<'s>) -> Self {
        Self::new(cx, ir::Closure(id, args))
    }

    fn new_coroutine_witness(cx: Interner<'s>, id: DefId, args: GenericArgs<'s>) -> Self {
        Self::new(cx, ir::CoroutineWitness(id, args))
    }

    fn new_coroutine_witness_for_coroutine(
        cx: Interner<'s>,
        id: DefId,
        args: GenericArgs<'s>,
    ) -> Self {
        Self::new_coroutine_witness(cx, id, args)
    }

    fn new_ptr(cx: Interner<'s>, inner: Self, m: ir::Mutability) -> Self {
        Self::new(cx, ir::RawPtr(inner, m))
    }

    fn new_ref(cx: Interner<'s>, region: Region<'s>, inner: Self, m: ir::Mutability) -> Self {
        Self::new(cx, ir::Ref(region, inner, m))
    }

    fn new_array_with_const_len(cx: Interner<'s>, inner: Self, len: Const<'s>) -> Self {
        Self::new(cx, ir::Array(inner, len))
    }

    fn new_slice(cx: Interner<'s>, inner: Self) -> Self {
        Self::new(cx, ir::Slice(inner))
    }

    fn new_tup(cx: Interner<'s>, fields: &[Self]) -> Self {
        Self::new(cx, ir::Tuple(List::new(cx, fields)))
    }

    fn new_tup_from_iter<It, T>(cx: Interner<'s>, iter: It) -> T::Output
    where
        It: Iterator<Item = T>,
        T: ir::CollectAndApply<Self, Self>,
    {
        T::collect_and_apply(iter, |v| Self::new_tup(cx, v))
    }

    fn new_fn_def(cx: Interner<'s>, id: DefId, args: GenericArgs<'s>) -> Self {
        Self::new(cx, ir::FnDef(id, args))
    }

    fn new_fn_ptr(
        cx: Interner<'s>,
        sig: ir::Binder<Interner<'s>, ir::FnSig<Interner<'s>>>,
    ) -> Self {
        let (sig, header) = sig.split();
        Self::new(cx, ir::FnPtr(sig, header))
    }

    fn new_pat(cx: Interner<'s>, inner: Self, pat: Pattern<'s>) -> Self {
        Self::new(cx, ir::Pat(inner, pat))
    }

    fn new_unsafe_binder(cx: Interner<'s>, ty: ir::Binder<Interner<'s>, Self>) -> Self {
        Self::new(cx, ir::UnsafeBinder(ty.into()))
    }

    fn tuple_fields(self) -> List<'s, Self> {
        match self.kind() {
            ir::Tuple(t) => t,
            _ => List::default(),
        }
    }

    fn to_opt_closure_kind(self) -> Option<ir::ClosureKind> {
        match self.kind() {
            ir::Int(ir::IntTy::I8) => Some(ir::ClosureKind::Fn),
            ir::Int(ir::IntTy::I16) => Some(ir::ClosureKind::FnMut),
            ir::Int(ir::IntTy::I32) => Some(ir::ClosureKind::FnOnce),
            _ => None,
        }
    }

    fn from_closure_kind(cx: Interner<'s>, kind: ir::ClosureKind) -> Self {
        Self::new(
            cx,
            ir::Int(match kind {
                ir::ClosureKind::Fn => ir::IntTy::I8,
                ir::ClosureKind::FnMut => ir::IntTy::I16,
                ir::ClosureKind::FnOnce => ir::IntTy::I32,
            }),
        )
    }

    fn from_coroutine_closure_kind(cx: Interner<'s>, kind: ir::ClosureKind) -> Self {
        Self::from_closure_kind(cx, kind)
    }

    fn has_unsafe_fields(self) -> bool {
        false
    }

    fn discriminant_ty(self, cx: Interner<'s>) -> Self {
        // Enum representation attributes are not part of the supported semantic model.
        cx.unavailable("enum discriminant type");
        Self::new_error(cx, ErrorGuaranteed)
    }
}

interned_value!(
    Const,
    ir::ConstKind<Interner<'s>>,
    visit_const,
    fold_const,
    try_fold_const
);

impl<'s> Const<'s> {
    pub fn new(cx: Interner<'s>, kind: ir::ConstKind<Interner<'s>>) -> Self {
        cx.intern_const(kind)
    }
}

impl<'s> Relate<Interner<'s>> for Const<'s> {
    fn relate<R: TypeRelation<Interner<'s>>>(
        r: &mut R,
        a: Self,
        b: Self,
    ) -> RelateResult<Interner<'s>, Self> {
        r.consts(a, b)
    }
}

impl<'s> ir::inherent::Const<Interner<'s>> for Const<'s> {
    fn new_infer(cx: Interner<'s>, v: ir::InferConst) -> Self {
        Self::new(cx, ir::ConstKind::Infer(v))
    }

    fn new_var(cx: Interner<'s>, v: ir::ConstVid) -> Self {
        Self::new_infer(cx, ir::InferConst::Var(v))
    }

    fn new_bound(cx: Interner<'s>, d: ir::DebruijnIndex, b: ir::BoundConst<Interner<'s>>) -> Self {
        Self::new(cx, ir::ConstKind::Bound(ir::BoundVarIndexKind::Bound(d), b))
    }

    fn new_anon_bound(cx: Interner<'s>, d: ir::DebruijnIndex, var: ir::BoundVar) -> Self {
        Self::new_bound(cx, d, ir::BoundConst::new(var))
    }

    fn new_canonical_bound(cx: Interner<'s>, var: ir::BoundVar) -> Self {
        Self::new(
            cx,
            ir::ConstKind::Bound(ir::BoundVarIndexKind::Canonical, ir::BoundConst::new(var)),
        )
    }

    fn new_placeholder(cx: Interner<'s>, p: ir::PlaceholderConst<Interner<'s>>) -> Self {
        Self::new(cx, ir::ConstKind::Placeholder(p))
    }

    fn new_unevaluated(cx: Interner<'s>, v: ir::UnevaluatedConst<Interner<'s>>) -> Self {
        Self::new(cx, ir::ConstKind::Unevaluated(v))
    }

    fn new_expr(_: Interner<'s>, e: ConstExpr) -> Self {
        match e {}
    }

    fn new_error(cx: Interner<'s>, _: ErrorGuaranteed) -> Self {
        Self::new(cx, ir::ConstKind::Error(ErrorGuaranteed))
    }
}

interned_value!(
    Predicate,
    ir::Binder<Interner<'s>, ir::PredicateKind<Interner<'s>>>,
    visit_predicate,
    fold_predicate,
    try_fold_predicate
);

impl<'s> Predicate<'s> {
    pub fn new(
        cx: Interner<'s>,
        kind: ir::Binder<Interner<'s>, ir::PredicateKind<Interner<'s>>>,
    ) -> Self {
        cx.intern_predicate(kind)
    }
}

impl<'s> ir::inherent::Predicate<Interner<'s>> for Predicate<'s> {
    fn as_clause(self) -> Option<Clause<'s>> {
        matches!(self.kind().skip_binder(), ir::PredicateKind::Clause(_)).then_some(Clause(self))
    }
}

impl<'s> ir::elaborate::Elaboratable<Interner<'s>> for Predicate<'s> {
    fn predicate(&self) -> Self {
        *self
    }

    fn child(&self, c: Clause<'s>) -> Self {
        c.0
    }

    fn child_with_derived_cause(
        &self,
        c: Clause<'s>,
        _: (),
        _: ir::Binder<Interner<'s>, ir::TraitPredicate<Interner<'s>>>,
        _: usize,
    ) -> Self {
        c.0
    }
}

impl<'s> ir::UpcastFrom<Interner<'s>, ir::Binder<Interner<'s>, ir::PredicateKind<Interner<'s>>>>
    for Predicate<'s>
{
    fn upcast_from(
        v: ir::Binder<Interner<'s>, ir::PredicateKind<Interner<'s>>>,
        cx: Interner<'s>,
    ) -> Self {
        Self::new(cx, v)
    }
}

impl<'s> ir::UpcastFrom<Interner<'s>, ir::PredicateKind<Interner<'s>>> for Predicate<'s> {
    fn upcast_from(v: ir::PredicateKind<Interner<'s>>, cx: Interner<'s>) -> Self {
        ir::Binder::dummy(v).upcast(cx)
    }
}

impl<'s> ir::UpcastFrom<Interner<'s>, Clause<'s>> for Predicate<'s> {
    fn upcast_from(v: Clause<'s>, _: Interner<'s>) -> Self {
        v.0
    }
}

impl<'s> ir::UpcastFrom<Interner<'s>, ir::ClauseKind<Interner<'s>>> for Predicate<'s> {
    fn upcast_from(v: ir::ClauseKind<Interner<'s>>, cx: Interner<'s>) -> Self {
        ir::PredicateKind::Clause(v).upcast(cx)
    }
}

impl<'s> ir::UpcastFrom<Interner<'s>, ir::Binder<Interner<'s>, ir::ClauseKind<Interner<'s>>>>
    for Predicate<'s>
{
    fn upcast_from(
        v: ir::Binder<Interner<'s>, ir::ClauseKind<Interner<'s>>>,
        cx: Interner<'s>,
    ) -> Self {
        v.map_bound(ir::PredicateKind::Clause).upcast(cx)
    }
}

predicate_upcast!(ir::TraitRef<Interner<'s>>, |trait_ref| {
    ir::ClauseKind::Trait(ir::TraitPredicate {
        trait_ref,
        polarity: ir::PredicatePolarity::Positive,
    })
});

predicate_upcast!(ir::TraitPredicate<Interner<'s>>, ir::ClauseKind::Trait);

predicate_upcast!(
    ir::ProjectionPredicate<Interner<'s>>,
    ir::ClauseKind::Projection
);

predicate_upcast!(
    ir::OutlivesPredicate<Interner<'s>, Ty<'s>>,
    ir::ClauseKind::TypeOutlives
);

predicate_upcast!(
    ir::OutlivesPredicate<Interner<'s>, Region<'s>>,
    ir::ClauseKind::RegionOutlives
);

impl<'s> ir::UpcastFrom<Interner<'s>, ir::NormalizesTo<Interner<'s>>> for Predicate<'s> {
    fn upcast_from(v: ir::NormalizesTo<Interner<'s>>, cx: Interner<'s>) -> Self {
        ir::PredicateKind::NormalizesTo(v).upcast(cx)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Region<'s>(pub ir::RegionKind<Interner<'s>>);

impl<'s> IntoKind for Region<'s> {
    type Kind = ir::RegionKind<Interner<'s>>;
    fn kind(self) -> Self::Kind {
        self.0
    }
}

impl<'s> TypeVisitable<Interner<'s>> for Region<'s> {
    fn visit_with<V: ir::TypeVisitor<Interner<'s>>>(&self, v: &mut V) -> V::Result {
        v.visit_region(*self)
    }
}

impl<'s> TypeFoldable<Interner<'s>> for Region<'s> {
    fn try_fold_with<F: ir::FallibleTypeFolder<Interner<'s>>>(
        self,
        f: &mut F,
    ) -> Result<Self, F::Error> {
        f.try_fold_region(self)
    }

    fn fold_with<F: ir::TypeFolder<Interner<'s>>>(self, f: &mut F) -> Self {
        f.fold_region(self)
    }
}

impl<'s> Relate<Interner<'s>> for Region<'s> {
    fn relate<R: TypeRelation<Interner<'s>>>(
        r: &mut R,
        a: Self,
        b: Self,
    ) -> RelateResult<Interner<'s>, Self> {
        r.regions(a, b)
    }
}

impl ir::Flags for Region<'_> {
    fn flags(&self) -> ir::TypeFlags {
        use ir::TypeFlags as F;
        match self.0 {
            ir::ReEarlyParam(_) => F::HAS_RE_PARAM | F::HAS_FREE_REGIONS,
            ir::ReBound(ir::BoundVarIndexKind::Bound(_), _) => F::HAS_RE_BOUND,
            ir::ReBound(ir::BoundVarIndexKind::Canonical, _) => F::HAS_CANONICAL_BOUND,
            ir::ReLateParam(_) | ir::ReStatic => F::HAS_FREE_REGIONS,
            ir::ReVar(_) => F::HAS_RE_INFER | F::HAS_FREE_REGIONS,
            ir::RePlaceholder(_) => F::HAS_RE_PLACEHOLDER | F::HAS_FREE_REGIONS,
            ir::ReErased => F::HAS_RE_ERASED,
            ir::ReError(_) => F::HAS_RE_ERROR | F::HAS_FREE_REGIONS,
        }
    }

    fn outer_exclusive_binder(&self) -> ir::DebruijnIndex {
        match self.0 {
            ir::ReBound(ir::BoundVarIndexKind::Bound(i), _) => i.shifted_in(1),
            _ => ir::INNERMOST,
        }
    }
}

impl<'s> ir::inherent::Region<Interner<'s>> for Region<'s> {
    fn new_bound(_: Interner<'s>, d: ir::DebruijnIndex, b: ir::BoundRegion<Interner<'s>>) -> Self {
        Self(ir::ReBound(ir::BoundVarIndexKind::Bound(d), b))
    }

    fn new_anon_bound(cx: Interner<'s>, d: ir::DebruijnIndex, var: ir::BoundVar) -> Self {
        Self::new_bound(
            cx,
            d,
            ir::BoundRegion {
                var,
                kind: ir::BoundRegionKind::Anon,
            },
        )
    }

    fn new_canonical_bound(_: Interner<'s>, var: ir::BoundVar) -> Self {
        Self(ir::ReBound(
            ir::BoundVarIndexKind::Canonical,
            ir::BoundRegion {
                var,
                kind: ir::BoundRegionKind::Anon,
            },
        ))
    }

    fn new_static(_: Interner<'s>) -> Self {
        Self(ir::ReStatic)
    }

    fn new_placeholder(_: Interner<'s>, p: ir::PlaceholderRegion<Interner<'s>>) -> Self {
        Self(ir::RePlaceholder(p))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GenericArg<'s>(pub ir::GenericArgKind<Interner<'s>>);

value_kind!(GenericArg, GenericArgKind, Type, Const, Lifetime);

impl<'s> From<Ty<'s>> for GenericArg<'s> {
    fn from(v: Ty<'s>) -> Self {
        Self(ir::GenericArgKind::Type(v))
    }
}

impl<'s> From<Const<'s>> for GenericArg<'s> {
    fn from(v: Const<'s>) -> Self {
        Self(ir::GenericArgKind::Const(v))
    }
}

impl<'s> From<Region<'s>> for GenericArg<'s> {
    fn from(v: Region<'s>) -> Self {
        Self(ir::GenericArgKind::Lifetime(v))
    }
}

impl<'s> From<Term<'s>> for GenericArg<'s> {
    fn from(v: Term<'s>) -> Self {
        match v.0 {
            ir::TermKind::Ty(t) => t.into(),
            ir::TermKind::Const(c) => c.into(),
        }
    }
}

impl<'s> ir::inherent::GenericArg<Interner<'s>> for GenericArg<'s> {}

impl<'s> GenericArg<'s> {
    pub(crate) fn param(cx: Interner<'s>, param: Param) -> Self {
        match param.source {
            GenericParamRef::Type(_) => Ty::new_param(cx, param).into(),
            GenericParamRef::Lifetime(_) => Region(ir::ReEarlyParam(param)).into(),
            GenericParamRef::Const(_) => Const::new(cx, ir::ConstKind::Param(param)).into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Term<'s>(pub ir::TermKind<Interner<'s>>);

value_kind!(Term, TermKind, Ty, Const);

impl<'s> From<Ty<'s>> for Term<'s> {
    fn from(v: Ty<'s>) -> Self {
        Self(ir::TermKind::Ty(v))
    }
}

impl<'s> From<Const<'s>> for Term<'s> {
    fn from(v: Const<'s>) -> Self {
        Self(ir::TermKind::Const(v))
    }
}

impl<'s> ir::inherent::Term<Interner<'s>> for Term<'s> {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ValueConst<'s> {
    pub ty: Ty<'s>,
    pub value: u128,
}

impl<'s> ir::inherent::ValueConst<Interner<'s>> for ValueConst<'s> {
    fn ty(self) -> Ty<'s> {
        self.ty
    }

    fn valtree(self) -> ValTree<'s> {
        ValTree(ir::ValTreeKind::Leaf(self.value))
    }
}

impl<'s> TypeVisitable<Interner<'s>> for ValueConst<'s> {
    fn visit_with<V: ir::TypeVisitor<Interner<'s>>>(&self, v: &mut V) -> V::Result {
        self.ty.visit_with(v)
    }
}

impl<'s> TypeFoldable<Interner<'s>> for ValueConst<'s> {
    fn try_fold_with<F: ir::FallibleTypeFolder<Interner<'s>>>(
        self,
        f: &mut F,
    ) -> Result<Self, F::Error> {
        Ok(Self {
            ty: self.ty.try_fold_with(f)?,
            ..self
        })
    }

    fn fold_with<F: ir::TypeFolder<Interner<'s>>>(self, f: &mut F) -> Self {
        Self {
            ty: self.ty.fold_with(f),
            ..self
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ValTree<'s>(pub ir::ValTreeKind<Interner<'s>>);

impl<'s> IntoKind for ValTree<'s> {
    type Kind = ir::ValTreeKind<Interner<'s>>;
    fn kind(self) -> Self::Kind {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Clause<'s>(pub Predicate<'s>);

impl<'s> IntoKind for Clause<'s> {
    type Kind = ir::Binder<Interner<'s>, ir::ClauseKind<Interner<'s>>>;
    fn kind(self) -> Self::Kind {
        self.0.kind().map_bound(|p| match p {
            ir::PredicateKind::Clause(c) => c,
            _ => unreachable!("clause constructed from a non-clause predicate"),
        })
    }
}

impl<'s> TypeVisitable<Interner<'s>> for Clause<'s> {
    fn visit_with<V: ir::TypeVisitor<Interner<'s>>>(&self, v: &mut V) -> V::Result {
        self.0.visit_with(v)
    }
}

impl<'s> TypeFoldable<Interner<'s>> for Clause<'s> {
    fn try_fold_with<F: ir::FallibleTypeFolder<Interner<'s>>>(
        self,
        f: &mut F,
    ) -> Result<Self, F::Error> {
        Ok(Self(self.0.try_fold_with(f)?))
    }

    fn fold_with<F: ir::TypeFolder<Interner<'s>>>(self, f: &mut F) -> Self {
        Self(self.0.fold_with(f))
    }
}

impl<'s> ir::inherent::Clause<Interner<'s>> for Clause<'s> {
    fn as_predicate(self) -> Predicate<'s> {
        self.0
    }

    fn instantiate_supertrait(
        self,
        cx: Interner<'s>,
        tr: ir::Binder<Interner<'s>, ir::TraitRef<Interner<'s>>>,
    ) -> Self {
        // Let the shared binder operation merge higher-ranked variables before substitution.
        let vars = tr
            .bound_vars()
            .iter()
            .chain(self.kind().bound_vars().iter())
            .collect::<Vec<_>>();
        let shifted =
            cx.shift_bound_indices(self.kind().skip_binder(), tr.bound_vars().len() as u32);
        let clause = ir::EarlyBinder::bind(shifted)
            .instantiate(cx, tr.skip_binder().args)
            .skip_norm_wip();
        ir::Binder::bind_with_vars(clause, List::new(cx, &vars)).upcast(cx)
    }
}

impl<'s> ir::elaborate::Elaboratable<Interner<'s>> for Clause<'s> {
    fn predicate(&self) -> Predicate<'s> {
        self.0
    }

    fn child(&self, c: Self) -> Self {
        c
    }

    fn child_with_derived_cause(
        &self,
        c: Self,
        _: (),
        _: ir::Binder<Interner<'s>, ir::TraitPredicate<Interner<'s>>>,
        _: usize,
    ) -> Self {
        c
    }
}

impl<'s> ir::UpcastFrom<Interner<'s>, ir::Binder<Interner<'s>, ir::ClauseKind<Interner<'s>>>>
    for Clause<'s>
{
    fn upcast_from(
        v: ir::Binder<Interner<'s>, ir::ClauseKind<Interner<'s>>>,
        cx: Interner<'s>,
    ) -> Self {
        Self(v.upcast(cx))
    }
}

impl<'s> ir::UpcastFrom<Interner<'s>, ir::ClauseKind<Interner<'s>>> for Clause<'s> {
    fn upcast_from(v: ir::ClauseKind<Interner<'s>>, cx: Interner<'s>) -> Self {
        ir::Binder::dummy(v).upcast(cx)
    }
}

/// Assumptions available where a goal is asked. Inside `fn f<T: Clone>()`, `T: Clone` belongs
/// here: the solver can use that bound without finding a concrete impl for the generic `T`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct ParamEnv<'s>(pub List<'s, Clause<'s>>);

impl<'s> TypeVisitable<Interner<'s>> for ParamEnv<'s> {
    fn visit_with<V: ir::TypeVisitor<Interner<'s>>>(&self, v: &mut V) -> V::Result {
        self.0.visit_with(v)
    }
}

impl<'s> TypeFoldable<Interner<'s>> for ParamEnv<'s> {
    fn try_fold_with<F: ir::FallibleTypeFolder<Interner<'s>>>(
        self,
        f: &mut F,
    ) -> Result<Self, F::Error> {
        Ok(Self(self.0.try_fold_with(f)?))
    }

    fn fold_with<F: ir::TypeFolder<Interner<'s>>>(self, f: &mut F) -> Self {
        Self(self.0.fold_with(f))
    }
}

impl<'s> ir::inherent::ParamEnv<Interner<'s>> for ParamEnv<'s> {
    fn caller_bounds(self) -> impl SliceLike<Item = Clause<'s>> {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AdtDef {
    pub id: DefId,
    pub is_struct: bool,
    pub is_packed: bool,
    pub is_phantom_data: bool,
    pub is_manually_drop: bool,
    pub is_fundamental: bool,
}

impl AdtDef {
    pub(crate) fn new(id: TypeDefRef) -> Self {
        Self {
            id: DefId::Adt(id),
            is_struct: matches!(id.id, rg_ir_model::TypeDefId::Struct(_)),
            // TODO: Carry representation attributes and language-item flags through the source
            // declaration model before enabling the corresponding compiler built-in rules.
            is_packed: false,
            is_phantom_data: false,
            is_manually_drop: false,
            is_fundamental: false,
        }
    }
}

impl<'s> ir::inherent::AdtDef<Interner<'s>> for AdtDef {
    fn def_id(self) -> DefId {
        self.id
    }

    fn is_struct(self) -> bool {
        self.is_struct
    }

    fn is_packed(self) -> bool {
        self.is_packed
    }

    fn is_phantom_data(self) -> bool {
        self.is_phantom_data
    }

    fn is_manually_drop(self) -> bool {
        self.is_manually_drop
    }

    fn is_fundamental(self) -> bool {
        self.is_fundamental
    }

    fn struct_tail_ty(self, cx: Interner<'s>) -> Option<ir::EarlyBinder<Interner<'s>, Ty<'s>>> {
        cx.field_tys(self.id)
            .last()
            .copied()
            .map(ir::EarlyBinder::bind)
    }

    fn all_field_tys(
        self,
        cx: Interner<'s>,
    ) -> ir::EarlyBinder<Interner<'s>, impl IntoIterator<Item = Ty<'s>>> {
        ir::EarlyBinder::bind(cx.field_tys(self.id))
    }

    fn sizedness_constraint(
        self,
        cx: Interner<'s>,
        _: ir::solve::SizedTraitKind,
    ) -> Option<ir::EarlyBinder<Interner<'s>, Ty<'s>>> {
        self.struct_tail_ty(cx)
    }

    fn field_representing_type_info(
        self,
        cx: Interner<'s>,
        _: GenericArgs<'s>,
    ) -> Option<ir::FieldInfo<Interner<'s>>> {
        cx.unavailable("field type information");
        None
    }

    fn destructor(self, cx: Interner<'s>) -> Option<ir::solve::AdtDestructorKind> {
        cx.unavailable("destructor information");
        None
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Generics<'s> {
    pub params: &'s [GenericParamRef],
    pub parent_count: usize,
}

impl<'s> ir::inherent::GenericsOf<Interner<'s>> for Generics<'s> {
    fn count(&self) -> usize {
        self.params.len()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ExternalConstraints<'s>(pub &'s ir::solve::ExternalConstraintsData<Interner<'s>>);

impl<'s> Deref for ExternalConstraints<'s> {
    type Target = ir::solve::ExternalConstraintsData<Interner<'s>>;
    fn deref(&self) -> &Self::Target {
        self.0
    }
}

impl<'s> TypeVisitable<Interner<'s>> for ExternalConstraints<'s> {
    fn visit_with<V: ir::TypeVisitor<Interner<'s>>>(&self, v: &mut V) -> V::Result {
        self.0.visit_with(v)
    }
}

impl<'s> TypeFoldable<Interner<'s>> for ExternalConstraints<'s> {
    fn try_fold_with<F: ir::FallibleTypeFolder<Interner<'s>>>(
        self,
        f: &mut F,
    ) -> Result<Self, F::Error> {
        let data = self.0.clone().try_fold_with(f)?;
        Ok(ir::Interner::mk_external_constraints(f.cx(), data))
    }

    fn fold_with<F: ir::TypeFolder<Interner<'s>>>(self, f: &mut F) -> Self {
        let data = self.0.clone().fold_with(f);
        ir::Interner::mk_external_constraints(f.cx(), data)
    }
}
