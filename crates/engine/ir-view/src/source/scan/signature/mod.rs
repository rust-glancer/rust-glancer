//! Source scanning over semantic item signatures.
//!
//! Semantic item stores own signature data, while the indexed view owns the source interpretation
//! of that data. The same scanner therefore works for retained Semantic IR and for one declaration
//! lowered from the editor buffer. It finds fields, variants, associated functions, and nested
//! type paths without making source-query APIs part of either storage layer. Completion can select
//! qualified or unqualified paths, explicit associated bindings, and a possible binding before
//! `=`. When recovery leaves no path at all, the scanner can still return the narrowest item scope
//! containing the cursor.
//!
//! ```text
//! struct Wrapper<T> {
//!     value: outer::Inner<Vec<T>>,
//!     ^^^^^  ^^^^^  ^^^^^ ^^^ ^ fields and every nested type path remain independently navigable
//! }
//!
//! fn load<T>(value: outer::Inn$0) -> T
//!                   ^^^^^^^ completion receives qualifier `outer`, prefix span `Inn`, and `T`
//!                             remains available through the function's generic scope
//!
//! fn read<T>(value: Iterator<Ite$0 = T>) {}
//!                            ^^^ explicit associated binding, resolved in `read`'s scope
//! ```

mod collector;
mod walker;

use rg_ir_model::{
    CrateRef, DefMapRef, EnumVariantRef, FieldRef, FunctionRef, GenericDefRef, Path,
};
use rg_item_tree::TypeRef;
use rg_package_store::PackageStoreError;
use rg_parse::{FileId, Span};
use rg_semantic_ir::{ItemStoreQuery, TypePathContext};

use self::{
    collector::{
        ImplicitAssociatedTypeBindingCollector, SignatureCompletionCollector,
        SignatureOccurrenceCollector,
    },
    walker::SignatureItemWalker,
};
use super::{AssociatedPathQualifier, TypeNamePosition};
use crate::IndexedViewDb;

/// Semantic scope carried by a type path written in an item signature.
///
/// In `impl<T> Wrapper<T> { fn map<U>(_: U$0) {} }`, the context resolves module paths and impl
/// `Self`, while the function generic owner exposes both `U` and inherited `T`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SignatureTypePathScope {
    pub(crate) context: TypePathContext,
    pub(crate) generic_owner: GenericDefRef,
}

/// Completion-shaped interpretation of one signature type path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SignatureCompletionSite {
    /// A binding already disambiguated by `=`, such as `Iterator<It$0 = u8>`.
    AssociatedTypeBinding {
        scope: SignatureTypePathScope,
        trait_ref: TypeRef,
        member_prefix_span: Span,
        existing_bindings: Vec<String>,
    },
    /// A signature path such as `fn load(_: model::Us$0)` whose qualifier must be resolved.
    Qualified {
        scope: SignatureTypePathScope,
        module_qualifier: Option<Path>,
        associated_qualifier: AssociatedPathQualifier,
        member_prefix_span: Span,
    },
    /// A signature name such as `fn load<T>(_: T$0)` whose candidates come from visible scopes.
    Unqualified {
        scope: SignatureTypePathScope,
        member_prefix_span: Span,
        member_prefix: String,
        position: TypeNamePosition,
    },
}

/// One semantic signature source node that can become an indexed occurrence.
///
/// Top-level item names already come from DefMap. Semantic signatures add names owned below that
/// boundary—fields, variants, associated functions—and type paths that need their item's generic
/// and impl context for resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SignatureSourceCandidate {
    Field {
        field: FieldRef,
        span: Span,
    },
    Function {
        function: FunctionRef,
        span: Span,
    },
    EnumVariant {
        variant: EnumVariantRef,
        span: Span,
    },
    TypePath {
        scope: SignatureTypePathScope,
        path: Path,
        type_ref: Option<TypeRef>,
        file_id: FileId,
        span: Span,
    },
}

impl SignatureSourceCandidate {
    fn span(&self) -> Span {
        match self {
            Self::Field { span, .. }
            | Self::Function { span, .. }
            | Self::EnumVariant { span, .. }
            | Self::TypePath { span, .. } => *span,
        }
    }
}

/// Scans semantic item signatures for declaration names and nested type paths.
///
/// Type references are walked recursively through generic defaults, bounds, where predicates,
/// fields, parameters, return types, and qualified anchors. For `Outer<Inner>`, both paths are
/// emitted rather than treating the annotation as one opaque source span.
pub(crate) struct SignatureSourceScanner<'view, 'db> {
    db: &'view IndexedViewDb<'db>,
    origin: DefMapRef,
    file_id: Option<FileId>,
    offset: Option<u32>,
}

impl<'view, 'db> SignatureSourceScanner<'view, 'db> {
    pub(crate) fn at(
        db: &'view IndexedViewDb<'db>,
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
    ) -> Self {
        Self {
            db,
            origin: DefMapRef::Crate(crate_ref),
            file_id: Some(file_id),
            offset: Some(offset),
        }
    }

    pub(crate) fn at_origin(
        db: &'view IndexedViewDb<'db>,
        origin: DefMapRef,
        file_id: FileId,
        offset: u32,
    ) -> Self {
        Self {
            db,
            origin,
            file_id: Some(file_id),
            offset: Some(offset),
        }
    }

    /// Returns the narrowest signature type path that can accept completion at the offset.
    ///
    /// `fn load(_: model::Us$0)` produces a qualified site for `model`, while
    /// `fn load<T>(_: T$0)` produces an unqualified site carrying the function's generic scope.
    /// An explicit binding such as `fn load(_: Iterator<It$0 = u8>)` is selected before either
    /// ordinary path interpretation.
    pub(crate) fn completion_site_at(
        db: &'view IndexedViewDb<'db>,
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
    ) -> Result<Option<SignatureCompletionSite>, PackageStoreError> {
        Self::completion_site_at_origin(db, DefMapRef::Crate(crate_ref), file_id, offset)
    }

    pub(crate) fn completion_site_at_origin(
        db: &'view IndexedViewDb<'db>,
        origin: DefMapRef,
        file_id: FileId,
        offset: u32,
    ) -> Result<Option<SignatureCompletionSite>, PackageStoreError> {
        let Some(items) = db.item_store_for_origin(origin)? else {
            return Ok(None);
        };
        let collector = SignatureItemWalker::new(
            db,
            items,
            origin,
            SignatureCompletionCollector::new(file_id, offset),
        )
        .scan()?;
        Ok(collector.finish())
    }

    /// Returns a possible associated binding whose `=` has not been written yet.
    ///
    /// In `fn load(_: Iterator<It$0>)`, the ordinary completion collector still owns `It` as a
    /// nested type argument. This separate scan lets the caller add associated-type names without
    /// suppressing those normal type candidates.
    pub(crate) fn implicit_associated_type_binding_site_at(
        db: &'view IndexedViewDb<'db>,
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
    ) -> Result<Option<SignatureCompletionSite>, PackageStoreError> {
        Self::implicit_associated_type_binding_site_at_origin(
            db,
            DefMapRef::Crate(crate_ref),
            file_id,
            offset,
        )
    }

    pub(crate) fn implicit_associated_type_binding_site_at_origin(
        db: &'view IndexedViewDb<'db>,
        origin: DefMapRef,
        file_id: FileId,
        offset: u32,
    ) -> Result<Option<SignatureCompletionSite>, PackageStoreError> {
        let Some(items) = db.item_store_for_origin(origin)? else {
            return Ok(None);
        };
        let collector = SignatureItemWalker::new(
            db,
            items,
            origin,
            ImplicitAssociatedTypeBindingCollector::new(file_id, offset),
        )
        .scan()?;
        Ok(collector.finish())
    }

    /// Find the declaration scope surrounding an explicit empty type path.
    ///
    /// `fn load<T>(_: $0)` has no `TypePath` for the ordinary signature walker to visit. This scan
    /// instead selects the narrowest semantic item span containing the cursor, then returns that
    /// item's type-path context and generic owner so `T` remains visible.
    pub(crate) fn empty_type_scope_at(
        db: &'view IndexedViewDb<'db>,
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
    ) -> Result<Option<SignatureTypePathScope>, PackageStoreError> {
        Self::empty_type_scope_at_origin(db, DefMapRef::Crate(crate_ref), file_id, offset)
    }

    pub(crate) fn empty_type_scope_at_origin(
        db: &'view IndexedViewDb<'db>,
        origin: DefMapRef,
        file_id: FileId,
        offset: u32,
    ) -> Result<Option<SignatureTypePathScope>, PackageStoreError> {
        let Some(items) = db.item_store_for_origin(origin)? else {
            return Ok(None);
        };
        let Some(def_map) = db.def_map_for_origin(origin)? else {
            return Ok(None);
        };
        let item_query = ItemStoreQuery::new(db);
        let mut best = super::NarrowestSourceSite::new();

        for item in items.semantic_items() {
            if item.source().file_id != file_id {
                continue;
            }
            let span = item
                .span()
                .or_else(|| {
                    item.local_def()
                        .and_then(|local| def_map.local_def(local.local_def))
                        .map(|data| data.span)
                })
                .or_else(|| {
                    item.local_impl()
                        .and_then(|local| def_map.local_impl(local.local_impl))
                        .map(|data| data.span)
                });
            let Some(span) = span.filter(|span| span.touches(offset)) else {
                continue;
            };
            let generic_owner = GenericDefRef::from(item.item());
            let Some(context) = item_query.type_path_context_for_generic_def(generic_owner)? else {
                continue;
            };
            let context = db.current_signature_context(context)?;
            best.consider(
                SignatureTypePathScope {
                    context,
                    generic_owner,
                },
                span.len(),
            );
        }

        Ok(best.finish())
    }

    pub(crate) fn in_crate(
        db: &'view IndexedViewDb<'db>,
        crate_ref: CrateRef,
        file_id: Option<FileId>,
    ) -> Self {
        Self {
            db,
            origin: DefMapRef::Crate(crate_ref),
            file_id,
            offset: None,
        }
    }

    /// Collect the source facts owned by each semantic item family in the selected store.
    pub(crate) fn scan(self) -> Result<Vec<SignatureSourceCandidate>, PackageStoreError> {
        let Some(items) = self.db.item_store_for_origin(self.origin)? else {
            return Ok(Vec::new());
        };
        let collector = SignatureItemWalker::new(
            self.db,
            items,
            self.origin,
            SignatureOccurrenceCollector::new(self.file_id, self.offset),
        )
        .scan()?;
        Ok(collector.finish())
    }
}
