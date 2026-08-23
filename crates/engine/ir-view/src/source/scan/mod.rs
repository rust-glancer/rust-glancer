//! Private source scanners behind the indexed view facade.
//!
//! DefMap, Semantic IR, and Body IR retain the source facts that belong to their domains. This
//! module interprets those facts for point queries, whole-file searches, and completion sites so
//! the storage crates do not expose editor-shaped candidate families.

mod body;
mod current_use;
mod definition;
mod import_completion;
mod module_scope;
mod signature;
mod trait_impl;
mod type_path;

use rg_item_tree::TypeRef;

pub(super) use self::{
    body::{
        BindingSurface, BodyCursorScanner, BodyQualifiedPathContext, BodySourceCandidate,
        BodySourceScanner, BodyUnqualifiedNameContext, DotCompletionSiteScanner, LabelScopeScanner,
        PathCompletionSiteScanner, PatternCompletionKind, RecordFieldCompletionSiteScanner,
        RecordFieldKeySurface, UnqualifiedCompletionSiteScanner, ValueReferenceSource,
        ValueReferenceSurface,
    },
    current_use::CurrentUsePathScanner,
    definition::{DefinitionSourceCandidate, DefinitionSourceScanner},
    import_completion::ImportPathCompletionSiteScanner,
    module_scope::{ModuleFileBase, ModuleSourceSiteScanner},
    signature::{
        SignatureCompletionSite, SignatureSourceCandidate, SignatureSourceScanner,
        SignatureTypePathScope,
    },
    trait_impl::TraitImplSourceSiteScanner,
};

/// Type-shaped projection of the path before a segment being completed.
///
/// Ordinary path prefixes retain their generic arguments as a `TypeRef`. A bare qualified anchor
/// stays split until the body/signature type context can resolve both halves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum AssociatedPathQualifier {
    /// An ordinary type-shaped prefix such as `Widget::<u8>` or `T`.
    Type(TypeRef),
    /// The two sides of an explicitly selected trait prefix such as `<T as Factory>`.
    QualifiedTrait {
        self_ty: TypeRef,
        trait_ref: TypeRef,
    },
}

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
