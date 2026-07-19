//! Query-facing generic parameter identities and their declaration data.
//!
//! Item data keeps the names and bounds the user wrote. This module pairs those facts with stable,
//! owner-scoped parameter refs and arranges every visible parameter in the order used by semantic
//! generic arguments.

use rg_ir_model::{GenericDefRef, GenericParamRef};
use rg_item_tree::{ConstParamData, LifetimeParamData, TypeBound, TypeParamData};
use rg_text::Name;

/// Borrowed declaration facts explaining where one semantic parameter came from.
///
/// This is not parameter identity. [`GenericParamView`] keeps the source facts beside the
/// owner-scoped [`GenericParamRef`] without putting names, bounds, or provenance into type
/// equality.
#[derive(Debug, Clone, Copy)]
pub enum GenericParamSource<'a> {
    Lifetime(&'a LifetimeParamData),
    Type(&'a TypeParamData),
    Const(&'a ConstParamData),
    /// The implicit type parameter represented by `Self` inside a trait.
    TraitSelf,
    /// Anonymous function type parameter introduced by argument-position `impl Trait`.
    ///
    /// For `fn visit(value: impl Display)`, the parameter type is this anonymous type parameter;
    /// its `Display` bound remains declaration data here. Return-position `impl Trait` is an opaque
    /// type occurrence instead and does not use this source.
    ArgumentImplTrait(&'a [TypeBound]),
}

/// One owner-scoped parameter together with the syntax facts used while lowering its signature.
#[derive(Debug, Clone, Copy)]
pub struct GenericParamView<'a> {
    param: GenericParamRef,
    source: GenericParamSource<'a>,
}

impl<'a> GenericParamView<'a> {
    pub(crate) fn new(param: GenericParamRef, source: GenericParamSource<'a>) -> Self {
        Self { param, source }
    }

    pub fn param(self) -> GenericParamRef {
        self.param
    }

    pub fn source(self) -> GenericParamSource<'a> {
        self.source
    }

    fn name(self) -> Option<&'a Name> {
        match self.source {
            GenericParamSource::Lifetime(param) => Some(&param.name),
            GenericParamSource::Type(param) => Some(&param.name),
            GenericParamSource::Const(param) => Some(&param.name),
            GenericParamSource::TraitSelf | GenericParamSource::ArgumentImplTrait(_) => None,
        }
    }
}

/// Canonically ordered parameters visible in one signature.
///
/// Parent parameters come first. The owner's own section is trait `Self` when present, lifetimes,
/// then the shared type/const declaration order. Argument-position `impl Trait` parameters follow
/// the explicit parameters. This is the same order used by full-arity semantic argument lists.
///
/// For `trait Store<'a, T, const N: usize>`, the trait's own list is `Self, 'a, T, N`.
/// `Self`, `T`, and `N` share the type/const local-ID lane, while `'a` uses the separate lifetime
/// lane. An associated method prepends this whole parent list before its own parameters.
#[derive(Debug, Clone)]
pub struct Generics<'a> {
    owner: GenericDefRef,
    parent_len: usize,
    params: Vec<GenericParamView<'a>>,
}

impl<'a> Generics<'a> {
    pub(crate) fn new(
        owner: GenericDefRef,
        parent: Option<Self>,
        own_params: Vec<GenericParamView<'a>>,
    ) -> Self {
        let mut params = parent.map(|parent| parent.params).unwrap_or_default();
        let parent_len = params.len();
        params.extend(own_params);
        Self {
            owner,
            parent_len,
            params,
        }
    }

    pub fn owner(&self) -> GenericDefRef {
        self.owner
    }

    pub fn parent_len(&self) -> usize {
        self.parent_len
    }

    pub fn len(&self) -> usize {
        self.params.len()
    }

    pub fn is_empty(&self) -> bool {
        self.params.is_empty()
    }

    pub fn iter(&self) -> impl DoubleEndedIterator<Item = GenericParamView<'a>> + '_ {
        self.params.iter().copied()
    }

    pub fn iter_self(&self) -> impl DoubleEndedIterator<Item = GenericParamView<'a>> + '_ {
        self.params[self.parent_len..].iter().copied()
    }

    /// Resolves a parameter name at this owner, letting local parameters shadow parent names.
    pub fn param_by_name(&self, name: &str) -> Option<GenericParamRef> {
        self.iter().rev().find_map(|param| {
            let matches = match param.source() {
                GenericParamSource::TraitSelf => name == "Self",
                GenericParamSource::Lifetime(_)
                | GenericParamSource::Type(_)
                | GenericParamSource::Const(_) => {
                    param.name().is_some_and(|param_name| param_name == name)
                }
                GenericParamSource::ArgumentImplTrait(_) => false,
            };
            matches.then(|| param.param())
        })
    }
}
