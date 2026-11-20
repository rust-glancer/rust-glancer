//! Body queries over live solver types. Declaration lookup stays in the existing semantic query
//! layer; substitutions and projections keep the caller's inference variables throughout.

mod call;
mod index;
mod member;

use rg_def_map::DefMapSource;
use rg_ir_model::{EnumVariantRef, FieldKey, FieldRef, ScopeId, TypeDefId};
use rg_item_tree::{GenericArg as ItemGenericArg, TypeRef};
use rg_semantic_ir::{Generics, SemanticItemSource};
use rg_source::ReadError;
use rg_ty::{
    lowering::{TypeLoweringAnchor, TypeLoweringEnv, TypeLoweringQuery},
    solver::{
        AdtTy, DefId, GenericArgs, InferenceSubstitution, InferenceTable, ProjectionTy,
        TraitApplication, Ty, TyShape,
    },
};

pub(crate) use self::{call::LiveCallTarget, member::FunctionLookup};
use crate::{body::facts::BodyResolution, resolution::BodyResolutionContext};

/// Looks up declarations without turning the body's live types into saved types along the way.
/// For a field on `Wrapper<?T>`, its declared `T` must become that same `?T`, so later assignments
/// are visible both through the receiver and through the field result.
pub(crate) struct LiveBodyQuery<'query, D, I> {
    context: BodyResolutionContext<'query, D, I>,
}

impl<'query, D, I> LiveBodyQuery<'query, D, I>
where
    D: DefMapSource + Copy,
    I: SemanticItemSource<'query> + Copy,
{
    pub(crate) fn new(context: BodyResolutionContext<'query, D, I>) -> Self {
        Self { context }
    }

    pub(crate) fn type_ref<'s>(
        &self,
        scope: ScopeId,
        ty: &TypeRef,
        table: &InferenceTable<'s>,
    ) -> Result<Ty<'s>, ReadError> {
        let paths = self.context.item_paths();
        let query = TypeLoweringQuery::new(&paths, &self.context);
        let owner = self.context.body().owner().generic_def();
        query.lower_inference_type(
            ty,
            TypeLoweringEnv::new(owner, TypeLoweringAnchor::Scope(scope)),
            table,
        )
    }

    pub(crate) fn generic_args<'s>(
        &self,
        scope: ScopeId,
        generics: &Generics<'_>,
        args: &[ItemGenericArg],
        table: &InferenceTable<'s>,
    ) -> Result<GenericArgs<'s>, ReadError> {
        let paths = self.context.item_paths();
        let query = TypeLoweringQuery::new(&paths, &self.context);
        let owner = self.context.body().owner().generic_def();
        query.lower_inference_args(
            generics,
            args,
            TypeLoweringEnv::new(owner, TypeLoweringAnchor::Scope(scope)),
            table,
        )
    }

    /// Find the associated declaration and give its value a live destination in the table.
    /// For `<Iter<?T> as Iterator>::Item`, `Some` means the question could be registered, not
    /// that its answer is already known. The returned type can settle as later goals are solved.
    pub(crate) fn projection<'s>(
        &self,
        application: TraitApplication<'s>,
        name: &str,
        table: &InferenceTable<'s>,
    ) -> Result<Option<Ty<'s>>, ReadError> {
        let Some(associated_ty) = self
            .context
            .item_query()
            .declared_associated_type_by_name(application.def, name)?
        else {
            return Ok(None);
        };
        let cx = table.interner();
        // Keep the requirement and its output connected to the same arguments. Registering the
        // question is useful before its answer is known; the body's checkpoints fulfill it later.
        table.commit_if_some(|table| {
            table.register(application.clause(cx));
            Ok(Some(table.normalize(cx.projection(ProjectionTy {
                associated_ty,
                args: application.args,
            }))))
        })
    }

    pub(crate) fn field<'s>(
        &self,
        ty: Ty<'s>,
        field: &FieldKey,
        table: &InferenceTable<'s>,
    ) -> Result<Option<(BodyResolution, Ty<'s>)>, ReadError> {
        for receiver in table.autoderef(ty) {
            if let (TyShape::Tuple(fields), FieldKey::Tuple(index)) = (receiver.shape(), field)
                && let Some(ty) = fields.get(*index)
            {
                return Ok(Some((BodyResolution::Unknown, *ty)));
            }
            let Some(adt) = receiver.as_adt() else {
                continue;
            };
            let Some((field_ref, ty)) = self.declared_field(adt, field, table)? else {
                continue;
            };
            return Ok(Some((
                BodyResolution::Declarations([field_ref.into()].into_iter().collect()),
                ty,
            )));
        }
        Ok(None)
    }

    /// Project a field of the given owner directly, as required for constructor arguments.
    pub(crate) fn declared_field<'s>(
        &self,
        adt: AdtTy<'s>,
        field: &FieldKey,
        table: &InferenceTable<'s>,
    ) -> Result<Option<(FieldRef, Ty<'s>)>, ReadError> {
        let Some(field_ref) = self.context.item_query().field_for_type(adt.def, field)? else {
            return Ok(None);
        };
        let paths = self.context.item_paths();
        let lowering = TypeLoweringQuery::new(&paths, &self.context);
        let ty = lowering
            .field_ty(table.interner(), field_ref)?
            .map(|ty| self.instantiate_field(adt, ty, table))
            .unwrap_or(table.interner().unknown());
        Ok(Some((field_ref, ty)))
    }

    fn instantiate_field<'s>(
        &self,
        adt: AdtTy<'s>,
        ty: Ty<'s>,
        table: &InferenceTable<'s>,
    ) -> Ty<'s> {
        let owner = DefId::Adt(adt.def);
        InferenceSubstitution::from_args(table.params(owner).iter().copied(), adt.args)
            .apply(table.interner(), ty)
    }

    pub(crate) fn enum_variant_field<'s>(
        &self,
        adt: AdtTy<'s>,
        variant: EnumVariantRef,
        field: &FieldKey,
        table: &InferenceTable<'s>,
    ) -> Result<Option<Ty<'s>>, ReadError> {
        if adt.def.id != TypeDefId::Enum(variant.enum_id) || adt.def.origin != variant.origin {
            return Ok(None);
        }
        let Some(data) = self.context.item_query().enum_variant_data(variant)? else {
            return Ok(None);
        };
        let Some(index) = data
            .variant
            .fields
            .fields()
            .iter()
            .position(|f| f.key.as_ref() == Some(field))
        else {
            return Ok(None);
        };
        let paths = self.context.item_paths();
        let lowering = TypeLoweringQuery::new(&paths, &self.context);
        Ok(lowering
            .enum_variant_field_ty(table.interner(), variant, index)?
            .map(|ty| self.instantiate_field(adt, ty, table)))
    }

    pub(crate) fn pattern_field<'s>(
        &self,
        owner: AdtTy<'s>,
        variant: Option<EnumVariantRef>,
        field: &FieldKey,
        table: &InferenceTable<'s>,
    ) -> Result<Option<Ty<'s>>, ReadError> {
        match (owner.def.id, variant) {
            (TypeDefId::Enum(_), Some(variant)) => {
                self.enum_variant_field(owner, variant, field, table)
            }
            (TypeDefId::Enum(_), None) => Ok(None),
            (TypeDefId::Struct(_) | TypeDefId::Union(_), _) => Ok(self
                .field(table.interner().adt(owner), field, table)?
                .map(|(_, ty)| ty)),
        }
    }
}
