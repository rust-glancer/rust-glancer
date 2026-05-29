//! Cursor-oriented queries over the frozen namespace map.
//!
//! DefMap owns module-scope source facts such as local definition names and import path spans.
//! Analysis can therefore ask a read transaction for cursor candidates without reaching back into
//! item-tree storage.

use rg_ir_model::{DefId, DefMapRef, LocalDefId, LocalDefRef, ModuleId, ModuleRef, TargetRef};
use rg_parse::{FileId, Span};

use rg_package_store::PackageStoreError;

use crate::{DefMap, DefMapReadTxn, ModuleOrigin, Path};

/// One def-map source node that can participate in cursor queries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DefMapCursorCandidate {
    Def {
        def: DefId,
        file_id: FileId,
        span: Span,
    },
    UsePath {
        module: ModuleRef,
        path: Path,
        file_id: FileId,
        span: Span,
    },
}

impl DefMapReadTxn<'_> {
    /// Returns namespace-level cursor candidates at `offset`.
    pub fn cursor_candidates(
        &self,
        target: TargetRef,
        file_id: FileId,
        offset: u32,
    ) -> Result<Vec<DefMapCursorCandidate>, PackageStoreError> {
        NamespaceCursorScanner {
            def_map: self,
            target,
            file_id: Some(file_id),
            offset: Some(offset),
        }
        .scan()
    }

    /// Returns namespace-level source candidates for one target.
    pub fn source_candidates(
        &self,
        target: TargetRef,
        file_id: Option<FileId>,
    ) -> Result<Vec<DefMapCursorCandidate>, PackageStoreError> {
        NamespaceCursorScanner {
            def_map: self,
            target,
            file_id,
            offset: None,
        }
        .scan()
    }
}

/// Scans module declarations, item names, and import path segments owned by DefMap.
struct NamespaceCursorScanner<'txn, 'db> {
    def_map: &'txn DefMapReadTxn<'db>,
    target: TargetRef,
    file_id: Option<FileId>,
    offset: Option<u32>,
}

impl NamespaceCursorScanner<'_, '_> {
    fn scan(&self) -> Result<Vec<DefMapCursorCandidate>, PackageStoreError> {
        let mut candidates = Vec::new();
        let Some(def_map) = self.def_map.def_map(self.target)? else {
            return Ok(candidates);
        };

        self.push_module_candidates(def_map, &mut candidates);
        self.push_local_def_candidates(def_map, &mut candidates);
        self.push_import_candidates(def_map, &mut candidates);
        Ok(candidates)
    }

    fn push_module_candidates(
        &self,
        def_map: &DefMap,
        candidates: &mut Vec<DefMapCursorCandidate>,
    ) {
        for (module_idx, module) in def_map.modules().iter().enumerate() {
            let module_ref = ModuleRef {
                origin: DefMapRef::Target(self.target),
                module: ModuleId(module_idx),
            };
            let declaration_file = match module.origin {
                ModuleOrigin::Root { .. } => continue,
                ModuleOrigin::Inline {
                    declaration_file, ..
                }
                | ModuleOrigin::OutOfLine {
                    declaration_file, ..
                } => declaration_file,
            };
            if !self.file_matches(declaration_file) {
                continue;
            }

            let Some(span) = module.name_span else {
                continue;
            };
            if self.offset_matches(span) {
                candidates.push(DefMapCursorCandidate::Def {
                    def: DefId::Module(module_ref),
                    file_id: declaration_file,
                    span,
                });
            }
        }
    }

    fn push_local_def_candidates(
        &self,
        def_map: &DefMap,
        candidates: &mut Vec<DefMapCursorCandidate>,
    ) {
        for (local_def_idx, local_def) in def_map.local_defs().iter().enumerate() {
            let local_def_ref = LocalDefRef {
                origin: DefMapRef::Target(self.target),
                local_def: LocalDefId(local_def_idx),
            };
            if !self.file_matches(local_def.file_id) {
                continue;
            }

            let span = local_def.name_span.unwrap_or(local_def.span);
            if self.offset_matches(span) {
                candidates.push(DefMapCursorCandidate::Def {
                    def: DefId::Local(local_def_ref),
                    file_id: local_def.file_id,
                    span,
                });
            }
        }
    }

    fn push_import_candidates(
        &self,
        def_map: &DefMap,
        candidates: &mut Vec<DefMapCursorCandidate>,
    ) {
        for import in def_map.imports() {
            if !self.file_matches(import.source.file_id) {
                continue;
            }

            let module = ModuleRef {
                origin: DefMapRef::Target(self.target),
                module: import.module,
            };
            for (idx, segment) in import.source_path.segments().iter().enumerate() {
                if self.offset_matches(segment.span) {
                    candidates.push(DefMapCursorCandidate::UsePath {
                        module,
                        path: import.source_path.prefix_path(idx),
                        file_id: import.source.file_id,
                        span: segment.span,
                    });
                }
            }

            if let Some(alias_span) = import.alias_span
                && self.offset_matches(alias_span)
            {
                candidates.push(DefMapCursorCandidate::UsePath {
                    module,
                    path: Path::from(&import.path),
                    file_id: import.source.file_id,
                    span: alias_span,
                });
            }
        }
    }

    fn file_matches(&self, file_id: FileId) -> bool {
        self.file_id.is_none_or(|selected| selected == file_id)
    }

    fn offset_matches(&self, span: Span) -> bool {
        self.offset.is_none_or(|offset| span.touches(offset))
    }
}
