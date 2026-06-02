mod def_map;
mod item;

pub use self::{
    def_map::{
        DefMap, DefMapBuilder, ImportBinding, ImportData, ImportKind, ImportPath, ImportSourcePath,
        LocalDefData, LocalDefKind, LocalImplData, MacroDefinitionData, MacroDefinitionPayload,
        ModuleData, ModuleOrigin, ModuleScope, ModuleScopeBuilder, Namespace, PackageDefMaps, Path,
        PathSegment, ScopeBinding, ScopeBindingOrigin, ScopeEntry, ScopeEntryRef, ScopeNamespace,
        TargetData, VisibleScopeDef, VisibleScopeDefs, VisibleScopeOrigin,
    },
    item::{
        ItemLookupIndex, ItemStore, ItemStoreBuilder, ItemStoreQuery, ItemStoreSource,
        SemanticItemView, TypePathContext,
    },
};

fn push_unique<T: PartialEq>(items: &mut Vec<T>, item: T) {
    if !items.contains(&item) {
        items.push(item);
    }
}
