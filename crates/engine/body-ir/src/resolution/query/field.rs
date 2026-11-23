//! Field types for owned receivers and pattern bindings.

use std::collections::HashSet;

use rg_def_map::{DefMapSource, NamespaceSet};
use rg_ir_model::{EnumVariantRef, FieldKey, FieldRef, PatId, ScopeId, TypeDefId, TypeDefRef};
use rg_item_tree::FieldList;
use rg_semantic_ir::SemanticItemSource;
use rg_source::ReadError;
use rg_ty::{Ty, solver::SemanticDeclarations};

use crate::{BodyPath, PatKind, body::facts::BodyResolution, resolution::BodyResolutionContext};

/// Reads field declarations and substitutes the owner type's arguments into them.
/// Enum patterns also supply a variant identity so the same field position is read from the right
/// variant. Editor queries use a temporary solver to project fields from saved receiver types.
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
    /// With an expected type, compare the field as accessed on the receiver. For a
    /// `Box<Slot<u8>>`, a field declared as `T` on `Slot<T>` is compared as `u8`.
    pub fn field_candidates_for_ty(
        &self,
        ty: &Ty,
        expected: Option<&Ty>,
    ) -> Result<Vec<(FieldRef, bool)>, ReadError> {
        let context = self.context.ty_context();
        let declarations = SemanticDeclarations::new(&context, &self.context);
        declarations.with_table(|table, params| {
            let receiver = table.interner().lower_ty(ty, params);
            let items = self.context.item_query();
            let live = self.context.live();
            let mut seen_fields = HashSet::new();
            let mut fields = Vec::new();
            for receiver in table.autoderef(receiver) {
                let Some(adt) = receiver.as_adt() else {
                    continue;
                };
                for field in items.fields_for_type(adt.def)? {
                    // Access stops at the first field with this name, even if a later
                    // dereference exposes the same declaration with different arguments.
                    let matches = if let Some(expected) = expected
                        && let Some(data) = items.field_data(field)?
                        && data
                            .field
                            .key
                            .as_ref()
                            .is_some_and(|key| seen_fields.insert(key))
                    {
                        live.field_ty_on_receiver(adt, field, &table)?
                            .is_some_and(|ty| table.resolves_to(ty, expected))
                    } else {
                        false
                    };
                    fields.push((field, matches));
                }
            }
            Ok(fields)
        })?
    }

    /// Project the selected field from its receiver before applying any use-site coercion.
    pub fn field_access_ty(
        &self,
        receiver: &Ty,
        key: &FieldKey,
        field: FieldRef,
    ) -> Result<Option<Ty>, ReadError> {
        let context = self.context.ty_context();
        let declarations = SemanticDeclarations::new(&context, &self.context);
        declarations.with_table(|table, params| {
            let receiver = table.interner().lower_ty(receiver, params);
            let Some((BodyResolution::Declarations(declarations), ty)) =
                self.context.live().field(receiver, key, &table)?
            else {
                return Ok(None);
            };
            if declarations.as_one() != Some(&field.into()) {
                return Ok(None);
            }
            let ty = table.normalize(ty);
            let _ = table.fulfill();
            Ok(Some(table.finalize(ty)))
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
        namespace: NamespaceSet,
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
            .resolve_enum_variant_in_scope(scope, &path, namespace)?
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
}
