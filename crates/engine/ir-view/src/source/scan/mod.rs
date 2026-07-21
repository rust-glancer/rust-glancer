//! Private source scanners behind the indexed view facade.
//!
//! DefMap, Semantic IR, and Body IR retain the source facts that belong to their domains. This
//! module interprets those facts for point queries, whole-file searches, and completion sites so
//! the storage crates do not expose editor-shaped candidate families.

mod body;
mod definition;
mod import_completion;
mod signature;
mod type_path;

pub(super) use self::{
    body::{
        BindingSurface, BodyCursorScanner, BodySourceCandidate, BodySourceScanner,
        BodyUnqualifiedNameContext, DotCompletionSiteScanner, PathCompletionSiteScanner,
        RecordFieldCompletionSiteScanner, RecordFieldKeySurface, UnqualifiedCompletionSiteScanner,
        ValueReferenceSource, ValueReferenceSurface,
    },
    definition::{DefinitionSourceCandidate, DefinitionSourceScanner},
    import_completion::ImportPathCompletionSiteScanner,
    signature::{
        SignatureCompletionSite, SignatureSourceCandidate, SignatureSourceScanner,
        SignatureTypePathScope,
    },
};

/// Syntactic role of a type-shaped path at the completion cursor.
///
/// A path naming a type accepts type parameters. A bare path used as a whole `<...>` argument may
/// instead name a const parameter, because Rust's generic argument syntax is ambiguous until the
/// referenced declaration is known.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TypeNamePosition {
    /// Ordinary type syntax, such as `fn load(_: Us$0)` or `Wrapper<&Us$0>`.
    Type,
    /// A whole generic argument, such as `Array<N$0>`, whose kind is not known from syntax alone.
    BareGenericArgument,
}

impl TypeNamePosition {
    /// Keeps const ambiguity only for a whole, unqualified generic argument such as `Array<N>`.
    /// Once syntax adds structure (`&N`, `(N,)`, or `Wrapper<N>`), the path names a type again.
    fn for_path(self, path: &rg_item_tree::TypePath) -> Self {
        if matches!(self, Self::BareGenericArgument)
            && !path.absolute
            && path.anchor.is_none()
            && path.segments.len() == 1
            && path
                .segments
                .first()
                .is_some_and(|segment| segment.args.is_empty())
        {
            Self::BareGenericArgument
        } else {
            Self::Type
        }
    }
}

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
