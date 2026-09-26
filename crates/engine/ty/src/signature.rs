//! Query the interpreted types and bounds of known declarations.
//!
//! A caller supplies a declaration identity, such as a function or field, and receives owned
//! types it can inspect, substitute, or retain. Queries that interpret source types use the shared
//! lowerer in temporary storage and export the result before returning. A whole signature is
//! lowered in one session so its parameters and `impl Trait` occurrences keep the same identities.

use rg_def_map::DefMapSource;
use rg_ir_model::{
    ConstRef, EnumVariantRef, FieldRef, FunctionRef, GenericDefRef, ImplRef, StaticRef,
    TraitDefRef, TypeAliasRef, TypeParamRef,
};
use rg_item_tree::FunctionQualifiers;
use rg_semantic_ir::ItemStoreSource;

use crate::{
    AssocTypeBinding, Clause, OpaqueTy, Substitution, TraitApplication, TraitRefLowering, Ty,
    lookup::ItemPathQuery,
    lowering::{TypeLoweringQuery, TypePathResolver},
};

/// One function's parameter types, return type, and qualifiers before choosing call arguments.
///
/// For `fn id<T>(value: T) -> T`, both types refer to the function's declared `T`. A caller can
/// inspect those generic types or substitute the arguments chosen for a particular call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallableSignature {
    pub params: Vec<Ty>,
    pub ret: Ty,
    /// Keep qualifiers beside the lowered types so semantic consumers do not have to reopen the
    /// source-shaped declaration to distinguish safe, unsafe, and async function items.
    pub qualifiers: FunctionQualifiers,
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

/// Semantic declaration queries. Results borrow no syntax and are safe to pass between type
/// algorithms within the request that owns the underlying package transaction.
pub struct SemanticSignatureQuery<'query, D, I, R = ItemPathQuery<'query, D, I>> {
    item_paths: ItemPathQuery<'query, D, I>,
    resolver: R,
}

impl<'query, D, I> SemanticSignatureQuery<'query, D, I>
where
    D: DefMapSource + Clone,
    I: ItemStoreSource<'query, Error = D::Error>,
{
    pub fn new(def_maps: D, items: I) -> Self {
        Self {
            item_paths: ItemPathQuery::new(def_maps.clone(), items.clone()),
            resolver: ItemPathQuery::new(def_maps, items),
        }
    }
}

impl<'query, D, I, R> SemanticSignatureQuery<'query, D, I, R>
where
    D: DefMapSource,
    I: ItemStoreSource<'query, Error = D::Error>,
    R: TypePathResolver<Error = D::Error>,
{
    /// Build signature queries with the path semantics of the requesting layer.
    ///
    /// Body IR supplies lexical lookup here for body-local declarations. Ordinary item queries
    /// use `new`, whose resolver is definition-level lookup over the same semantic stores.
    pub fn with_resolver(def_maps: D, items: I, resolver: R) -> Self {
        Self {
            item_paths: ItemPathQuery::new(def_maps, items),
            resolver,
        }
    }

    /// Lower a function's parameter and return types into the shared semantic vocabulary.
    ///
    /// Parameters are visited in source order before the return type. Keeping that walk in one
    /// session gives argument-position parameters and opaque return occurrences repeatable
    /// owner-local identities.
    pub fn function(&self, function: FunctionRef) -> Result<Option<CallableSignature>, D::Error> {
        let lowering = TypeLoweringQuery::new(&self.item_paths, &self.resolver);
        lowering.with_storage(|cx| {
            Ok(lowering
                .function(cx, function)?
                .map(|value| value.raise(cx)))
        })
    }

    /// Build the use-site type of one function item with its complete generic arity.
    ///
    /// Path lookup has no inference table in which to allocate use-site variables. Keep each
    /// generic position as a kind-correct unknown instead of leaking the declaration's own `T`
    /// into the body. Direct calls replace these placeholders with stable call-owned slots.
    pub fn function_item_ty(&self, function: FunctionRef) -> Result<Option<Ty>, D::Error> {
        if self.item_paths.items().function_data(function)?.is_none() {
            return Ok(None);
        }
        let generics = self
            .item_paths
            .generics()
            .generics(GenericDefRef::Function(function))?;
        let args = Substitution::new().args_for(&generics);
        Ok(Some(Ty::fn_def_with_args(function, args)))
    }

    /// Return the trait bounds carried by one function-owned type parameter.
    ///
    /// This is especially useful for argument-position `impl Trait`: the semantic type is a
    /// function-owned parameter, while its `impl Trait` spelling comes from these declaration
    /// predicates rather than from type identity.
    pub fn function_type_param_bounds(
        &self,
        param: TypeParamRef,
    ) -> Result<Vec<TraitRefLowering>, D::Error> {
        let GenericDefRef::Function(function) = param.owner else {
            return Ok(Vec::new());
        };
        let lowering = TypeLoweringQuery::new(&self.item_paths, &self.resolver);
        let clauses = lowering.with_storage(|cx| {
            Ok(lowering
                .predicates(cx, function.into())?
                .unwrap_or_default()
                .into_iter()
                .map(|clause| cx.raise_clause(clause))
                .collect::<Vec<_>>())
        })?;
        let subject = Ty::Param(param);
        let mut bounds = Vec::new();
        for clause in &clauses {
            let Clause::Implemented(application) = clause else {
                continue;
            };
            if application.self_ty() != Some(&subject) {
                continue;
            }
            let associated_types = clauses
                .iter()
                .filter_map(|clause| {
                    let Clause::AliasEq { alias, ty } = clause else {
                        return None;
                    };
                    (alias.args == application.args).then(|| AssocTypeBinding {
                        associated_ty: alias.associated_ty,
                        ty: ty.clone(),
                    })
                })
                .collect();
            bounds.push(TraitRefLowering {
                application: application.clone(),
                associated_types,
            });
        }
        Ok(bounds)
    }

    pub fn field_ty(&self, field: FieldRef) -> Result<Option<Ty>, D::Error> {
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
    ) -> Result<Option<Ty>, D::Error> {
        let lowering = TypeLoweringQuery::new(&self.item_paths, &self.resolver);
        lowering.with_storage(|cx| {
            Ok(lowering
                .enum_variant_field_ty(cx, variant, field_index)?
                .map(|value| cx.raise_ty(value).unwrap_or(Ty::Unknown)))
        })
    }

    pub fn impl_header(&self, impl_ref: ImplRef) -> Result<Option<ImplHeader>, D::Error> {
        let lowering = TypeLoweringQuery::new(&self.item_paths, &self.resolver);
        lowering.with_storage(|cx| {
            Ok(lowering
                .impl_header(cx, impl_ref)?
                .map(|value| value.raise(cx)))
        })
    }

    pub fn type_alias_ty(&self, alias: TypeAliasRef) -> Result<Option<Ty>, D::Error> {
        let lowering = TypeLoweringQuery::new(&self.item_paths, &self.resolver);
        lowering.with_storage(|cx| {
            Ok(lowering
                .type_alias_ty(cx, alias)?
                .map(|value| cx.raise_ty(value).unwrap_or(Ty::Unknown)))
        })
    }

    /// Returns the predicates declared by one opaque occurrence.
    ///
    /// Bounds are queried declaration data, not part of opaque type equality. Replaying the
    /// owner's canonical lowering session keeps occurrence IDs and nested alias traversal aligned
    /// with the type that introduced the opaque identity.
    pub fn opaque_bounds(
        &self,
        opaque: &OpaqueTy,
    ) -> Result<Option<Vec<TraitRefLowering>>, D::Error> {
        let lowering = TypeLoweringQuery::new(&self.item_paths, &self.resolver);
        let bounds = lowering.with_storage(|cx| {
            Ok(lowering
                .opaque_bounds_for_owner(cx, opaque.opaque.owner)?
                .into_iter()
                .find_map(|(candidate, bounds)| {
                    (candidate.opaque == opaque.opaque)
                        .then(|| bounds.iter().map(|b| b.raise(cx)).collect::<Vec<_>>())
                }))
        })?;
        let Some(bounds) = bounds else {
            return Ok(None);
        };
        let generics = self.item_paths.generics().generics(opaque.opaque.owner)?;
        let subst = Substitution::from_args(&generics, &opaque.args);
        Ok(Some(
            bounds
                .iter()
                .map(|bound| subst.apply_trait_ref(bound))
                .collect(),
        ))
    }

    pub fn const_ty(&self, konst: ConstRef) -> Result<Option<Ty>, D::Error> {
        let lowering = TypeLoweringQuery::new(&self.item_paths, &self.resolver);
        lowering.with_storage(|cx| {
            Ok(lowering
                .const_ty(cx, konst)?
                .map(|value| cx.raise_ty(value).unwrap_or(Ty::Unknown)))
        })
    }

    pub fn static_ty(&self, static_ref: StaticRef) -> Result<Option<Ty>, D::Error> {
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
    I: ItemStoreSource<'query, Error = D::Error>,
{
    pub(crate) fn trait_header_from(
        item_paths: &ItemPathQuery<'query, D, I>,
        trait_ref: TraitDefRef,
    ) -> Result<Option<TraitHeader>, D::Error> {
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
