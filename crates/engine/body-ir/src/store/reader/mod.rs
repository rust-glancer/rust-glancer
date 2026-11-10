//! Body reads that combine saved owner results with request-local declarations.

mod saved;

use std::sync::Arc;

use rg_def_map::DefMap;
use rg_ir_model::{BodyRef, ConstOrStaticRef, CrateRef, EvaluatedConstValue, FileId, Span};
use rg_semantic_ir::{ItemStore, TargetDeclarationReader};
use rg_source::{QueryContext, ReadError};
use rg_std::UniqueVec;

use self::saved::SavedBodyReader;
use crate::{BodyLocalItems, BodyView, CurrentSourceStore};

pub(crate) trait BodySource: Send + Sync + std::fmt::Debug {
    fn files(&self, krate: CrateRef) -> Result<UniqueVec<FileId>, ReadError>;
    fn bodies(
        &self,
        krate: CrateRef,
        file: Option<FileId>,
        masked: &dyn Fn(FileId) -> bool,
    ) -> Result<Vec<(BodyRef, BodyView<'_>)>, ReadError>;
    fn body(&self, body: BodyRef) -> Result<Option<(BodyView<'_>, &BodyLocalItems)>, ReadError>;
    fn initializer(
        &self,
        item: ConstOrStaticRef,
    ) -> Result<Option<&EvaluatedConstValue>, ReadError>;
}

/// One operation's saved body reads and its optional current-source overlay.
#[derive(Debug, Clone)]
pub struct BodyReader<'db> {
    saved: Arc<dyn BodySource + 'db>,
    current: Arc<CurrentSourceStore>,
}

impl<'db> BodyReader<'db> {
    pub fn new(db: &'db QueryContext<'db>, declarations: &TargetDeclarationReader<'db>) -> Self {
        Self::from_source(Arc::new(SavedBodyReader::new(db, declarations.targets())))
    }

    pub(crate) fn from_source(saved: Arc<dyn BodySource + 'db>) -> Self {
        Self {
            saved,
            current: Arc::new(CurrentSourceStore::default()),
        }
    }

    /// Attach the frozen bodies and declaration contexts prepared for this request.
    pub fn with_current_source(mut self, current: CurrentSourceStore) -> Self {
        self.current = Arc::new(current);
        self
    }

    /// Return whether this body was rebuilt from the request's current source.
    pub fn is_current_body(&self, body_ref: BodyRef) -> bool {
        self.current.contains_body(body_ref)
    }

    pub fn is_current_origin(&self, origin: rg_ir_model::DefMapRef) -> bool {
        self.current.contains_origin(origin)
    }

    pub fn current_signature_origins(
        &self,
        crate_ref: CrateRef,
        file: FileId,
    ) -> impl Iterator<Item = rg_ir_model::DefMapRef> + '_ {
        self.current.signature_origins(crate_ref, file)
    }

    /// Header-only modules borrow saved names; body-owned modules retain lexical parent lookup.
    pub fn signature_lookup_module(
        &self,
        module: rg_ir_model::ModuleRef,
    ) -> Result<rg_ir_model::ModuleRef, ReadError> {
        if let Some(fallback) = self.current.declaration_fallback(module) {
            return Ok(fallback);
        }
        if let rg_ir_model::DefMapRef::Body(body_ref) = module.origin
            && let Some(body) = self.body(body_ref)?
            && module == body.owner_module()
        {
            return Ok(body.fallback_module());
        }
        Ok(module)
    }

    pub fn selected_current_impl(
        &self,
        crate_ref: CrateRef,
        file: FileId,
        span: Span,
    ) -> Option<rg_ir_model::ImplRef> {
        self.current.selected_impl(crate_ref, file, span)
    }

    /// List source files with body owners before reading any body results.
    ///
    /// Whole-crate query work can visit these files separately, checking cancellation before each
    /// read. Current bodies contribute their own files.
    pub fn body_files(&self, crate_ref: CrateRef) -> Result<Vec<FileId>, ReadError> {
        let mut files = self.saved.files(crate_ref)?;
        files.extend(
            self.current
                .bodies()
                .iter()
                .filter(|body| body.body_ref().crate_ref == crate_ref)
                .map(|body| body.view().source().file_id),
        );
        Ok(files.into_vec())
    }

    /// Enumerate bodies from one file, or every body when `file` is absent.
    ///
    /// File-scoped callers acquire only owners in the selected file.
    pub fn bodies(
        &self,
        crate_ref: CrateRef,
        file: Option<FileId>,
    ) -> Result<Vec<(BodyRef, BodyView<'_>)>, ReadError> {
        let mut saved = self.saved.bodies(crate_ref, file, &|file| {
            self.current.masks_file(crate_ref, file)
        })?;
        saved.retain(|(body_ref, body)| {
            !self.current.contains_body(*body_ref)
                && !self.current.masks_file(crate_ref, body.source().file_id)
        });

        // Add rebuilt bodies after removing their saved identities and any saved bodies masked by
        // different editor text. Their expressions and locals come from the current source.
        for current in self.current.bodies().iter().filter(|body| {
            body.body_ref().crate_ref == crate_ref
                && file.is_none_or(|file| body.view().source().file_id == file)
        }) {
            saved.push((current.body_ref(), current.view()));
        }

        Ok(saved)
    }

    /// Select a body by its saved owner and local ID, or by its request-local identity.
    pub fn body(&self, body_ref: BodyRef) -> Result<Option<BodyView<'_>>, ReadError> {
        if let Some(body) = self
            .current
            .bodies()
            .iter()
            .find(|body| body.body_ref() == body_ref)
        {
            return Ok(Some(body.view()));
        }
        Ok(self.saved_body(body_ref)?.map(|(body, _)| body))
    }

    pub fn body_local_items(
        &self,
        body_ref: BodyRef,
    ) -> Result<Option<&BodyLocalItems>, ReadError> {
        if let Some(items) = self.current.items(rg_ir_model::DefMapRef::Body(body_ref)) {
            return Ok(Some(items));
        }
        Ok(self.saved_body(body_ref)?.map(|(_, items)| items))
    }

    fn saved_body(
        &self,
        body_ref: BodyRef,
    ) -> Result<Option<(BodyView<'_>, &BodyLocalItems)>, ReadError> {
        Ok(self.saved.body(body_ref)?.filter(|(body, _)| {
            !self
                .current
                .masks_file(body_ref.crate_ref, body.source().file_id)
        }))
    }

    pub fn saved_initializer_value(
        &self,
        item: ConstOrStaticRef,
    ) -> Result<Option<&EvaluatedConstValue>, ReadError> {
        self.saved.initializer(item)
    }

    /// Return the body-local DefMap without exposing the containing storage object.
    pub fn body_def_map(&self, body_ref: BodyRef) -> Result<Option<&DefMap>, ReadError> {
        Ok(self
            .body_local_items(body_ref)?
            .map(BodyLocalItems::def_map))
    }

    /// Return the body-local item store without exposing the containing storage object.
    pub fn body_item_store(&self, body_ref: BodyRef) -> Result<Option<&ItemStore>, ReadError> {
        Ok(self
            .body_local_items(body_ref)?
            .map(BodyLocalItems::item_store))
    }
}
