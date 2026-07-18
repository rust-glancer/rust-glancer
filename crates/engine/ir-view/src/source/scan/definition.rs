//! Source scanning over the frozen namespace map.
//!
//! DefMap owns module-scope source facts such as local definition names and import path spans. The
//! indexed view interprets those facts as source occurrences without adding cursor APIs to the
//! frozen DefMap transaction.
//!
//! ```text
//! mod model;                       definition: `model`
//! use crate::model::User as Person;
//!     ^^^^^  ^^^^^  ^^^^    ^^^^^ one path-prefix occurrence per segment, plus the alias
//! ```

use rg_def_map::{DefMap, DefMapReadTxn, ModuleOrigin};
use rg_ir_model::Path;
use rg_ir_model::{CrateRef, DefId, DefMapRef, LocalDefId, LocalDefRef, ModuleId, ModuleRef};
use rg_package_store::PackageStoreError;
use rg_parse::{FileId, Span};

/// One module-scope source node that can become an indexed occurrence.
///
/// Imports retain a path prefix for each written segment. Resolving `model` in
/// `crate::model::User` must not accidentally resolve the complete `User` path instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DefinitionSourceCandidate {
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
    ImportAlias {
        module: ModuleRef,
        path: Path,
        file_id: FileId,
        span: Span,
    },
}

/// Scans module declarations, item names, and import path segments owned by DefMap.
///
/// Point mode filters every source span against one cursor offset. Crate mode emits the same
/// candidate vocabulary for all matching files, which keeps go-to-definition and project-wide
/// references on one source interpretation.
pub(crate) struct DefinitionSourceScanner<'txn, 'db> {
    def_map: &'txn DefMapReadTxn<'db>,
    crate_ref: CrateRef,
    file_id: Option<FileId>,
    offset: Option<u32>,
}

impl<'txn, 'db> DefinitionSourceScanner<'txn, 'db> {
    pub(crate) fn at(
        def_map: &'txn DefMapReadTxn<'db>,
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
    ) -> Self {
        Self {
            def_map,
            crate_ref,
            file_id: Some(file_id),
            offset: Some(offset),
        }
    }

    pub(crate) fn in_crate(
        def_map: &'txn DefMapReadTxn<'db>,
        crate_ref: CrateRef,
        file_id: Option<FileId>,
    ) -> Self {
        Self {
            def_map,
            crate_ref,
            file_id,
            offset: None,
        }
    }

    /// Collects declarations first, followed by every written import segment and alias.
    pub(crate) fn scan(&self) -> Result<Vec<DefinitionSourceCandidate>, PackageStoreError> {
        let mut candidates = Vec::new();
        let Some(def_map) = self.def_map.def_map(self.crate_ref)? else {
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
        candidates: &mut Vec<DefinitionSourceCandidate>,
    ) {
        for (module_idx, module) in def_map.modules().iter().enumerate() {
            let module_ref = ModuleRef {
                origin: DefMapRef::Crate(self.crate_ref),
                module: ModuleId(module_idx),
            };
            let declaration_file = match module.origin {
                ModuleOrigin::Root { .. } | ModuleOrigin::Synthetic { .. } => continue,
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
                candidates.push(DefinitionSourceCandidate::Def {
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
        candidates: &mut Vec<DefinitionSourceCandidate>,
    ) {
        for (local_def_idx, local_def) in def_map.local_defs().iter().enumerate() {
            let local_def_ref = LocalDefRef {
                origin: DefMapRef::Crate(self.crate_ref),
                local_def: LocalDefId(local_def_idx),
            };
            if !self.file_matches(local_def.file_id) {
                continue;
            }

            let span = local_def.name_span.unwrap_or(local_def.span);
            if self.offset_matches(span) {
                candidates.push(DefinitionSourceCandidate::Def {
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
        candidates: &mut Vec<DefinitionSourceCandidate>,
    ) {
        for import in def_map.imports() {
            if !self.file_matches(import.source.file_id) {
                continue;
            }

            let module = ModuleRef {
                origin: DefMapRef::Crate(self.crate_ref),
                module: import.module,
            };
            let Some(segments) = import.path.segments_with_spans() else {
                continue;
            };
            for (idx, (_, span)) in segments.enumerate() {
                if self.offset_matches(span) {
                    candidates.push(DefinitionSourceCandidate::UsePath {
                        module,
                        path: Path {
                            absolute: import.path.semantic().absolute,
                            segments: import.path.semantic().segments[..=idx].to_vec(),
                        },
                        file_id: import.source.file_id,
                        span,
                    });
                }
            }

            if let Some(alias_span) = import.alias_span
                && self.offset_matches(alias_span)
            {
                candidates.push(DefinitionSourceCandidate::ImportAlias {
                    module,
                    path: import.path.semantic().clone(),
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
