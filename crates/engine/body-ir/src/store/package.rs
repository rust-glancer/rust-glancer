use wincode::{SchemaRead, SchemaWrite};

use rg_arena::Arena;
use rg_ir_model::BodyId;
use rg_ir_storage::{BodyLocalItems, DefMap, ItemStore};
use rg_memsize::MemorySize;
use rg_parse::TargetId;

use crate::ir::body::ResolvedBodyData;

/// Lowered bodies for all targets inside one parsed package.
#[derive(Debug, Clone, PartialEq, Eq, Default, SchemaRead, SchemaWrite, MemorySize)]
pub struct PackageBodies {
    pub(crate) targets: Arena<TargetId, TargetBodies>,
}

impl PackageBodies {
    pub(crate) fn new(targets: Vec<TargetBodies>) -> Self {
        Self {
            targets: Arena::from_vec(targets),
        }
    }

    pub(crate) fn shrink_to_fit(&mut self) {
        self.targets.shrink_to_fit();
        for target in self.targets.iter_mut() {
            target.shrink_to_fit();
        }
    }

    pub fn targets(&self) -> &[TargetBodies] {
        self.targets.as_slice()
    }

    pub fn target(&self, target: TargetId) -> Option<&TargetBodies> {
        self.targets.get(target)
    }

    pub(crate) fn targets_mut(&mut self) -> &mut [TargetBodies] {
        self.targets.as_mut_slice()
    }
}

/// Resolved bodies for one target.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize)]
pub struct TargetBodies {
    pub(crate) status: TargetBodiesStatus,
    pub(crate) bodies: Arena<BodyId, ResolvedBodyData>,
    pub(crate) body_local_items: Arena<BodyId, BodyLocalItems>,
}

impl TargetBodies {
    pub(crate) fn new() -> Self {
        Self {
            status: TargetBodiesStatus::Built,
            bodies: Arena::new(),
            body_local_items: Arena::new(),
        }
    }

    pub(crate) fn skipped() -> Self {
        Self {
            status: TargetBodiesStatus::Skipped,
            bodies: Arena::new(),
            body_local_items: Arena::new(),
        }
    }

    pub fn status(&self) -> TargetBodiesStatus {
        self.status
    }

    pub fn body(&self, body: BodyId) -> Option<&ResolvedBodyData> {
        self.bodies.get(body)
    }

    pub fn body_local_items(&self, body: BodyId) -> Option<&BodyLocalItems> {
        self.body_local_items.get(body)
    }

    pub fn body_def_map(&self, body: BodyId) -> Option<&DefMap> {
        self.body_local_items(body).map(BodyLocalItems::def_map)
    }

    pub fn body_item_store(&self, body: BodyId) -> Option<&ItemStore> {
        self.body_local_items(body).map(BodyLocalItems::item_store)
    }

    pub fn bodies(&self) -> &[ResolvedBodyData] {
        self.bodies.as_slice()
    }

    pub(crate) fn alloc_body(&mut self, data: ResolvedBodyData) -> BodyId {
        self.bodies.alloc(data)
    }

    pub(crate) fn set_body_local_items(&mut self, items: Vec<BodyLocalItems>) {
        debug_assert_eq!(
            self.bodies.len(),
            items.len(),
            "every built body should have finalized body-local items"
        );
        self.body_local_items = Arena::from_vec(items);
    }

    pub(crate) fn bodies_mut(&mut self) -> &mut [ResolvedBodyData] {
        self.bodies.as_mut_slice()
    }

    fn shrink_to_fit(&mut self) {
        self.bodies.shrink_to_fit();
        for body in self.bodies.iter_mut() {
            body.shrink_to_fit();
        }
        self.body_local_items.shrink_to_fit();
        for items in self.body_local_items.iter_mut() {
            items.shrink_to_fit();
        }
    }
}

/// Whether one target's bodies were eagerly lowered.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, derive_more::Display, SchemaRead, SchemaWrite, MemorySize,
)]
#[memsize(leaf)]
pub enum TargetBodiesStatus {
    #[display("built")]
    Built,
    #[display("skipped")]
    Skipped,
}
