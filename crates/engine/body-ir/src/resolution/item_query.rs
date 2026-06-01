//! Body-aware routing for semantic-shaped item storage.

use rg_ir_model::{BodyRef, DefMapRef};
use rg_package_store::PackageStoreError;
use rg_semantic_ir::{ItemStore, ItemStoreSource, SemanticIrReadTxn};

use crate::ir::body::BodyData;

#[derive(Clone, Copy)]
pub(super) struct BodyItemStoreSource<'a, 'db> {
    semantic_ir: &'a SemanticIrReadTxn<'db>,
    body_ref: BodyRef,
    body: &'a BodyData,
}

impl<'a, 'db> BodyItemStoreSource<'a, 'db> {
    pub(super) fn new(
        semantic_ir: &'a SemanticIrReadTxn<'db>,
        body_ref: BodyRef,
        body: &'a BodyData,
    ) -> Self {
        Self {
            semantic_ir,
            body_ref,
            body,
        }
    }
}

impl<'a> ItemStoreSource<'a> for BodyItemStoreSource<'a, '_> {
    type Error = PackageStoreError;

    fn item_store_for_origin(
        &self,
        origin: DefMapRef,
    ) -> Result<Option<&'a ItemStore>, Self::Error> {
        match origin {
            DefMapRef::Target(target) => self.semantic_ir.items(target),
            DefMapRef::Body(body_ref) if body_ref == self.body_ref => {
                Ok(self.body.body_item_store())
            }
            DefMapRef::Body(_) => Ok(None),
        }
    }

    fn visible_stores(&self) -> Result<Vec<&'a ItemStore>, Self::Error> {
        self.semantic_ir.included_stores()
    }
}
