//! Generic module facts from the indexed module tree.

use rg_def_map::{ModuleOrigin, PackageSlot};
use rg_ir_model::{ModuleRef, TargetRef};
use rg_parse::{FileId, TargetId};

use crate::IndexedViewDb;

pub struct ModuleView<'a, 'db> {
    db: &'a IndexedViewDb<'db>,
}

impl<'a, 'db> ModuleView<'a, 'db> {
    pub fn new(db: &'a IndexedViewDb<'db>) -> Self {
        Self { db }
    }

    pub fn root_file(&self, module_ref: ModuleRef) -> anyhow::Result<Option<FileId>> {
        let Some(target) = module_ref.origin.as_target_ref() else {
            return Ok(None);
        };
        let Some(def_map) = self.db.def_map.def_map(target)? else {
            return Ok(None);
        };
        let Some(module) = def_map.module(module_ref.module) else {
            return Ok(None);
        };
        match module.origin {
            ModuleOrigin::Root { file_id } => Ok(Some(file_id)),
            ModuleOrigin::Inline { .. } | ModuleOrigin::OutOfLine { .. } => Ok(None),
        }
    }

    pub fn targets_containing_file(
        &self,
        package: PackageSlot,
        file: FileId,
    ) -> anyhow::Result<Vec<TargetRef>> {
        let mut targets = Vec::new();
        let def_map_package = self.db.def_map.package(package)?;

        for (target_idx, def_map) in def_map_package.def_maps().iter().enumerate() {
            let target_ref = TargetRef {
                package,
                target: TargetId(target_idx),
            };
            let owns_file = def_map
                .modules()
                .iter()
                .any(|module| module.origin.contains_file(file));
            if owns_file {
                targets.push(target_ref);
            }
        }

        Ok(targets)
    }
}
