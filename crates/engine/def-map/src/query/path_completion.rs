//! Path completion source-site scanners.
//!
//! Completion still uses DefMap queries for lookup. This module only finds the source location
//! that should be completed.

use rg_ir_model::{DefMapRef, ModuleRef, TargetRef};
use rg_ir_storage::{DefMap, ImportSourcePath, Path};
use rg_package_store::PackageStoreError;
use rg_parse::{FileId, Span, TextSpan};

use crate::DefMapReadTxn;

/// Source site selected for a qualified import-path completion query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefMapPathCompletionSite {
    pub module: ModuleRef,
    /// Path before the segment being completed.
    pub qualifier: Path,
    /// Segment prefix already typed after `::`.
    pub member_prefix_span: Span,
}

/// Source site selected for an unqualified import-path completion query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefMapUnqualifiedCompletionSite {
    pub module: ModuleRef,
    /// Name prefix already typed in the import path.
    pub member_prefix_span: Span,
}

impl DefMapReadTxn<'_> {
    /// Returns the source site for a qualified import-path completion query.
    pub fn path_completion_site(
        &self,
        target: TargetRef,
        file_id: FileId,
        offset: u32,
    ) -> Result<Option<DefMapPathCompletionSite>, PackageStoreError> {
        PathCompletionSiteScanner {
            def_map: self,
            target,
            file_id,
            offset,
        }
        .scan()
    }

    /// Returns the source site for an unqualified import-path completion query.
    pub fn unqualified_completion_site(
        &self,
        target: TargetRef,
        file_id: FileId,
        offset: u32,
    ) -> Result<Option<DefMapUnqualifiedCompletionSite>, PackageStoreError> {
        PathCompletionSiteScanner {
            def_map: self,
            target,
            file_id,
            offset,
        }
        .scan_unqualified()
    }
}

/// Scans import paths owned by DefMap.
struct PathCompletionSiteScanner<'txn, 'db> {
    def_map: &'txn DefMapReadTxn<'db>,
    target: TargetRef,
    file_id: FileId,
    offset: u32,
}

impl PathCompletionSiteScanner<'_, '_> {
    fn scan(&self) -> Result<Option<DefMapPathCompletionSite>, PackageStoreError> {
        let Some(def_map) = self.def_map.def_map(self.target)? else {
            return Ok(None);
        };
        let mut best: Option<(DefMapPathCompletionSite, u32)> = None;

        self.scan_import_paths(def_map, &mut best);
        Ok(best.map(|(site, _)| site))
    }

    fn scan_unqualified(
        &self,
    ) -> Result<Option<DefMapUnqualifiedCompletionSite>, PackageStoreError> {
        let Some(def_map) = self.def_map.def_map(self.target)? else {
            return Ok(None);
        };
        let mut best: Option<(DefMapUnqualifiedCompletionSite, u32)> = None;

        for import in def_map.imports() {
            if import.source.file_id != self.file_id {
                continue;
            }
            let module = ModuleRef {
                origin: DefMapRef::Target(self.target),
                module: import.module,
            };
            let Some((site, source_len)) =
                self.unqualified_site_for_import_path(module, &import.source_path)
            else {
                continue;
            };

            if best
                .as_ref()
                .is_none_or(|(_, best_len)| source_len < *best_len)
            {
                best = Some((site, source_len));
            }
        }

        Ok(best.map(|(site, _)| site))
    }

    fn scan_import_paths(
        &self,
        def_map: &DefMap,
        best: &mut Option<(DefMapPathCompletionSite, u32)>,
    ) {
        for import in def_map.imports() {
            if import.source.file_id != self.file_id {
                continue;
            }
            let module = ModuleRef {
                origin: DefMapRef::Target(self.target),
                module: import.module,
            };
            let Some((site, source_len)) = self.site_for_import_path(module, &import.source_path)
            else {
                continue;
            };

            if best
                .as_ref()
                .is_none_or(|(_, best_len)| source_len < *best_len)
            {
                *best = Some((site, source_len));
            }
        }
    }

    /// Finds either a partially typed path segment or an empty segment after a trailing `::`.
    fn site_for_import_path(
        &self,
        module: ModuleRef,
        path: &ImportSourcePath,
    ) -> Option<(DefMapPathCompletionSite, u32)> {
        for (idx, segment) in path.segments.iter().enumerate().skip(1) {
            if !segment.span.touches(self.offset) {
                continue;
            }

            return Some((
                DefMapPathCompletionSite {
                    module,
                    qualifier: Path {
                        absolute: path.absolute,
                        segments: path
                            .segments
                            .iter()
                            .take(idx)
                            .map(|segment| segment.segment.clone())
                            .collect(),
                    },
                    member_prefix_span: segment.span,
                },
                path.source_span.unwrap_or(segment.span).len(),
            ));
        }

        let source_span = path.source_span()?;
        let last_segment = path.segments.last()?;
        let offset_after_last_segment =
            last_segment.span.text.end <= self.offset && self.offset <= source_span.text.end;
        if source_span.text.end <= last_segment.span.text.end || !offset_after_last_segment {
            return None;
        }

        Some((
            DefMapPathCompletionSite {
                module,
                qualifier: Path {
                    absolute: path.absolute,
                    segments: path
                        .segments
                        .iter()
                        .map(|segment| segment.segment.clone())
                        .collect(),
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
        path: &ImportSourcePath,
    ) -> Option<(DefMapUnqualifiedCompletionSite, u32)> {
        if path.absolute || path.segments.len() != 1 {
            return None;
        }
        let segment = path.segments.first()?;
        if !segment.span.touches(self.offset) {
            return None;
        }

        Some((
            DefMapUnqualifiedCompletionSite {
                module,
                member_prefix_span: segment.span,
            },
            path.source_span.unwrap_or(segment.span).len(),
        ))
    }
}
