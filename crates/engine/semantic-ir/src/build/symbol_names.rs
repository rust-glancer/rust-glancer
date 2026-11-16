//! Compact search names, with separate discovery for names that member macros can reveal.
//!
//! Searching for `child_modules` can skip an ordinary impl containing only `insert` and `remove`.
//! An impl containing `generate_methods!()` cannot be skipped by spelling: the macro might emit
//! `child_modules`. Broad search first discovers names from those owners, then filters names
//! before loading declarations and source locations to render the matches.

use rg_def_map::DefMap;
use rg_ir_model::{
    CrateRef, EnumVariantRef, FieldRef, ImplRef, ItemOwner, ItemStoreRef, ModuleId, ModuleRef,
    TraitDefRef, TypeDefId, identity::DeclarationRef,
};
use rg_source::QueryContext;
use rg_std::Shrink;
use rg_text::Name;
use wincode::{SchemaRead, SchemaWrite};

use crate::{
    AssociatedItemStore, ItemStore, SemanticItemView, associated_declarations, target_declarations,
};

/// Names known without opening a trait or impl's member store. These select an owner, not a
/// member ID: member IDs are assigned only when the owner's declarations are lowered.
/// Even a written member cannot derive its ID from syntax order, since preceding macro output
/// can consume slots in the same arena.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, Shrink)]
pub(crate) struct MemberNameData {
    pub owner: ItemStoreRef,
    pub names: Vec<Name>,
}

/// Searchable names can identify a declaration directly or select a member store to inspect.
/// Module declarations have identities already; written member names need only their owner.
/// Both tables stay separate from the signatures and source maps used to render a match.
///
/// The target's table puts macro-free owners in `members` and other owners in `macro_owners`.
/// Macro discovery returns another table containing only `declarations`, with the real IDs of
/// both written and generated members from those owners.
#[derive(Debug, Default, PartialEq, Eq, SchemaRead, SchemaWrite, Shrink)]
pub struct SymbolNameIndex {
    pub(crate) declarations: Vec<SymbolNameData>,
    pub(crate) members: Vec<MemberNameData>,
    // Macro output can contain any name. Such owners always need discovery, so retaining their
    // written names here would not help reject them. Lookup hints still keep those written names.
    pub(crate) macro_owners: Vec<ItemStoreRef>,
}

/// A searchable name paired with its resolved declaration and optional container identity.
/// Filtering these records needs no signatures or locations; rendering a match loads them later.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, Shrink)]
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

        // Scope declarations already have identities. Fields and variants are searchable too,
        // but associated members remain in their separate owner summaries.
        for item in items.scope_items() {
            let declaration = DeclarationRef::from(item.item());
            names.extend(Self::for_item(item));
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

    /// Iterate member names with the IDs assigned by semantic lowering, including macro output.
    pub(crate) fn for_members(
        owner: ItemStoreRef,
        members: &AssociatedItemStore,
    ) -> impl Iterator<Item = Self> + '_ {
        members.semantic_items(owner).filter_map(Self::for_item)
    }

    fn for_item(item: SemanticItemView<'_>) -> Option<Self> {
        let name = item.name()?.clone();
        let origin = item.item().origin();
        let container = match item.item_owner() {
            Some(ItemOwner::Trait(id)) => Some(DeclarationRef::from(TraitDefRef { origin, id })),
            Some(ItemOwner::Impl(id)) => Some(DeclarationRef::from(ImplRef { origin, id })),
            _ => None,
        };
        Some(Self {
            name,
            declaration: DeclarationRef::from(item.item()),
            container,
        })
    }
}

/// Discover all member names from owners containing macros, including their written members.
///
/// This query is demanded by broad search, not startup. Those owners expand in parallel because
/// their names cannot be predicted from the search string. Ordinary owners remain unopened until
/// a search matches their summary. The compact result is persisted, so repeated searches and
/// restarts can reuse the names while its dependencies stay unchanged.
#[ajika::query(returns(ref))]
pub(super) fn macro_symbol_names(
    db: &QueryContext<'_>,
    krate: CrateRef,
) -> ajika::Result<SymbolNameIndex> {
    let target = target_declarations(db, krate)?;
    let names = target.symbol_names.value_ref(db)?;
    let members = db.parallel_map(&names.macro_owners, |db, &owner| {
        let Some(declarations) = associated_declarations(db, krate, owner.id)? else {
            return Ok(Vec::new());
        };
        let members = declarations.items.value_ref(db)?;
        Ok(SymbolNameData::for_members(owner, members).collect::<Vec<_>>())
    })?;
    let mut names = SymbolNameIndex {
        declarations: members.into_iter().flatten().collect(),
        ..SymbolNameIndex::default()
    };
    names.shrink_to_fit();
    Ok(names)
}
