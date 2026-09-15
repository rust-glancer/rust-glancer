//! The declaration index shared by queries over one active body.

use std::{
    collections::HashMap,
    sync::{Arc, OnceLock},
};

use rg_ir_model::{ImplRef, TraitDefRef, TraitImplRef, TypeDefRef};
use rg_package_store::PackageStoreError;
use rg_std::UniqueVec;
use rg_text::Name;

/// Successful body-local declaration scan shared by one body-resolution pass.
///
/// Body resolution creates short-lived query contexts over progressively stronger inference views.
/// Every context clones this handle, so the first lookup scans the active body's item stores and
/// all later lookups reuse the resulting [`BodyLocalItemIndex`]. The index is request-local: it is
/// neither serialized nor shared with another body.
///
/// Only a complete scan enters the `OnceLock`. If loading a package fails, the cache stays empty so
/// a later query can retry instead of treating a partial declaration set as authoritative.
#[derive(Clone, Default)]
pub(crate) struct BodyLocalItemCache {
    index: Arc<OnceLock<BodyLocalItemIndex>>,
}

impl BodyLocalItemCache {
    /// Publish only a complete index. A package-loading error leaves the cache empty for retry.
    pub(crate) fn index_or_try_init(
        &self,
        load: impl FnOnce() -> Result<BodyLocalItemIndex, PackageStoreError>,
    ) -> Result<&BodyLocalItemIndex, PackageStoreError> {
        if let Some(index) = self.index.get() {
            return Ok(index);
        }

        let index = load()?;
        let _ = self.index.set(index);
        Ok(self
            .index
            .get()
            .expect("a successful body-local index load should publish a value"))
    }
}

/// Compact lookup surface derived from the body stores visible to one active body.
///
/// This mirrors only the persisted lookup lanes needed during body resolution. For example,
/// `impl Local { fn run(&self) {} }` contributes an inherent impl and the name `run`, while
/// `trait Paint { fn draw(&self); }` contributes a function surface and the `draw -> Paint`
/// reverse-name entry. The full declaration data remains in the item stores.
#[derive(Default)]
pub(crate) struct BodyLocalItemIndex {
    pub(crate) inherent_impls_by_type: HashMap<TypeDefRef, UniqueVec<ImplRef>>,
    pub(crate) inherent_item_names_by_type: HashMap<TypeDefRef, BodyLocalInherentItemNames>,
    pub(crate) trait_impls: UniqueVec<TraitImplRef>,
    pub(crate) trait_impls_by_type: HashMap<TypeDefRef, UniqueVec<TraitImplRef>>,
    pub(crate) traits_with_functions: UniqueVec<TraitDefRef>,
    pub(crate) traits_with_associated_items: UniqueVec<TraitDefRef>,
    pub(crate) traits_by_function_name: HashMap<Name, UniqueVec<TraitDefRef>>,
    pub(crate) traits_by_const_name: HashMap<Name, UniqueVec<TraitDefRef>>,
}

/// Names declared by body-local inherent impls, separated by associated-item kind.
///
/// An active source impl replaces the saved impl's member of the same kind and name. If the saved
/// snapshot has `fn render(&self) -> Old` and the overlay has `fn render(&self) -> New`, retaining
/// `render` here lets consumers suppress the stale function without hiding unrelated saved consts
/// or type aliases that happen to use other names.
#[derive(Default)]
pub(crate) struct BodyLocalInherentItemNames {
    pub(crate) functions: UniqueVec<Name>,
    pub(crate) consts: UniqueVec<Name>,
    pub(crate) type_aliases: UniqueVec<Name>,
}

impl BodyLocalInherentItemNames {
    pub(crate) fn extend(&mut self, other: &Self) {
        self.functions.extend(other.functions.iter().cloned());
        self.consts.extend(other.consts.iter().cloned());
        self.type_aliases.extend(other.type_aliases.iter().cloned());
    }

    pub(crate) fn contains_function(&self, name: &str) -> bool {
        self.functions
            .iter()
            .any(|candidate| candidate.as_str() == name)
    }

    pub(crate) fn contains_const(&self, name: &str) -> bool {
        self.consts
            .iter()
            .any(|candidate| candidate.as_str() == name)
    }

    pub(crate) fn contains_type_alias(&self, name: &str) -> bool {
        self.type_aliases
            .iter()
            .any(|candidate| candidate.as_str() == name)
    }
}
