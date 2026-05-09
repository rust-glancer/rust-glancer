//! Cursor-oriented queries over the frozen namespace map.
//!
//! DefMap owns module-scope source facts such as local definition names and import path spans.
//! Analysis can therefore ask a read transaction for cursor candidates without reaching back into
//! item-tree storage.

use rg_parse::{FileId, Span};

use rg_package_store::PackageStoreError;

use crate::{DefId, DefMapReadTxn, ModuleOrigin, ModuleRef, Path, TargetRef};

/// One def-map source node that can participate in cursor queries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DefMapCursorCandidate {
    Def {
        def: DefId,
        span: Span,
    },
    UsePath {
        module: ModuleRef,
        path: Path,
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
            file_id,
            offset,
        }
        .scan()
    }
}

/// Scans module declarations, item names, and import path segments owned by DefMap.
struct NamespaceCursorScanner<'txn, 'db> {
    def_map: &'txn DefMapReadTxn<'db>,
    target: TargetRef,
    file_id: FileId,
    offset: u32,
}

impl NamespaceCursorScanner<'_, '_> {
    fn scan(&self) -> Result<Vec<DefMapCursorCandidate>, PackageStoreError> {
        let mut candidates = Vec::new();
        self.push_module_candidates(&mut candidates)?;
        self.push_local_def_candidates(&mut candidates)?;
        self.push_import_candidates(&mut candidates)?;
        Ok(candidates)
    }

    fn push_module_candidates(
        &self,
        candidates: &mut Vec<DefMapCursorCandidate>,
    ) -> Result<(), PackageStoreError> {
        for (module_ref, module) in self.def_map.modules(self.target)? {
            let declaration_file = match module.origin {
                ModuleOrigin::Root { .. } => continue,
                ModuleOrigin::Inline {
                    declaration_file, ..
                }
                | ModuleOrigin::OutOfLine {
                    declaration_file, ..
                } => declaration_file,
            };
            if declaration_file != self.file_id {
                continue;
            }

            let Some(span) = module.name_span else {
                continue;
            };
            if span.touches(self.offset) {
                candidates.push(DefMapCursorCandidate::Def {
                    def: DefId::Module(module_ref),
                    span,
                });
            }
        }

        Ok(())
    }

    fn push_local_def_candidates(
        &self,
        candidates: &mut Vec<DefMapCursorCandidate>,
    ) -> Result<(), PackageStoreError> {
        for (local_def_ref, local_def) in self.def_map.local_defs(self.target)? {
            if local_def.file_id != self.file_id {
                continue;
            }

            let span = local_def.name_span.unwrap_or(local_def.span);
            if span.touches(self.offset) {
                candidates.push(DefMapCursorCandidate::Def {
                    def: DefId::Local(local_def_ref),
                    span,
                });
            }
        }

        Ok(())
    }

    fn push_import_candidates(
        &self,
        candidates: &mut Vec<DefMapCursorCandidate>,
    ) -> Result<(), PackageStoreError> {
        for (_, import) in self.def_map.imports(self.target)? {
            if import.source.file_id != self.file_id {
                continue;
            }

            let module = ModuleRef {
                target: self.target,
                module: import.module,
            };
            for (idx, segment) in import.source_path.segments().iter().enumerate() {
                if segment.span.touches(self.offset) {
                    candidates.push(DefMapCursorCandidate::UsePath {
                        module,
                        path: import.source_path.prefix_path(idx),
                        span: segment.span,
                    });
                }
            }

            if let Some(alias_span) = import.alias_span
                && alias_span.touches(self.offset)
            {
                candidates.push(DefMapCursorCandidate::UsePath {
                    module,
                    path: Path::from(&import.path),
                    span: alias_span,
                });
            }
        }

        Ok(())
    }
}
