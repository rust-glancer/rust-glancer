use rg_def_map::DefMapDb;
use rg_ir_model::{BodyRef, DefMapRef, TargetRef};
use rg_ir_storage::{DefMap, ItemStore};
use rg_parse::ParseDb;
use rg_semantic_ir::{SemanticIrDb, testonly::SemanticIrFixture};
use rg_text::PackageNameInterners;

use crate::{BodyIrBuildPolicy, BodyIrDb, ResolvedBodyData};

/// End-to-end fixture for tests that need body lowering and type propagation data.
pub struct BodyIrFixture {
    semantic_ir: SemanticIrFixture,
    body_ir: BodyIrDb,
}

impl BodyIrFixture {
    pub fn build(fixture: &str) -> Self {
        Self::build_with_policy(fixture, BodyIrBuildPolicy::default())
    }

    pub fn build_with_sysroot(fixture: &str) -> Self {
        Self::build_from_semantic_ir(SemanticIrFixture::build_with_sysroot(fixture))
    }

    pub fn build_with_policy(fixture: &str, policy: BodyIrBuildPolicy) -> Self {
        Self::build_from_semantic_ir_with_policy(SemanticIrFixture::build(fixture), policy)
    }

    pub fn build_from_semantic_ir(semantic_ir: SemanticIrFixture) -> Self {
        Self::build_from_semantic_ir_with_policy(semantic_ir, BodyIrBuildPolicy::default())
    }

    pub fn build_from_semantic_ir_with_policy(
        semantic_ir: SemanticIrFixture,
        policy: BodyIrBuildPolicy,
    ) -> Self {
        let mut names = PackageNameInterners::new(semantic_ir.parse_db().package_count());
        let body_ir = BodyIrDb::builder(
            semantic_ir.parse_db(),
            semantic_ir.def_map_db(),
            semantic_ir.semantic_ir_db(),
        )
        .name_interners(&mut names)
        .policy(policy)
        .build()
        .expect("fixture body ir db should build");

        Self {
            semantic_ir,
            body_ir,
        }
    }

    pub fn parse_db(&self) -> &ParseDb {
        self.semantic_ir.parse_db()
    }

    pub fn def_map_db(&self) -> &DefMapDb {
        self.semantic_ir.def_map_db()
    }

    pub fn semantic_ir_db(&self) -> &SemanticIrDb {
        self.semantic_ir.semantic_ir_db()
    }

    pub fn body_ir_db(&self) -> &BodyIrDb {
        &self.body_ir
    }

    pub fn resident_def_map(&self, target: TargetRef) -> Option<&DefMap> {
        self.semantic_ir.resident_def_map(target)
    }

    pub fn resident_target_ir(&self, target: TargetRef) -> Option<&ItemStore> {
        self.semantic_ir.resident_target_ir(target)
    }

    pub fn resident_body(&self, body_ref: BodyRef) -> Option<&ResolvedBodyData> {
        self.body_ir
            .resident_package(body_ref.target.package)?
            .target(body_ref.target.target)?
            .body(body_ref.body)
    }

    pub fn resident_body_item_store(&self, body_ref: BodyRef) -> Option<&ItemStore> {
        self.body_ir
            .resident_package(body_ref.target.package)?
            .target(body_ref.target.target)?
            .body_item_store(body_ref.body)
    }

    pub fn resident_body_def_map(&self, body_ref: BodyRef) -> Option<&DefMap> {
        self.body_ir
            .resident_package(body_ref.target.package)?
            .target(body_ref.target.target)?
            .body_def_map(body_ref.body)
    }

    pub fn resident_item_store(&self, origin: DefMapRef) -> Option<&ItemStore> {
        match origin {
            DefMapRef::Target(target) => self.resident_target_ir(target),
            DefMapRef::Body(body_ref) => self.resident_body_item_store(body_ref),
        }
    }
}
