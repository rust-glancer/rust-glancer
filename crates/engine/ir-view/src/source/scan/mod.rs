//! Private source scanners behind the indexed view facade.
//!
//! DefMap, Semantic IR, and Body IR retain the source facts that belong to their domains. This
//! module interprets those facts for point queries, whole-file searches, and completion sites so
//! the storage crates do not expose editor-shaped candidate families.

mod body;
mod definition;
mod import_completion;
mod signature;

pub(super) use self::{
    body::{
        BindingSurface, BodyCursorScanner, BodySourceCandidate, BodySourceScanner,
        DotCompletionSiteScanner, PathCompletionSiteScanner, RecordFieldCompletionSiteScanner,
        RecordFieldKeySurface, UnqualifiedCompletionSiteScanner, ValueReferenceSource,
        ValueReferenceSurface,
    },
    definition::{DefinitionSourceCandidate, DefinitionSourceScanner},
    import_completion::ImportPathCompletionSiteScanner,
    signature::{SignatureSourceCandidate, SignatureSourceScanner},
};

/// Chooses the source site backed by the narrowest enclosing syntax.
///
/// Recovered syntax can expose both an inner site and one or more enclosing sites at the same
/// offset. Point queries should target the inner spelling, so all site scanners share this
/// ordering. For example, both record expressions below enclose the cursor, but completion belongs
/// to the inner field list:
///
/// ```text
/// Outer { inner: Inner { na$0 } }
///                  ^^^^^^^^^^^^^ inner site
/// ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ outer site
/// ```
struct NarrowestSourceSite<T> {
    selected: Option<(T, u32)>,
}

impl<T> NarrowestSourceSite<T> {
    fn new() -> Self {
        Self { selected: None }
    }

    fn consider(&mut self, site: T, source_len: u32) {
        if self
            .selected
            .as_ref()
            .is_none_or(|(_, selected_len)| source_len < *selected_len)
        {
            self.selected = Some((site, source_len));
        }
    }

    fn finish(self) -> Option<T> {
        self.selected.map(|(site, _)| site)
    }
}
