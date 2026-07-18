//! Import-path completion source-site scanning.
//!
//! Completion still uses DefMap queries for lookup. This module only finds the source location
//! that should be completed.
//!
//! ```text
//! use crate::mo$0; qualifier `crate`, replace `mo`
//! use crate::$0;   qualifier `crate`, insert into an empty span
//! use cr$0;        unqualified lookup from the importing module
//! ```

use rg_def_map::{DefMap, DefMapReadTxn, ImportPath};
use rg_ir_model::Path;
use rg_ir_model::{CrateRef, DefMapRef, ModuleRef};
use rg_package_store::PackageStoreError;
use rg_parse::{FileId, Span, TextSpan};

use super::NarrowestSourceSite;

/// Source site selected for a qualified import-path completion query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ImportQualifiedPathSite {
    pub module: ModuleRef,
    /// Path before the segment being completed.
    pub qualifier: Path,
    /// Segment prefix already typed after `::`.
    pub member_prefix_span: Span,
}

/// Source site selected for an unqualified import-path completion query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ImportUnqualifiedNameSite {
    pub module: ModuleRef,
    /// Name prefix already typed in the import path.
    pub member_prefix_span: Span,
}

/// Scans import paths owned by DefMap.
///
/// This scanner reports the module containing the `use`, because import lookup starts from that
/// module even when the written path itself is relative or incomplete.
pub(crate) struct ImportPathCompletionSiteScanner<'txn, 'db> {
    def_map: &'txn DefMapReadTxn<'db>,
    crate_ref: CrateRef,
    file_id: FileId,
    offset: u32,
}

impl<'txn, 'db> ImportPathCompletionSiteScanner<'txn, 'db> {
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

    /// Finds a qualified segment such as `mo$0` or the empty segment in `crate::$0`.
    pub(crate) fn qualified_site(
        &self,
    ) -> Result<Option<ImportQualifiedPathSite>, PackageStoreError> {
        let Some(def_map) = self.def_map.def_map(self.crate_ref)? else {
            return Ok(None);
        };
        let mut best = NarrowestSourceSite::new();

        self.scan_import_paths(def_map, &mut best);
        Ok(best.finish())
    }

    /// Finds a single relative import name such as `use cr$0;`.
    pub(crate) fn unqualified_site(
        &self,
    ) -> Result<Option<ImportUnqualifiedNameSite>, PackageStoreError> {
        let Some(def_map) = self.def_map.def_map(self.crate_ref)? else {
            return Ok(None);
        };
        let mut best = NarrowestSourceSite::new();

        for import in def_map.imports() {
            if import.source.file_id != self.file_id {
                continue;
            }
            let module = ModuleRef {
                origin: DefMapRef::Crate(self.crate_ref),
                module: import.module,
            };
            let Some((site, source_len)) =
                self.unqualified_site_for_import_path(module, &import.path)
            else {
                continue;
            };

            best.consider(site, source_len);
        }

        Ok(best.finish())
    }

    fn scan_import_paths(
        &self,
        def_map: &DefMap,
        best: &mut NarrowestSourceSite<ImportQualifiedPathSite>,
    ) {
        for import in def_map.imports() {
            if import.source.file_id != self.file_id {
                continue;
            }
            let module = ModuleRef {
                origin: DefMapRef::Crate(self.crate_ref),
                module: import.module,
            };
            let Some((site, source_len)) = self.site_for_import_path(module, &import.path) else {
                continue;
            };

            best.consider(site, source_len);
        }
    }

    /// Finds either a partially typed path segment or an empty segment after a trailing `::`.
    fn site_for_import_path(
        &self,
        module: ModuleRef,
        path: &ImportPath,
    ) -> Option<(ImportQualifiedPathSite, u32)> {
        let semantic = path.semantic();

        for (idx, (_, span)) in path.segments_with_spans()?.enumerate().skip(1) {
            if !span.touches(self.offset) {
                continue;
            }

            return Some((
                ImportQualifiedPathSite {
                    module,
                    qualifier: Path {
                        absolute: semantic.absolute,
                        segments: semantic.segments[..idx].to_vec(),
                    },
                    member_prefix_span: span,
                },
                path.source_span().unwrap_or(span).len(),
            ));
        }

        let source_span = path.source_span()?;
        let last_segment_span = path.segments_with_spans()?.last()?.1;
        let offset_after_last_segment =
            last_segment_span.text.end <= self.offset && self.offset <= source_span.text.end;
        if source_span.text.end <= last_segment_span.text.end || !offset_after_last_segment {
            return None;
        }

        Some((
            ImportQualifiedPathSite {
                module,
                qualifier: Path {
                    absolute: semantic.absolute,
                    segments: semantic.segments.clone(),
                },
                member_prefix_span: Span {
                    text: TextSpan {
                        start: self.offset,
                        end: self.offset,
                    },
                },
            },
            source_span.len(),
        ))
    }

    /// Finds a partially typed first segment in an import path such as `use st$0;`.
    fn unqualified_site_for_import_path(
        &self,
        module: ModuleRef,
        path: &ImportPath,
    ) -> Option<(ImportUnqualifiedNameSite, u32)> {
        let semantic = path.semantic();
        let mut segments = path.segments_with_spans()?;
        if semantic.absolute || segments.len() != 1 {
            return None;
        }
        let segment_span = segments.next()?.1;
        if !segment_span.touches(self.offset) {
            return None;
        }

        Some((
            ImportUnqualifiedNameSite {
                module,
                member_prefix_span: segment_span,
            },
            path.source_span().unwrap_or(segment_span).len(),
        ))
    }
}
