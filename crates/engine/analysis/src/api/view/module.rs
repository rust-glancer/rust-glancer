//! Generic module facts from the indexed module tree.

use rg_def_map::{ModuleOrigin, PackageSlot};
use rg_ir_model::{ModuleRef, TargetRef};
use rg_parse::{FileId, TargetId};

use crate::api::Analysis;

pub(crate) struct ModuleView<'a, 'db> {
    analysis: &'a Analysis<'db>,
}

impl<'a, 'db> ModuleView<'a, 'db> {
    pub(crate) fn new(analysis: &'a Analysis<'db>) -> Self {
        Self { analysis }
    }

    pub(crate) fn root_file(&self, module_ref: ModuleRef) -> anyhow::Result<Option<FileId>> {
        let Some(module) = self.analysis.def_map.module(module_ref)? else {
            return Ok(None);
        };
        match module.origin {
            ModuleOrigin::Root { file_id } => Ok(Some(file_id)),
            ModuleOrigin::Inline { .. } | ModuleOrigin::OutOfLine { .. } => Ok(None),
        }
    }

    pub(crate) fn targets_containing_file(
        &self,
        package: PackageSlot,
        file: FileId,
    ) -> anyhow::Result<Vec<TargetRef>> {
        let mut targets = Vec::new();
        let def_map_package = self.analysis.def_map.package(package)?;

        for (target_idx, def_map) in def_map_package.into_ref().targets().iter().enumerate() {
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
