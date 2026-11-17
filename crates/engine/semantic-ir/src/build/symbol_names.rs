//! Compact workspace-symbol discovery, built while declarations are already available.

use rg_def_map::DefMap;
use rg_ir_model::{
    EnumVariantRef, FieldRef, ImplRef, ItemOwner, ModuleId, ModuleRef, TraitDefRef, TypeDefId,
    identity::DeclarationRef,
};
use rg_text::Name;
use wincode::{SchemaRead, SchemaWrite};

use crate::ItemStore;

/// Searching names does not need signatures, locations, or the declarations of unrelated targets.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub struct SymbolNameData {
    pub name: Name,
    pub declaration: DeclarationRef,
    pub container: Option<DeclarationRef>,
}

impl SymbolNameData {
    pub(crate) fn for_target(items: &ItemStore, definitions: &DefMap) -> Vec<Self> {
        let origin = items.origin();
        let mut names = Vec::new();
        for (index, module) in definitions.modules().iter().enumerate() {
            if let Some(name) = &module.name {
                names.push(Self {
                    name: name.clone(),
                    declaration: DeclarationRef::Module(ModuleRef {
                        origin,
                        module: ModuleId(index),
                    }),
                    container: None,
                });
            }
        }
        for item in items.semantic_items() {
            let declaration = DeclarationRef::from(item.item());
            let container = match item.item_owner() {
                Some(ItemOwner::Trait(id)) => {
                    Some(DeclarationRef::from(TraitDefRef { origin, id }))
                }
                Some(ItemOwner::Impl(id)) => Some(DeclarationRef::from(ImplRef { origin, id })),
                _ => None,
            };
            if let Some(name) = item.name() {
                names.push(Self {
                    name: name.clone(),
                    declaration,
                    container,
                });
            }
            let Some(ty) = item.type_def() else {
                continue;
            };
            let fields = item
                .struct_data()
                .map(|data| data.fields.fields())
                .or_else(|| item.union_data().map(|data| data.fields.as_slice()));
            for (index, field) in fields.into_iter().flatten().enumerate() {
                names.push(Self {
                    name: Name::new(
                        field.key.as_ref().map_or_else(
                            || "<unsupported>".to_owned(),
                            |key| key.declaration_label(),
                        ),
                    ),
                    declaration: DeclarationRef::from(FieldRef { owner: ty, index }),
                    container: Some(declaration),
                });
            }
            if let Some(data) = item.enum_data() {
                let TypeDefId::Enum(enum_id) = ty.id else {
                    unreachable!("enum item has enum identity")
                };
                for (index, variant) in data.variants.iter().enumerate() {
                    names.push(Self {
                        name: variant.name.clone(),
                        declaration: DeclarationRef::from(EnumVariantRef {
                            origin,
                            enum_id,
                            index,
                        }),
                        container: Some(declaration),
                    });
                }
            }
        }
        names
    }
}
