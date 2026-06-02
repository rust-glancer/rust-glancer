mod autoderef;
mod build;
mod cursor;
mod deref;
mod impl_match;
mod ir;
mod item_lookup_index;
mod item_query;
mod item_store;
mod item_store_lowering;
mod path_query;
mod store;
mod type_conversion;
mod view;

#[cfg(test)]
mod tests;

pub use self::{
    autoderef::{
        Autoderef, AutoderefCandidate, AutoderefCandidates, AutoderefMode,
        ReferencePeelingCandidates,
    },
    cursor::SemanticCursorCandidate,
    impl_match::ImplMatcher,
    ir::{PackageIr, SemanticIrStats, TypePathContext},
    item_lookup_index::ItemLookupIndex,
    item_query::{ItemStoreQuery, ItemStoreSource},
    item_store::{ItemStore, ItemStoreBuilder},
    item_store_lowering::{ItemStoreLowerer, ItemStoreSourceReader},
    path_query::ItemPathQuery,
    store::{SemanticIrDb, SemanticIrReadTxn},
    type_conversion::{
        subst_from_generics, substitute_type_param, ty_from_type_path_resolution,
        ty_from_type_ref_in_context, type_ref_is_self,
    },
    view::SemanticItemView,
};

pub use rg_item_tree::{
    Documentation, EnumVariantItem, FieldItem, FieldKey, FieldList, FunctionItem,
    FunctionQualifiers, GenericParams, Mutability, ParamItem, TypeBound, TypeRef, VisibilityLevel,
    WherePredicate,
};

fn push_unique<T: PartialEq>(items: &mut Vec<T>, item: T) {
    if !items.contains(&item) {
        items.push(item);
    }
}
