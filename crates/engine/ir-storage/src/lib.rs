mod body;
mod def_map;
mod item;

pub use self::{
    body::BodyLocalItems,
    def_map::{
        DefMap, DefMapBuilder, DefMapQuery, DefMapSource, ImportBinding, ImportData, ImportKind,
        ImportPath, ImportSourcePath, LocalDefData, LocalDefKind, LocalImplData,
        MacroDefinitionData, MacroDefinitionPayload, ModuleData, ModuleOrigin, ModuleScope,
        ModuleScopeBuilder, NameResolutionFilter, Namespace, PackageDefMaps, PathResolver,
        ResolvePathResult, ScopeBinding, ScopeBindingOrigin, ScopeEntry, ScopeEntryRef,
        ScopeNamespace, ScopeResolutionEnv, TargetData, TargetResolutionEnv, VisibleScopeDef,
        VisibleScopeDefs, VisibleScopeOrigin,
    },
    item::{
        ItemLookupIndex, ItemStore, ItemStoreBuilder, ItemStoreQuery, ItemStoreSource,
        SemanticItemView, TargetItemQuery, TypePathContext,
    },
};

fn push_unique<T: PartialEq>(items: &mut Vec<T>, item: T) {
    if !items.contains(&item) {
        items.push(item);
    }
}
