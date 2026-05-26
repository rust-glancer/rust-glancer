//! Composite enum-variant view over semantic and body-local enum declarations.

use rg_body_ir::{
    BodyEnumVariantData, BodyEnumVariantRef, BodyRef, BodyTypePathResolution,
    ResolvedEnumVariantRef, ScopeId,
};
use rg_def_map::Path;
use rg_semantic_ir::{Documentation, EnumVariantData, EnumVariantRef, TypeDefId, TypeDefRef};

use crate::api::Analysis;

/// Borrowed data for one resolved enum variant, independent from the storage layer it came from.
#[derive(Debug, Clone, Copy)]
pub(crate) enum EnumVariant<'a> {
    Semantic {
        variant: EnumVariantRef,
        data: EnumVariantData<'a>,
    },
    BodyLocal {
        variant: BodyEnumVariantRef,
        data: BodyEnumVariantData<'a>,
    },
}

impl<'a> EnumVariant<'a> {
    pub(crate) fn variant_ref(&self) -> ResolvedEnumVariantRef {
        match self {
            Self::Semantic { variant, .. } => ResolvedEnumVariantRef::Semantic(*variant),
            Self::BodyLocal { variant, .. } => ResolvedEnumVariantRef::BodyLocal(*variant),
        }
    }

    pub(crate) fn name(&self) -> &'a str {
        match self {
            Self::Semantic { data, .. } => data.variant.name.as_str(),
            Self::BodyLocal { data, .. } => data.variant.name.as_str(),
        }
    }

    pub(crate) fn docs_text(&self) -> Option<String> {
        self.docs().map(Documentation::text)
    }

    fn docs(&self) -> Option<&'a Documentation> {
        match self {
            Self::Semantic { data, .. } => data.variant.docs.as_ref(),
            Self::BodyLocal { data, .. } => data.variant.docs.as_ref(),
        }
    }
}

pub(crate) struct EnumVariantView<'a, 'db> {
    analysis: &'a Analysis<'db>,
}

impl<'a, 'db> EnumVariantView<'a, 'db> {
    pub(crate) fn new(analysis: &'a Analysis<'db>) -> Self {
        Self { analysis }
    }

    pub(crate) fn variants_for_body_type_path(
        &self,
        body: BodyRef,
        scope: ScopeId,
        path: &Path,
    ) -> anyhow::Result<Vec<EnumVariant<'_>>> {
        let resolution = self.analysis.body_ir.resolve_type_path_in_scope(
            &self.analysis.def_map,
            &self.analysis.semantic_ir,
            body,
            scope,
            path,
        )?;
        let mut variants = Vec::new();

        match resolution {
            BodyTypePathResolution::BodyLocal(item) => {
                variants.extend(self.body_local_variants(item)?);
            }
            BodyTypePathResolution::TypeDefs(types) | BodyTypePathResolution::SelfType(types) => {
                for ty in types {
                    variants.extend(self.semantic_variants(ty)?);
                }
            }
            BodyTypePathResolution::Primitive(_)
            | BodyTypePathResolution::Traits(_)
            | BodyTypePathResolution::Unknown => {}
        }

        Ok(variants)
    }

    fn semantic_variants(&self, ty: TypeDefRef) -> anyhow::Result<Vec<EnumVariant<'_>>> {
        let TypeDefId::Enum(enum_id) = ty.id else {
            return Ok(Vec::new());
        };
        let Some(data) = self.analysis.semantic_ir.enum_data_for_type_def(ty)? else {
            return Ok(Vec::new());
        };
        let variant_refs = (0..data.variants.len()).map(|index| EnumVariantRef {
            target: ty.target,
            enum_id,
            index,
        });

        let mut variants = Vec::new();
        for variant_ref in variant_refs {
            let Some(variant) = self.variant(ResolvedEnumVariantRef::Semantic(variant_ref))? else {
                continue;
            };
            variants.push(variant);
        }

        Ok(variants)
    }

    fn body_local_variants(
        &self,
        item_ref: rg_body_ir::BodyItemRef,
    ) -> anyhow::Result<Vec<EnumVariant<'_>>> {
        let Some(body) = self.analysis.body_ir.body_data(item_ref.body)? else {
            return Ok(Vec::new());
        };
        let Some(item) = body.local_item(item_ref.item) else {
            return Ok(Vec::new());
        };
        let variant_refs = (0..item.enum_variants().len()).map(|index| {
            ResolvedEnumVariantRef::BodyLocal(BodyEnumVariantRef {
                item: item_ref,
                index,
            })
        });

        let mut variants = Vec::new();
        for variant_ref in variant_refs {
            let Some(variant) = self.variant(variant_ref)? else {
                continue;
            };
            variants.push(variant);
        }

        Ok(variants)
    }

    pub(crate) fn variant(
        &self,
        variant: ResolvedEnumVariantRef,
    ) -> anyhow::Result<Option<EnumVariant<'_>>> {
        match variant {
            ResolvedEnumVariantRef::Semantic(variant) => Ok(self
                .analysis
                .semantic_ir
                .enum_variant_data(variant)?
                .map(|data| EnumVariant::Semantic { variant, data })),
            ResolvedEnumVariantRef::BodyLocal(variant) => Ok(self
                .analysis
                .body_ir
                .local_enum_variant_data(variant)?
                .map(|data| EnumVariant::BodyLocal { variant, data })),
        }
    }
}
