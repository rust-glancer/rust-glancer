//! Lower complete declarations in the active operation's working type storage.
//!
//! Signatures share one source walk so anonymous parameters and opaque occurrences keep their
//! identities. These are templates: call-specific variables are introduced during instantiation.

use rg_def_map::DefMapSource;
use rg_ir_model::{
    ConstRef, EnumVariantRef, FieldRef, FunctionRef, GenericDefRef, GenericParamRef, ImplRef,
    ItemOwner, StaticRef, TraitDefRef, TypeAliasRef,
};
use rg_item_tree::{ParamKind, SelfParamKind};
use rg_semantic_ir::{GenericParamSource, ItemStoreSource, SelfTypeOwner, TypePathContext};

use super::{TypeLoweringAnchor, TypeLoweringEnv, TypeLoweringQuery, TypePathResolver};
use crate::solver::{
    CallableSignature, Clause, ImplHeader, List, OpaqueTy, SolverInterner, TraitRefLowering, Ty,
};

pub(crate) struct TraitHeader<'s> {
    pub owner: TraitDefRef,
    pub self_ty: Ty<'s>,
    pub clauses: Vec<Clause<'s>>,
}

impl<'lower, 'query, D, I, R> TypeLoweringQuery<'lower, 'query, D, I, R>
where
    D: DefMapSource,
    I: ItemStoreSource<'query, Error = D::Error>,
    R: TypePathResolver<Error = D::Error>,
{
    pub fn function<'s>(
        &self,
        cx: SolverInterner<'s>,
        function: FunctionRef,
    ) -> Result<Option<CallableSignature<'s>>, D::Error> {
        let Some(data) = self.item_paths.items().function_data(function)? else {
            return Ok(None);
        };
        let Some(context) = self
            .item_paths
            .items()
            .type_path_context_for_function(function)?
        else {
            return Ok(None);
        };
        let owner = GenericDefRef::Function(function);
        let implicit_self_ty = match data.owner {
            ItemOwner::Impl(id) => self
                .impl_header(
                    cx,
                    ImplRef {
                        origin: function.origin,
                        id,
                    },
                )?
                .map(|header| header.self_ty)
                .unwrap_or_else(|| cx.unknown()),
            ItemOwner::Trait(_) => {
                let generics = self.item_paths.generics().generics(owner)?;
                generics
                    .iter()
                    .enumerate()
                    .find_map(|(index, param)| {
                        matches!(param.source(), GenericParamSource::TraitSelf).then(|| {
                            cx.param_arg(param.param(), index)
                                .as_ty()
                                .expect("Self is a type")
                        })
                    })
                    .unwrap_or_else(|| cx.unknown())
            }
            ItemOwner::Module(_) => cx.unknown(),
        };
        let mut session = self.session(
            cx,
            TypeLoweringEnv::new(owner, TypeLoweringAnchor::Context(context)),
        )?;

        // One session walks parameters in source order so each APIT occurrence receives the same
        // owner-local ID in every query.
        let mut params = Vec::with_capacity(data.signature.params().len());
        for param in data.signature.params() {
            let ty = match &param.ty {
                Some(ty) => session.lower_parameter_type(ty)?,
                None => match param.kind {
                    ParamKind::SelfParam(SelfParamKind::Value) => implicit_self_ty,
                    ParamKind::SelfParam(SelfParamKind::Reference { mutability }) => {
                        if implicit_self_ty.is_unknown() {
                            implicit_self_ty
                        } else {
                            cx.reference(mutability, implicit_self_ty)
                        }
                    }
                    ParamKind::SelfParam(SelfParamKind::Explicit) | ParamKind::Normal => {
                        cx.unknown()
                    }
                },
            };
            params.push(ty);
        }
        let ret = data
            .signature
            .ret_ty()
            .map(|ty| session.lower_type_ref(ty))
            .transpose()?
            .unwrap_or(cx.unit());
        let clauses = session.lower_clauses()?;

        Ok(Some(CallableSignature {
            params: List::new(cx, &params),
            ret,
            clauses: List::new(cx, &clauses),
            qualifiers: data.signature.qualifiers(),
        }))
    }

    pub fn field_ty<'s>(
        &self,
        cx: SolverInterner<'s>,
        field: FieldRef,
    ) -> Result<Option<Ty<'s>>, D::Error> {
        let Some(data) = self.item_paths.items().field_data(field)? else {
            return Ok(None);
        };
        let owner = GenericDefRef::TypeDef(field.owner);

        self.session(
            cx,
            TypeLoweringEnv::new(
                owner,
                TypeLoweringAnchor::Context(TypePathContext::module(data.owner_module)),
            ),
        )?
        .lower_type_ref(&data.field.ty)
        .map(Some)
    }

    pub fn enum_variant_field_ty<'s>(
        &self,
        cx: SolverInterner<'s>,
        variant: EnumVariantRef,
        field_index: usize,
    ) -> Result<Option<Ty<'s>>, D::Error> {
        let Some(data) = self.item_paths.items().enum_variant_data(variant)? else {
            return Ok(None);
        };
        let Some(field) = data.variant.fields.fields().get(field_index) else {
            return Ok(None);
        };
        let owner = GenericDefRef::TypeDef(data.owner);

        self.session(
            cx,
            TypeLoweringEnv::new(
                owner,
                TypeLoweringAnchor::Context(TypePathContext::module(data.owner_module)),
            ),
        )?
        .lower_type_ref(&field.ty)
        .map(Some)
    }

    pub fn const_ty<'s>(
        &self,
        cx: SolverInterner<'s>,
        konst: ConstRef,
    ) -> Result<Option<Ty<'s>>, D::Error> {
        let Some(data) = self.item_paths.items().const_data(konst)? else {
            return Ok(None);
        };
        let Some(ty) = data.signature.ty() else {
            return Ok(Some(cx.unknown()));
        };
        let Some(context) = self
            .item_paths
            .items()
            .type_path_context_for_owner(konst.origin, data.owner)?
        else {
            return Ok(None);
        };

        self.session(
            cx,
            TypeLoweringEnv::new(
                GenericDefRef::Const(konst),
                TypeLoweringAnchor::Context(context),
            ),
        )?
        .lower_type_ref(ty)
        .map(Some)
    }

    pub fn static_ty<'s>(
        &self,
        cx: SolverInterner<'s>,
        static_ref: StaticRef,
    ) -> Result<Option<Ty<'s>>, D::Error> {
        let Some(data) = self.item_paths.items().static_data(static_ref)? else {
            return Ok(None);
        };
        let Some(ty) = &data.ty else {
            return Ok(Some(cx.unknown()));
        };

        self.session(
            cx,
            TypeLoweringEnv::new(
                GenericDefRef::Static(static_ref),
                TypeLoweringAnchor::Context(TypePathContext::module(data.owner)),
            ),
        )?
        .lower_type_ref(ty)
        .map(Some)
    }

    pub(crate) fn trait_header<'s>(
        &self,
        cx: SolverInterner<'s>,
        trait_ref: TraitDefRef,
    ) -> Result<Option<TraitHeader<'s>>, D::Error> {
        let Some(data) = self.item_paths.items().trait_data(trait_ref)? else {
            return Ok(None);
        };
        let owner = GenericDefRef::Trait(trait_ref);
        let generics = self.item_paths.generics().generics(owner)?;
        let Some(self_param) = generics.iter().find_map(|param| {
            matches!(param.source(), GenericParamSource::TraitSelf).then_some(param.param())
        }) else {
            return Ok(None);
        };
        let GenericParamRef::Type(self_param) = self_param else {
            return Ok(None);
        };
        let self_ty = cx
            .param_arg(GenericParamRef::Type(self_param), 0)
            .as_ty()
            .expect("trait Self is a type");

        let mut session = self.session(
            cx,
            TypeLoweringEnv::new(
                owner,
                TypeLoweringAnchor::Context(TypePathContext::module(data.owner)),
            ),
        )?;
        let mut super_traits = Vec::new();
        for bound in &data.super_traits {
            let Some(trait_ty) = bound.required_trait_ty() else {
                continue;
            };
            if let Some(super_trait) = session.lower_trait_ref(trait_ty, self_ty)? {
                super_traits.push(super_trait);
            }
        }
        let mut clauses = session.lower_clauses()?;
        for super_trait in &super_traits {
            clauses.extend(super_trait.clauses(cx));
        }

        Ok(Some(TraitHeader {
            owner: trait_ref,
            self_ty,
            clauses,
        }))
    }

    pub fn type_alias_ty<'s>(
        &self,
        cx: SolverInterner<'s>,
        alias: TypeAliasRef,
    ) -> Result<Option<Ty<'s>>, D::Error> {
        let Some(data) = self.item_paths.items().type_alias_data(alias)? else {
            return Ok(None);
        };
        let Some(context) = self
            .item_paths
            .items()
            .type_path_context_for_owner(alias.origin, data.owner)?
        else {
            return Ok(None);
        };

        let mut session = self.session(
            cx,
            TypeLoweringEnv::new(
                GenericDefRef::TypeAlias(alias),
                TypeLoweringAnchor::Context(context),
            ),
        )?;
        session.lower_alias(alias, &[]).map(Some)
    }

    pub(crate) fn opaque_bounds_for_owner<'s>(
        &self,
        cx: SolverInterner<'s>,
        owner: GenericDefRef,
    ) -> Result<Vec<(OpaqueTy<'s>, Vec<TraitRefLowering<'s>>)>, D::Error> {
        let Some(context) = self
            .item_paths
            .items()
            .type_path_context_for_generic_def(owner)?
        else {
            return Ok(Vec::new());
        };

        let mut session = self.session(
            cx,
            TypeLoweringEnv::new(owner, TypeLoweringAnchor::Context(context)),
        )?;

        match owner {
            GenericDefRef::Function(function) => {
                let Some(data) = self.item_paths.items().function_data(function)? else {
                    return Ok(Vec::new());
                };
                for param in data.signature.params() {
                    if let Some(ty) = &param.ty {
                        session.lower_parameter_type(ty)?;
                    }
                }
                if let Some(ret) = data.signature.ret_ty() {
                    session.lower_type_ref(ret)?;
                }
            }
            GenericDefRef::TypeAlias(alias) => {
                session.lower_alias(alias, &[])?;
            }
            GenericDefRef::Const(konst) => {
                if let Some(ty) = self
                    .item_paths
                    .items()
                    .const_data(konst)?
                    .and_then(|data| data.signature.ty())
                {
                    session.lower_type_ref(ty)?;
                }
            }
            GenericDefRef::Static(static_ref) => {
                if let Some(ty) = self
                    .item_paths
                    .items()
                    .static_data(static_ref)?
                    .and_then(|data| data.ty.as_ref())
                {
                    session.lower_type_ref(ty)?;
                }
            }
            GenericDefRef::TypeDef(_) | GenericDefRef::Trait(_) | GenericDefRef::Impl(_) => {}
        }

        Ok(session.into_opaque_bounds())
    }

    pub fn impl_header<'s>(
        &self,
        cx: SolverInterner<'s>,
        impl_ref: ImplRef,
    ) -> Result<Option<ImplHeader<'s>>, D::Error> {
        let Some(data) = self.item_paths.items().impl_data(impl_ref)? else {
            return Ok(None);
        };
        let owner = GenericDefRef::Impl(impl_ref);
        let context = TypePathContext {
            module: data.owner,
            self_owner: Some(SelfTypeOwner::Impl(impl_ref)),
        };

        let mut session = self.session(
            cx,
            TypeLoweringEnv::new(owner, TypeLoweringAnchor::Context(context)),
        )?;
        let self_ty = session.lower_type_ref(&data.self_ty)?;
        let trait_ref = data
            .trait_ref
            .as_ref()
            .map(|trait_ty| session.lower_trait_ref(trait_ty, self_ty))
            .transpose()?
            .flatten();
        let clauses = session.lower_clauses()?;

        Ok(Some(ImplHeader {
            owner: impl_ref,
            self_ty,
            trait_ref,
            clauses,
        }))
    }
}
