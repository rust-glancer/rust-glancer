//! Find written names introduced inside a body for cursor and reference queries.

use rg_body_ir::BodyView;
use rg_def_map::ItemSourceKind;
use rg_ir_model::{
    BindingId, BodyRef, DefMapRef, EnumVariantRef, FieldRef, FileId, SemanticItemRef, SourceId,
    TypeDefId, identity::DeclarationRef,
};
use rg_item_tree::SourceLocations as _;
use rg_semantic_ir::SemanticItemSource as _;
use rg_source::ReadError;

use super::{BindingSurface, BodySourceCandidate, record_pat_shorthand::RecordPatShorthandBinding};
use crate::IndexedViewDb;

/// Finds bindings and local item declarations using each name's own source location.
/// The cursor supplies an offset. A reference scan asks for every name in scope.
pub(super) struct BodyDeclarationScanner<'view, 'db, 'body> {
    db: &'view IndexedViewDb<'db>,
    body_ref: BodyRef,
    body: BodyView<'body>,
    file_id: Option<FileId>,
    offset: Option<u32>,
}

impl<'view, 'db, 'body> BodyDeclarationScanner<'view, 'db, 'body> {
    pub(super) fn new(
        db: &'view IndexedViewDb<'db>,
        body_ref: BodyRef,
        body: BodyView<'body>,
        file_id: Option<FileId>,
        offset: Option<u32>,
    ) -> Self {
        Self {
            db,
            body_ref,
            body,
            file_id,
            offset,
        }
    }

    pub(super) fn scan(&self, mut visit: impl FnMut(BodySourceCandidate)) -> Result<(), ReadError> {
        let body = self.body;
        // In `let User { name } = user`, the field key and the binding share a token.
        // Keep the pattern ranges so a rename can give the binding a separate spelling.
        let record_bindings = RecordPatShorthandBinding::collect(body, self.file_id, self.offset);
        for (idx, binding) in body.bindings().iter().enumerate() {
            rg_std::check_cancel!(self.db, "body binding occurrences");
            let name = body.source_at(binding.name_location.unwrap_or(binding.source));
            let location = name.location();
            if !location.is_exact()
                || self.file_id.is_some_and(|file| file != name.file_id)
                || self
                    .offset
                    .is_some_and(|offset| !location.span.touches(offset))
            {
                continue;
            }
            let binding = BindingId(idx);
            let surface = record_bindings
                .iter()
                .find(|shorthand| shorthand.binding == binding)
                .map_or(BindingSurface::Plain, |shorthand| {
                    BindingSurface::RecordPatShorthand {
                        key: shorthand.key.clone(),
                        field_span: shorthand.field_span,
                        pat_span: shorthand.pat_span,
                        binding_name_span: shorthand.binding_name_span,
                    }
                });
            visit(BodySourceCandidate::Binding {
                body: self.body_ref,
                binding,
                file_id: name.file_id,
                location,
                surface,
            });
        }

        for item in self.db.all_items(DefMapRef::Body(self.body_ref))? {
            rg_std::check_cancel!(self.db, "body item occurrences");
            let source = item.source();
            if self.file_id.is_some_and(|file| file != source.file_id) {
                continue;
            }
            let locations = self.db.declaration_source(item.item().origin(), source)?;
            let mut emit = |declaration, name: Option<SourceId>| {
                if let Some(location) = locations.exact_location(name)
                    && self
                        .offset
                        .is_none_or(|offset| location.span.touches(offset))
                {
                    visit(BodySourceCandidate::LocalDeclaration {
                        declaration,
                        location,
                    });
                }
            };
            let name = match source.kind {
                ItemSourceKind::Body(source) if source.body == self.body_ref => body
                    .source_item(source.item)
                    .and_then(|node| node.name_location),
                _ => item.name_location(),
            };
            if !matches!(item.item(), SemanticItemRef::Impl(_)) {
                emit(DeclarationRef::from(item.item()), name);
            }

            // In `($field:ident) => { struct Local { $field: u8 } }`, `Local` comes from
            // the template. Check the field's name too, since it can come from the invocation.
            let Some(ty) = item.type_def() else {
                continue;
            };
            let fields = item
                .struct_data()
                .map(|data| data.fields.fields())
                .or_else(|| item.union_data().map(|data| data.fields.as_slice()));
            for (index, field) in fields.into_iter().flatten().enumerate() {
                rg_std::check_cancel!(self.db, "body field occurrences");
                emit(
                    DeclarationRef::from(FieldRef { owner: ty, index }),
                    Some(field.location),
                );
            }
            if let TypeDefId::Enum(enum_id) = ty.id
                && let Some(data) = item.enum_data()
            {
                for (index, variant) in data.variants.iter().enumerate() {
                    rg_std::check_cancel!(self.db, "body variant occurrences");
                    emit(
                        DeclarationRef::from(EnumVariantRef {
                            origin: ty.origin,
                            enum_id,
                            index,
                        }),
                        Some(variant.name_location),
                    );
                }
            }
        }
        Ok(())
    }
}
