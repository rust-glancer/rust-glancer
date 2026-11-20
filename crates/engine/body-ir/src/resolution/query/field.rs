//! Field types for owned receivers and pattern bindings.

use rg_def_map::DefMapSource;
use rg_ir_model::{EnumVariantRef, FieldKey, FieldRef, PatId, ScopeId, TypeDefId, TypeDefRef};
use rg_item_tree::FieldList;
use rg_semantic_ir::SemanticItemSource;
use rg_source::ReadError;
use rg_ty::{AdtTy, Ty, solver::SemanticDeclarations};

use crate::{BodyPath, PatKind, resolution::BodyResolutionContext};

/// Reads field declarations and substitutes the owner type's arguments into them.
/// Enum patterns also supply a variant identity so the same field position is read from the right
/// variant. Results use owned types and can be used before live body inference starts.
pub struct BodyFieldQuery<'query, D, I> {
    context: BodyResolutionContext<'query, D, I>,
}

impl<'query, D, I> BodyFieldQuery<'query, D, I>
where
    D: DefMapSource + Copy,
    I: SemanticItemSource<'query> + Copy,
{
    pub(crate) fn new(context: BodyResolutionContext<'query, D, I>) -> Self {
        Self { context }
    }

    /// Collect fields along the same receiver chain used by live field inference. The body's
    /// bounds and lexical declarations stay available while generic Deref targets are resolved.
    pub fn field_candidates_for_ty(&self, ty: &Ty) -> Result<Vec<FieldRef>, ReadError> {
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

    /// In `(left, .., right)`, count `left` from the start and `right` from the end.
    /// Only a suffix needs the arity, so leading fields still work with incomplete type data.
    pub(crate) fn tuple_pattern_fields<'pat>(
        &self,
        fields: &'pat [PatId],
        arity: impl FnOnce() -> Result<Option<usize>, ReadError>,
    ) -> Result<impl Iterator<Item = (usize, PatId)> + 'pat, ReadError> {
        let body = self.context.body();
        let rest = fields
            .iter()
            .position(|pat| {
                body.pat(*pat)
                    .is_some_and(|pat| matches!(pat.kind, PatKind::Rest))
            })
            .unwrap_or(fields.len());
        let (prefix, rest_and_suffix) = fields.split_at(rest);
        let suffix = rest_and_suffix.get(1..).unwrap_or_default();
        let suffix_start = if suffix.is_empty() {
            None
        } else {
            arity()?
                .and_then(|arity| arity.checked_sub(suffix.len()))
                .filter(|start| *start >= prefix.len())
        };
        let suffix = suffix_start
            .into_iter()
            .flat_map(move |start| (start..).zip(suffix.iter().copied()));
        Ok(prefix.iter().copied().enumerate().chain(suffix))
    }

    pub(crate) fn tuple_pattern_arity(
        &self,
        owner: TypeDefRef,
        variant: Option<EnumVariantRef>,
    ) -> Result<Option<usize>, ReadError> {
        let items = self.context.item_query();
        let fields = match owner.id {
            TypeDefId::Struct(_) => items
                .semantic_item_view(owner.into())?
                .and_then(|item| item.struct_data())
                .map(|data| &data.fields),
            TypeDefId::Enum(_) => {
                let Some(variant) = variant else {
                    return Ok(None);
                };
                items
                    .enum_variant_data(variant)?
                    .map(|data| &data.variant.fields)
            }
            TypeDefId::Union(_) => None,
        };
        Ok(match fields {
            Some(FieldList::Tuple(fields)) => Some(fields.len()),
            _ => None,
        })
    }

    pub(crate) fn pattern_variant(
        &self,
        scope: ScopeId,
        path: Option<&BodyPath>,
        owner: TypeDefRef,
    ) -> Result<Option<EnumVariantRef>, ReadError> {
        let (TypeDefId::Enum(enum_id), Some(path)) =
            (owner.id, path.and_then(BodyPath::as_def_map_path))
        else {
            return Ok(None);
        };
        // The written name can alias a variant with a different name, even one whose name
        // collides with another variant of this enum. Prefer the declaration visible in scope.
        if let Some(variant) = self
            .context
            .type_path_query()
            .resolve_enum_variant_in_scope(scope, &path)?
        {
            return Ok(
                (variant.origin == owner.origin && variant.enum_id == enum_id).then_some(variant),
            );
        }
        // DefMap path lookup does not follow type aliases or `Self` to their underlying enum.
        // Keep using the expected enum as a fallback for these patterns.
        let Some(name) = path.segments().last() else {
            return Ok(None);
        };
        self.context
            .item_query()
            .enum_variant_ref_for_type_def(owner, name.as_str())
    }

    /// Project the field type destructured by a record or tuple-variant pattern.
    ///
    /// Read from the expected type after the caller peels any written references.
    pub(crate) fn pattern_field_ty(
        &self,
        owner: &AdtTy,
        variant: Option<EnumVariantRef>,
        field_key: &FieldKey,
    ) -> Result<Option<Ty>, ReadError> {
        match (owner.def.id, variant) {
            (TypeDefId::Enum(_), Some(variant)) => {
                self.enum_variant_field_ty(owner, variant, field_key)
            }
            (TypeDefId::Enum(_), None) => Ok(None),
            (TypeDefId::Struct(_) | TypeDefId::Union(_), _) => self.declared(owner, field_key),
        }
    }

    /// Resolve a declared field directly from its owner type.
    pub(crate) fn declared(
        &self,
        owner_ty: &AdtTy,
        field: &FieldKey,
    ) -> Result<Option<Ty>, ReadError> {
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
    ) -> Result<Option<Ty>, ReadError> {
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
