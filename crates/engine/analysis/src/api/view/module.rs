//! Generic module facts from the indexed module tree.

use rg_def_map::ModuleOrigin;
use rg_ir_model::ModuleRef;
use rg_parse::FileId;

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
}
