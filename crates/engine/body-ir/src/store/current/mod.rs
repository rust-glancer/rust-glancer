//! Frozen semantic objects owned by one current-source request.
//!
//! A body supplies lexical lookup and a selected impl supplies a complete member list. Those can
//! describe the same syntax with different identities. Only the context selected for source
//! scanning is exposed to signature queries; member queries use the explicit impl selection.

mod body;

use std::collections::HashSet;

use rg_ir_model::{BodyRef, CrateRef, DefMapRef, ImplRef, ModuleRef};
use rg_parse::{FileId, Span};

use crate::{BodyLocalItems, BodySource};

pub(crate) use self::body::CurrentBody;

#[derive(Debug, Clone, Copy)]
pub(crate) enum CurrentImplRole {
    /// No body owns the cursor, so this store also describes current header occurrences.
    Signature,
    /// A body's context owns source scanning; this complete impl is only for member queries.
    Members,
}

/// A complete current impl collected without an expression body.
///
/// Its declarations use a `DefMapRef::Body` origin so they can share local-item storage,
/// but that origin has no `BodyData` or inferred facts.
#[derive(Debug)]
pub(crate) struct CurrentImplData {
    pub(crate) source: BodySource,
    pub(crate) impl_ref: ImplRef,
    /// Saved containing module used to resolve paths in the header, such as `model::Worker`.
    pub(crate) fallback_module: ModuleRef,
    pub(crate) role: CurrentImplRole,
    pub(crate) items: BodyLocalItems,
}

/// A complete impl chosen from current syntax. Its semantic identity may still be saved.
///
/// `source` records the current span even when `impl_ref` points to a saved declaration at
/// a different offset.
#[derive(Debug)]
pub(crate) struct SelectedImpl {
    pub(crate) source: BodySource,
    pub(crate) impl_ref: ImplRef,
}

/// Current bodies and declarations layered over a saved read transaction.
///
/// Different source text masks other saved bodies in that file because their ranges no longer
/// describe this document. Exact source retains unselected saved bodies. None of these objects
/// enters saved crate indexes or package artifacts.
#[derive(Debug, Default)]
pub struct CurrentSourceStore {
    pub(crate) masked_files: HashSet<(CrateRef, FileId)>,
    pub(crate) bodies: Vec<CurrentBody>,
    pub(crate) impls: Vec<CurrentImplData>,
    pub(crate) selected_impls: Vec<SelectedImpl>,
}

impl CurrentSourceStore {
    /// Both body and declaration lookups dispatch by origin. Check that each origin names
    /// exactly one local store before any query can observe it.
    pub(crate) fn validate(&self) -> anyhow::Result<()> {
        let mut origins = HashSet::new();
        anyhow::ensure!(
            self.bodies
                .iter()
                .all(|body| origins.insert(body.body_ref())),
            "current source contains one body identity more than once",
        );
        for current in &self.impls {
            let DefMapRef::Body(origin) = current.impl_ref.origin else {
                anyhow::bail!("current impl must belong to a request-local origin");
            };
            anyhow::ensure!(
                origins.insert(origin),
                "current declaration origin overlaps another context"
            );
            anyhow::ensure!(
                current.items.def_map().own_ref() == current.impl_ref.origin,
                "current impl belongs to a different declaration store"
            );
        }
        Ok(())
    }

    pub(crate) fn masks_file(&self, crate_ref: CrateRef, file: FileId) -> bool {
        self.masked_files.contains(&(crate_ref, file))
    }

    /// A whole-crate saved body value cannot include rebuilt bodies or file masks.
    /// Adding only header declarations does not change that body inventory.
    pub(crate) fn affects_crate(&self, crate_ref: CrateRef) -> bool {
        self.masked_files
            .iter()
            .any(|(owner, _)| *owner == crate_ref)
            || self
                .bodies
                .iter()
                .any(|body| body.body_ref().crate_ref == crate_ref)
    }

    pub(crate) fn bodies(&self) -> &[CurrentBody] {
        &self.bodies
    }

    pub(crate) fn contains_body(&self, body_ref: BodyRef) -> bool {
        self.bodies.iter().any(|body| body.body_ref() == body_ref)
    }

    pub(crate) fn contains_origin(&self, origin: DefMapRef) -> bool {
        let DefMapRef::Body(body_ref) = origin else {
            return false;
        };
        self.contains_body(body_ref)
            || self
                .impls
                .iter()
                .any(|current| current.impl_ref.origin == origin)
    }

    pub(crate) fn items(&self, origin: DefMapRef) -> Option<&BodyLocalItems> {
        let DefMapRef::Body(body_ref) = origin else {
            return None;
        };
        self.bodies
            .iter()
            .find(|body| body.body_ref() == body_ref)
            .map(CurrentBody::local_items)
            .or_else(|| {
                self.impls
                    .iter()
                    .find(|current| current.impl_ref.origin == origin)
                    .map(|current| &current.items)
            })
    }

    /// A complete impl prepared beside a body must not introduce a second signature identity.
    ///
    /// For `impl<T> ...`, scanning both copies would give the same written `T` two different
    /// generic identities.
    pub(crate) fn signature_origins(
        &self,
        crate_ref: CrateRef,
        file: FileId,
    ) -> impl Iterator<Item = DefMapRef> + '_ {
        self.bodies
            .iter()
            .filter(move |body| {
                body.body_ref().crate_ref == crate_ref && body.view().source().file_id == file
            })
            .map(|body| DefMapRef::Body(body.body_ref()))
            .chain(
                self.impls
                    .iter()
                    .filter(move |current| {
                        current.impl_ref.origin.origin_crate() == crate_ref
                            && current.source.file_id == file
                            && matches!(current.role, CurrentImplRole::Signature)
                    })
                    .map(|current| current.impl_ref.origin),
            )
    }

    pub(crate) fn declaration_fallback(&self, module: ModuleRef) -> Option<ModuleRef> {
        self.impls
            .iter()
            .find(|current| current.impl_ref.origin == module.origin)
            .map(|current| current.fallback_module)
    }

    pub(crate) fn selected_impl(
        &self,
        crate_ref: CrateRef,
        file: FileId,
        span: Span,
    ) -> Option<ImplRef> {
        self.selected_impls
            .iter()
            .find(|selected| {
                selected.impl_ref.origin.origin_crate() == crate_ref
                    && selected.source.file_id == file
                    && selected.source.span == span
            })
            .map(|selected| selected.impl_ref)
    }
}
