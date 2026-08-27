//! Definition path queries over semantic item stores.

use rg_def_map::DefMapSource;
use rg_ir_model::{ModuleRef, Path, SemanticItemRef};
use rg_semantic_ir::{
    GenericsQuery, ItemResolutionQuery, ItemStoreQuery, ItemStoreSource, TypePathContext,
    TypePathResolution,
};
use rg_std::UniqueVec;

/// Resolves paths into semantic item refs.
#[derive(Clone)]
pub struct ItemPathQuery<'a, D, I> {
    definitions: ItemResolutionQuery<'a, D, I>,
    generics: GenericsQuery<'a, I>,
}

impl<'a, D, I> ItemPathQuery<'a, D, I>
where
    D: DefMapSource,
    I: ItemStoreSource<'a, Error = D::Error>,
{
    pub fn new(def_maps: D, items: I) -> Self {
        Self {
            definitions: ItemResolutionQuery::new(def_maps, items.clone()),
            generics: GenericsQuery::new(items),
        }
    }

    pub fn items(&self) -> &ItemStoreQuery<'a, I> {
        self.definitions.items()
    }

    pub fn generics(&self) -> &GenericsQuery<'a, I> {
        &self.generics
    }

    pub fn resolve_type_path(
        &self,
        context: TypePathContext,
        path: &Path,
    ) -> Result<TypePathResolution, D::Error> {
        self.definitions.resolve_type_path(context, path)
    }

    /// Resolves a type path from a synthetic body scope without applying owner-context fallbacks.
    ///
    /// Callers use this exact lookup before constructing the richer body type context needed for
    /// `Self` and associated aliases.
    pub fn resolve_lexical_type_path(
        &self,
        from: ModuleRef,
        path: &Path,
    ) -> Result<TypePathResolution, D::Error> {
        self.definitions.resolve_lexical_type_path(from, path)
    }

    pub fn semantic_items_for_type_path(
        &self,
        context: TypePathContext,
        path: &Path,
    ) -> Result<UniqueVec<SemanticItemRef>, D::Error> {
        self.definitions.semantic_items_for_type_path(context, path)
    }
}
