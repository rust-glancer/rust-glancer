//! Source ownership for module-scope completion sites.
//!
//! Request-local syntax can identify a macro invocation or `mod name;`, but DefMap owns the
//! semantic module from which names resolve. This scanner joins those two facts without making
//! analysis reconstruct module ancestry from source text.

use rg_def_map::{DefMap, DefMapReadTxn, ModuleFileSelection, ModuleOrigin};
use rg_ir_model::{CrateRef, DefMapRef, ModuleId, ModuleRef};
use rg_package_store::PackageStoreError;
use rg_parse::FileId;

use super::NarrowestSourceSite;

/// The semantic module containing a source offset plus facts needed by `mod` completion.
///
/// `inline_module_path` is the directory descent from the nearest file-backed module to the
/// selected inline module. `declared_children` contains sibling module declarations that should
/// not be offered again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ModuleSourceSite {
    pub(crate) module: ModuleRef,
    pub(crate) file_base: ModuleFileBase,
    pub(crate) inline_module_path: Vec<String>,
    pub(crate) declared_children: Vec<String>,
}

/// File lookup rule recovered from the nearest file-backed semantic module.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ModuleFileBase {
    TargetRoot,
    Conventional,
    PathAttribute,
}

/// Finds the narrowest DefMap module whose written source owns one cursor offset.
///
/// A file can contain nested inline modules, while an out-of-line module uses the whole definition
/// file as its source. The scanner normalizes both forms and then derives filesystem descent from
/// the selected module's ancestry.
pub(crate) struct ModuleSourceSiteScanner<'txn, 'db> {
    def_map: &'txn DefMapReadTxn<'db>,
    crate_ref: CrateRef,
    file_id: FileId,
    offset: u32,
}

impl<'txn, 'db> ModuleSourceSiteScanner<'txn, 'db> {
    pub(crate) fn new(
        def_map: &'txn DefMapReadTxn<'db>,
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
    ) -> Self {
        Self {
            def_map,
            crate_ref,
            file_id,
            offset,
        }
    }

    /// Return source ownership and already-declared child names for the selected module.
    pub(crate) fn site(&self) -> Result<Option<ModuleSourceSite>, PackageStoreError> {
        let Some(def_map) = self.def_map.def_map(self.crate_ref)? else {
            return Ok(None);
        };
        let mut best = NarrowestSourceSite::new();
        for (module_index, module) in def_map.modules().iter().enumerate() {
            let source_len = match module.origin {
                ModuleOrigin::Root { file_id } if file_id == self.file_id => u32::MAX,
                ModuleOrigin::Inline {
                    declaration_file,
                    declaration_span,
                } if declaration_file == self.file_id && declaration_span.touches(self.offset) => {
                    declaration_span.len()
                }
                ModuleOrigin::OutOfLine {
                    definition_file: Some(definition_file),
                    ..
                } if definition_file == self.file_id => u32::MAX,
                ModuleOrigin::Root { .. }
                | ModuleOrigin::Synthetic { .. }
                | ModuleOrigin::Inline { .. }
                | ModuleOrigin::OutOfLine { .. } => continue,
            };
            best.consider(ModuleId(module_index), source_len);
        }
        let Some(module_id) = best.finish() else {
            return Ok(None);
        };

        let Some((file_base, inline_module_path)) =
            Self::file_base_and_inline_path(def_map, module_id, self.file_id)
        else {
            return Ok(None);
        };

        // A declaration being edited is not a duplicate of itself. Other declared children are
        // excluded before filesystem candidates are rendered.
        let declared_children = def_map
            .module(module_id)
            .into_iter()
            .flat_map(|module| &module.children)
            .filter_map(|(name, child)| {
                let child = def_map.module(*child)?;
                let is_current_declaration = match child.origin {
                    ModuleOrigin::Inline {
                        declaration_file,
                        declaration_span,
                    }
                    | ModuleOrigin::OutOfLine {
                        declaration_file,
                        declaration_span,
                        ..
                    } => declaration_file == self.file_id && declaration_span.touches(self.offset),
                    ModuleOrigin::Root { .. } | ModuleOrigin::Synthetic { .. } => false,
                };
                (!is_current_declaration).then(|| name.to_string())
            })
            .collect();

        Ok(Some(ModuleSourceSite {
            module: ModuleRef {
                origin: DefMapRef::Crate(self.crate_ref),
                module: module_id,
            },
            file_base,
            inline_module_path,
            declared_children,
        }))
    }

    /// Follow a current inline-module path through the saved module tree.
    ///
    /// Start from the saved module backed by `file_id`, then follow names such as `outer::inner`.
    /// This still works when edits moved the module away from its saved byte range. Return `None`
    /// if the file has more than one possible root or any path component is missing or ambiguous.
    pub(crate) fn module_for_inline_path(
        def_map_txn: &DefMapReadTxn<'_>,
        crate_ref: CrateRef,
        file_id: FileId,
        inline_module_path: &[String],
    ) -> Result<Option<ModuleRef>, PackageStoreError> {
        def_map_txn.module_for_inline_path(crate_ref, file_id, inline_module_path)
    }

    /// Return module ownership from a path recovered from current syntax rather than a saved
    /// source offset.
    pub(crate) fn site_for_inline_path(
        def_map_txn: &DefMapReadTxn<'_>,
        crate_ref: CrateRef,
        file_id: FileId,
        inline_module_path: &[String],
    ) -> Result<Option<ModuleSourceSite>, PackageStoreError> {
        let Some(module) =
            Self::module_for_inline_path(def_map_txn, crate_ref, file_id, inline_module_path)?
        else {
            return Ok(None);
        };
        let Some(def_map) = def_map_txn.def_map(crate_ref)? else {
            return Ok(None);
        };
        let Some((file_base, semantic_inline_module_path)) =
            Self::file_base_and_inline_path(def_map, module.module, file_id)
        else {
            return Ok(None);
        };
        let declared_children = def_map
            .module(module.module)
            .into_iter()
            .flat_map(|module| &module.children)
            .map(|(name, _)| name.to_string())
            .collect();

        Ok(Some(ModuleSourceSite {
            module,
            file_base,
            inline_module_path: semantic_inline_module_path,
            declared_children,
        }))
    }

    /// Walk from an inline completion site to the semantic module that defines the file base.
    fn file_base_and_inline_path(
        def_map: &DefMap,
        module_id: ModuleId,
        file_id: FileId,
    ) -> Option<(ModuleFileBase, Vec<String>)> {
        let mut inline_module_path = Vec::new();
        let mut ancestor = Some(module_id);
        let file_base = loop {
            let module = def_map.module(ancestor?)?;
            match module.origin {
                ModuleOrigin::Inline {
                    declaration_file, ..
                } if declaration_file == file_id => {
                    inline_module_path.push(module.name.as_ref()?.to_string());
                }
                ModuleOrigin::Root { file_id: root_file } if root_file == file_id => {
                    break ModuleFileBase::TargetRoot;
                }
                ModuleOrigin::OutOfLine {
                    definition_file: Some(definition_file),
                    file_selection,
                    ..
                } if definition_file == file_id => {
                    break match file_selection {
                        ModuleFileSelection::Conventional => ModuleFileBase::Conventional,
                        ModuleFileSelection::PathAttribute => ModuleFileBase::PathAttribute,
                    };
                }
                ModuleOrigin::Root { .. }
                | ModuleOrigin::Synthetic { .. }
                | ModuleOrigin::Inline { .. }
                | ModuleOrigin::OutOfLine { .. } => return None,
            }
            ancestor = module.parent;
        };
        inline_module_path.reverse();
        Some((file_base, inline_module_path))
    }
}
