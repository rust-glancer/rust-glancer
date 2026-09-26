//! Field types for owned receivers and pattern bindings.

use rg_def_map::DefMapSource;
use rg_ir_model::{EnumVariantRef, FieldKey, FieldRef, TypeDefId};
use rg_item_tree::FieldList;
use rg_package_store::PackageStoreError;
use rg_semantic_ir::ItemStoreSource;
use rg_std::ExpectedUnique;
use rg_ty::{AdtTy, Ty, solver::SemanticDeclarations};

use crate::{BodyPath, resolution::BodyResolutionContext};

/// Reads field declarations and substitutes the owner type's arguments into them.
/// Enum patterns also supply a variant name so the same field position is read from the right
/// variant. Results use owned types and can be used before live body inference starts.
pub struct BodyFieldQuery<'query, D, I> {
    context: BodyResolutionContext<'query, D, I>,
}

impl<'query, D, I> BodyFieldQuery<'query, D, I>
where
    D: DefMapSource<Error = PackageStoreError> + Copy,
    I: ItemStoreSource<'query, Error = PackageStoreError> + Copy,
{
    pub(crate) fn new(context: BodyResolutionContext<'query, D, I>) -> Self {
        Self { context }
    }

    /// Collect fields along the same receiver chain used by live field inference. The body's
    /// bounds and lexical declarations stay available while generic Deref targets are resolved.
    pub fn field_candidates_for_ty(&self, ty: &Ty) -> Result<Vec<FieldRef>, PackageStoreError> {
        let context = self.context.ty_context();
        let declarations = SemanticDeclarations::new(&context, &self.context);
        declarations.with_table(|table, params| {
            let receiver = table.interner().lower_ty(ty, params);
            let mut fields = Vec::new();
            for receiver in table.autoderef(receiver) {
                if let Some(adt) = receiver.as_adt() {
                    fields.extend(self.context.item_query().fields_for_type(adt.def)?);
                }
            }
            Ok(fields)
        })?
    }

    /// Project the field type destructured by a record or tuple-variant pattern.
    ///
    /// Pattern matching peels written references but does not use receiver autoderef. Struct and
    /// union fields come directly from the expected nominal type; enum fields additionally use the
    /// pattern path's final segment to select the variant.
    pub(crate) fn pattern_field_ty(
        &self,
        path: Option<&BodyPath>,
        expected_ty: &Ty,
        field_key: &FieldKey,
    ) -> Result<Option<Ty>, PackageStoreError> {
        let variant_path = path.and_then(BodyPath::as_def_map_path);
        let variant_name = variant_path
            .as_ref()
            .and_then(|path| path.segments().last())
            .map(rg_text::Name::as_str);
        let mut candidates = ExpectedUnique::new();

        for candidate in expected_ty.reference_chain() {
            for nominal_ty in candidate.as_adts() {
                let field_ty = match nominal_ty.def.id {
                    TypeDefId::Struct(_) | TypeDefId::Union(_) => {
                        self.declared(nominal_ty, field_key)?
                    }
                    TypeDefId::Enum(_) => {
                        let Some(variant_name) = variant_name else {
                            continue;
                        };
                        let Some(variant_ref) = self
                            .context
                            .item_query()
                            .enum_variant_ref_for_type_def(nominal_ty.def, variant_name)?
                        else {
                            continue;
                        };
                        self.enum_variant_field_ty(nominal_ty, variant_ref, field_key)?
                    }
                };
                if let Some(field_ty) = field_ty {
                    candidates.push(field_ty);
                }
            }
        }

        Ok(candidates.into_option())
    }

    /// Resolve a declared field directly from its owner type.
    pub(crate) fn declared(
        &self,
        owner_ty: &AdtTy,
        field: &FieldKey,
    ) -> Result<Option<Ty>, PackageStoreError> {
        let item_query = self.context.item_query();
        let Some(field_ref) = item_query.field_for_type(owner_ty.def, field)? else {
            return Ok(None);
        };
        // A declaration remains useful for navigation even if its type data is unavailable.
        // Read the type once before asking for substitutions, so missing data stays fail-soft.
        let ty = self
            .context
            .signatures()
            .field_ty(field_ref)?
            .map(|ty| {
                self.context
                    .generics()
                    .subst_for_nominal_ty(owner_ty)
                    .map(|subst| subst.apply(&ty))
            })
            .transpose()?;

        Ok(ty)
    }

    /// Return the type of an enum variant field for a known enum type.
    pub(crate) fn enum_variant_field_ty(
        &self,
        enum_ty: &AdtTy,
        variant_ref: EnumVariantRef,
        field_key: &FieldKey,
    ) -> Result<Option<Ty>, PackageStoreError> {
        let TypeDefId::Enum(enum_id) = enum_ty.def.id else {
            return Ok(None);
        };
        if variant_ref.origin != enum_ty.def.origin || variant_ref.enum_id != enum_id {
            return Ok(None);
        }

        let item_query = self.context.item_query();
        let Some(variant_data) = item_query.enum_variant_data(variant_ref)? else {
            return Ok(None);
        };
        let Some(field_index) = Self::variant_field(&variant_data.variant.fields, field_key) else {
            return Ok(None);
        };
        let subst = self.context.generics().subst_for_nominal_ty(enum_ty)?;
        Ok(self
            .context
            .signatures()
            .enum_variant_field_ty(variant_ref, field_index)?
            .map(|ty| subst.apply(&ty)))
    }

    /// Find a named or tuple field inside a variant declaration.
    fn variant_field(fields: &FieldList, key: &FieldKey) -> Option<usize> {
        match key {
            FieldKey::Named(_) => fields
                .fields()
                .iter()
                .position(|field| field.key.as_ref() == Some(key)),
            FieldKey::Tuple(index) => fields
                .fields()
                .get(*index)
                .filter(|field| field.key.as_ref() == Some(key))
                .map(|_| *index),
        }
    }
}
