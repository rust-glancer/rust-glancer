//! Query the interpreted types and bounds of known declarations.
//!
//! A caller supplies a declaration identity, such as a function or field, and receives owned
//! types it can inspect, substitute, or retain. Queries that interpret source types use the shared
//! lowerer in temporary storage and export the result before returning. A whole signature is
//! lowered in one session so its parameters and `impl Trait` occurrences keep the same identities.

use std::{cell::RefCell, collections::HashMap};

use rg_def_map::DefMapSource;
use rg_ir_model::{
    ConstRef, EnumVariantRef, FieldRef, FunctionRef, GenericDefRef, GenericParamRef, ImplRef,
    OpaqueTyId, StaticRef, TraitDefRef, TypeAliasRef, TypeParamRef,
};
use rg_item_tree::FunctionQualifiers;
use rg_semantic_ir::{GenericParamSource, Generics, SemanticItemSource};
use rg_source::ReadError;
use rustc_type_ir::TypeVisitableExt as _;

use crate::{
    Clause, ConstructorRef, GenericArg, GenericArgs, OpaqueTy, Substitution, TraitApplication,
    TraitRefLowering, Ty,
    lookup::ItemPathQuery,
    lowering::{TypeLoweringAnchor, TypeLoweringEnv, TypeLoweringQuery, TypePathResolver},
};

/// One function's parameter types, return type, and qualifiers before choosing call arguments.
///
/// For `fn id<T>(value: T) -> T`, both types refer to the function's declared `T`. A caller can
/// inspect those generic types or substitute the arguments chosen for a particular call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallableSignature {
    pub params: Vec<Ty>,
    /// The type of the call expression. For an async function this is the future.
    pub call_return: Ty,
    pub(crate) async_output: Option<Box<Ty>>,
    /// Keep qualifiers beside the lowered types so semantic consumers do not have to reopen the
    /// source-shaped declaration to distinguish safe, unsafe, and async function items.
    pub qualifiers: FunctionQualifiers,
}

impl CallableSignature {
    /// The type written after `->`, or unit when the declaration omits it.
    pub fn body_output(&self) -> &Ty {
        self.async_output.as_deref().unwrap_or(&self.call_return)
    }
}

/// The receiver and positional trait arguments of an impl, before checking its requirements.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImplHeader {
    pub owner: ImplRef,
    pub self_ty: Ty,
    pub trait_ref: Option<TraitApplication>,
}

/// Trait `Self` and the predicates exposed to the trait solver.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TraitHeader {
    pub owner: TraitDefRef,
    pub self_ty: Ty,
    pub clauses: Vec<Clause>,
}

/// The bounds for every opaque type introduced by one declaration.
///
/// An `async fn` that returns `impl Iterator` has two entries here. One describes the
/// future returned by the call, and the other describes the iterator it produces.
struct OpaqueBounds<'query> {
    generics: Generics<'query>,
    entries: Vec<(OpaqueTyId, Vec<TraitRefLowering>)>,
}

impl OpaqueBounds<'_> {
    fn instantiate(&self, opaque: &OpaqueTy) -> Option<Vec<TraitRefLowering>> {
        let (_, bounds) = self
            .entries
            .iter()
            .find(|(id, _)| *id == opaque.opaque.id)?;
        let subst = Substitution::from_args(&self.generics, &opaque.args);
        Some(
            bounds
                .iter()
                .map(|bound| subst.apply_trait_ref(bound))
                .collect(),
        )
    }
}

/// Semantic declaration queries. Results borrow no syntax and are safe to pass between type
/// algorithms within the request that owns the underlying package transaction.
pub struct SemanticSignatureQuery<'query, D, I, R = ItemPathQuery<'query, D, I>> {
    item_paths: ItemPathQuery<'query, D, I>,
    resolver: R,
    opaque_bounds_by_owner: RefCell<HashMap<GenericDefRef, Option<OpaqueBounds<'query>>>>,
}

impl<'query, D, I> SemanticSignatureQuery<'query, D, I>
where
    D: DefMapSource + Clone,
    I: SemanticItemSource<'query>,
{
    pub fn new(def_maps: D, items: I) -> Self {
        Self::with_resolver(
            def_maps.clone(),
            items.clone(),
            ItemPathQuery::new(def_maps, items),
        )
    }
}

impl<'query, D, I, R> SemanticSignatureQuery<'query, D, I, R>
where
    D: DefMapSource,
    I: SemanticItemSource<'query>,
    R: TypePathResolver,
{
    /// Build signature queries with the path semantics of the requesting layer.
    ///
    /// Body IR supplies lexical lookup here for body-local declarations. Ordinary item queries
    /// use `new`, whose resolver is definition-level lookup over the same semantic stores.
    pub fn with_resolver(def_maps: D, items: I, resolver: R) -> Self {
        Self {
            item_paths: ItemPathQuery::new(def_maps, items),
            resolver,
            opaque_bounds_by_owner: RefCell::default(),
        }
    }

    /// Lower a function's parameter and return types into the shared semantic vocabulary.
    ///
    /// Parameters are visited in source order before the return type. Keeping that walk in one
    /// session gives argument-position parameters and opaque return occurrences repeatable
    /// owner-local identities.
    pub fn function(&self, function: FunctionRef) -> Result<Option<CallableSignature>, ReadError> {
        let lowering = TypeLoweringQuery::new(&self.item_paths, &self.resolver);
        lowering.with_storage(|cx| {
            Ok(lowering
                .function(cx, function)?
                .map(|value| value.raise(cx)))
        })
    }

    /// Read the parameter and return types with the tuple constructor's inferred arguments.
    /// For `Wrapper::<u8>(value)`, substitute `u8` into both the field and `Wrapper<u8>`.
    pub fn constructor(
        &self,
        constructor: ConstructorRef,
        args: &GenericArgs,
    ) -> Result<Option<CallableSignature>, ReadError> {
        let lowering = TypeLoweringQuery::new(&self.item_paths, &self.resolver);
        let Some(mut signature) = lowering.with_storage(|cx| {
            Ok(lowering
                .constructor(cx, constructor)?
                .map(|value| value.raise(cx)))
        })?
        else {
            return Ok(None);
        };
        let generics = self
            .item_paths
            .generics()
            .generics(constructor.owner().into())?;
        let substitution = Substitution::from_args(&generics, args);
        for param in &mut signature.params {
            *param = substitution.apply(param);
        }
        signature.call_return = substitution.apply(&signature.call_return);
        Ok(Some(signature))
    }

    /// Read the default written on a type or const parameter.
    ///
    /// For `struct Pair<T, U = Vec<T>>`, the default for `U` is still `Vec<T>`. The caller
    /// substitutes the actual `T` when it knows which `Pair` is being used.
    pub fn generic_param_default(
        &self,
        param: GenericParamRef,
    ) -> Result<Option<GenericArg>, ReadError> {
        let owner = param.owner();
        let generics = self.item_paths.generics().generics(owner)?;
        let Some(param) = generics.iter().find(|data| data.param() == param) else {
            return Ok(None);
        };
        let has_default = match param.source() {
            GenericParamSource::Type(source) => source.default.is_some(),
            GenericParamSource::Const(source) => source.default.is_some(),
            _ => false,
        };
        if !has_default {
            return Ok(None);
        }
        // In `A = Global`, look up `Global` where the parameter was declared, even if the
        // use site has a different type with that name.
        let Some(context) = self
            .item_paths
            .items()
            .type_path_context_for_generic_def(owner)?
        else {
            return Ok(None);
        };
        let lowering = TypeLoweringQuery::new(&self.item_paths, &self.resolver);
        lowering.with_storage(|cx| {
            let mut session = lowering.session(
                cx,
                TypeLoweringEnv::new(owner, TypeLoweringAnchor::Context(context)),
            )?;
            // Failed lowering must not count as a matching default when callers compare args.
            Ok(match param.source() {
                GenericParamSource::Type(source) => match &source.default {
                    Some(default) => {
                        let ty = session.lower_type_ref(default)?;
                        if ty.references_error() {
                            None
                        } else {
                            cx.raise_ty(ty).map(|ty| GenericArg::Type(Box::new(ty)))
                        }
                    }
                    None => None,
                },
                GenericParamSource::Const(source) => match &source.default {
                    Some(default) => {
                        // In `const N: usize = 4`, the declared `usize` tells the lowerer how
                        // to interpret the literal `4`.
                        let ty = source
                            .ty
                            .as_ref()
                            .map(|ty| session.lower_type_ref(ty))
                            .transpose()?
                            .unwrap_or_else(|| cx.unknown());
                        let value = session.lower_const(Some(default.as_str()), ty)?;
                        (!value.references_error())
                            .then(|| GenericArg::Const(cx.raise_const(value)))
                    }
                    None => None,
                },
                _ => None,
            })
        })
    }

    /// Return the trait bounds carried by one function-owned type parameter.
    ///
    /// This is especially useful for argument-position `impl Trait`: the semantic type is a
    /// function-owned parameter, while its `impl Trait` spelling comes from these declaration
    /// predicates rather than from type identity.
    pub fn function_type_param_bounds(
        &self,
        param: TypeParamRef,
    ) -> Result<Vec<TraitRefLowering>, ReadError> {
        let lowering = TypeLoweringQuery::new(&self.item_paths, &self.resolver);
        lowering.with_storage(|cx| {
            Ok(lowering
                .function_type_param_bounds(cx, param)?
                .into_iter()
                .map(|bound| bound.raise(cx))
                .collect())
        })
    }

    pub fn field_ty(&self, field: FieldRef) -> Result<Option<Ty>, ReadError> {
        let lowering = TypeLoweringQuery::new(&self.item_paths, &self.resolver);
        lowering.with_storage(|cx| {
            Ok(lowering
                .field_ty(cx, field)?
                .map(|value| cx.raise_ty(value).unwrap_or(Ty::Unknown)))
        })
    }

    pub fn enum_variant_field_ty(
        &self,
        variant: EnumVariantRef,
        field_index: usize,
    ) -> Result<Option<Ty>, ReadError> {
        let lowering = TypeLoweringQuery::new(&self.item_paths, &self.resolver);
        lowering.with_storage(|cx| {
            Ok(lowering
                .enum_variant_field_ty(cx, variant, field_index)?
                .map(|value| cx.raise_ty(value).unwrap_or(Ty::Unknown)))
        })
    }

    pub fn impl_header(&self, impl_ref: ImplRef) -> Result<Option<ImplHeader>, ReadError> {
        let lowering = TypeLoweringQuery::new(&self.item_paths, &self.resolver);
        lowering.with_storage(|cx| {
            Ok(lowering
                .impl_header(cx, impl_ref)?
                .map(|value| value.raise(cx)))
        })
    }

    pub fn type_alias_ty(&self, alias: TypeAliasRef) -> Result<Option<Ty>, ReadError> {
        let lowering = TypeLoweringQuery::new(&self.item_paths, &self.resolver);
        lowering.with_storage(|cx| {
            Ok(lowering
                .type_alias_ty(cx, alias)?
                .map(|value| cx.raise_ty(value).unwrap_or(Ty::Unknown)))
        })
    }

    /// Returns the bounds for an opaque type with its generic arguments applied.
    ///
    /// For `async fn fetch<T>() -> T`, both `fetch::<u8>()` and `fetch::<bool>()` use
    /// the same cached bound, `Future<Output = T>`. Each call substitutes its own `T`
    /// when asking for its bounds.
    pub fn opaque_bounds(
        &self,
        opaque: &OpaqueTy,
    ) -> Result<Option<Vec<TraitRefLowering>>, ReadError> {
        let owner = opaque.opaque.owner;
        if let Some(cached) = self.opaque_bounds_by_owner.borrow().get(&owner) {
            return Ok(cached
                .as_ref()
                .and_then(|bounds| bounds.instantiate(opaque)));
        }

        // Walk the whole signature once. Lowering can also visit an alias that declares its
        // own opaque types. Those get their own cache entry when a caller asks for them.
        let lowering = TypeLoweringQuery::new(&self.item_paths, &self.resolver);
        let entries = lowering.with_storage(|cx| {
            let mut entries = Vec::new();
            for (candidate, bounds) in lowering.opaque_bounds_for_owner(cx, owner)? {
                if candidate.opaque.owner == owner {
                    let bounds = bounds.iter().map(|bound| bound.raise(cx)).collect();
                    entries.push((candidate.opaque.id, bounds));
                }
            }
            Ok(entries)
        })?;

        // Cache an empty result too, so a missing declaration only needs one lookup.
        let owner_bounds = if entries.is_empty() {
            None
        } else {
            Some(OpaqueBounds {
                generics: self.item_paths.generics().generics(owner)?,
                entries,
            })
        };
        let bounds = owner_bounds
            .as_ref()
            .and_then(|bounds| bounds.instantiate(opaque));
        self.opaque_bounds_by_owner
            .borrow_mut()
            .insert(owner, owner_bounds);
        Ok(bounds)
    }

    pub fn const_ty(&self, konst: ConstRef) -> Result<Option<Ty>, ReadError> {
        let lowering = TypeLoweringQuery::new(&self.item_paths, &self.resolver);
        lowering.with_storage(|cx| {
            Ok(lowering
                .const_ty(cx, konst)?
                .map(|value| cx.raise_ty(value).unwrap_or(Ty::Unknown)))
        })
    }

    pub fn static_ty(&self, static_ref: StaticRef) -> Result<Option<Ty>, ReadError> {
        let lowering = TypeLoweringQuery::new(&self.item_paths, &self.resolver);
        lowering.with_storage(|cx| {
            Ok(lowering
                .static_ty(cx, static_ref)?
                .map(|value| cx.raise_ty(value).unwrap_or(Ty::Unknown)))
        })
    }
}

impl<'query, D, I> SemanticSignatureQuery<'query, D, I>
where
    D: DefMapSource,
    I: SemanticItemSource<'query>,
{
    pub(crate) fn trait_header_from(
        item_paths: &ItemPathQuery<'query, D, I>,
        trait_ref: TraitDefRef,
    ) -> Result<Option<TraitHeader>, ReadError> {
        let lowering = TypeLoweringQuery::new(item_paths, item_paths);
        lowering.with_storage(|cx| {
            Ok(lowering
                .trait_header(cx, trait_ref)?
                .map(|header| TraitHeader {
                    owner: header.owner,
                    self_ty: cx.raise_ty(header.self_ty).unwrap_or(Ty::Unknown),
                    clauses: header
                        .clauses
                        .into_iter()
                        .map(|clause| cx.raise_clause(clause))
                        .collect(),
                }))
        })
    }
}
