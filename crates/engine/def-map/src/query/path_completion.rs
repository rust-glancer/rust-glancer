//! Path completion queries over module scopes.
//!
//! Completion uses the same path resolution and visibility checks as imports. This module keeps
//! those rules inside DefMap and exposes only the visible definitions that analysis can render.

use std::collections::HashSet;

use rg_package_store::PackageStoreError;
use rg_parse::{FileId, Span, TextSpan};

use crate::{
    DefId, DefMap, DefMapReadTxn, ImportSourcePath, ModuleRef, Path, TargetRef,
    query::path_resolution,
};

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

/// Where a visible definition came from during unqualified lookup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisibleScopeOrigin {
    ModuleScope,
    Prelude,
    ExternRoot,
}

/// Namespace slot occupied by a visible module-scope definition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ScopeNamespace {
    Types,
    Values,
    Macros,
}

impl ScopeNamespace {
    fn sort_rank(self) -> u8 {
        match self {
            Self::Types => 0,
            Self::Values => 1,
            Self::Macros => 2,
        }
    }
}

/// One definition visible from a module through another module's scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisibleScopeDef {
    pub label: String,
    pub namespace: ScopeNamespace,
    pub def: DefId,
    /// Lookup source used by unqualified completions to rank familiar names first.
    pub origin: VisibleScopeOrigin,
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

    /// Returns definitions from `source_module` that are visible from `importing_module`.
    pub fn visible_scope_defs(
        &self,
        importing_module: ModuleRef,
        source_module: ModuleRef,
    ) -> Result<Vec<VisibleScopeDef>, PackageStoreError> {
        let scope = path_resolution::visible_module_scope_entry_set_with_env(
            self,
            importing_module,
            source_module,
        )?;
        let mut defs = Vec::new();
        let mut shadowed = HashSet::new();
        push_visible_scope_defs(
            &mut defs,
            &mut shadowed,
            &scope,
            VisibleScopeOrigin::ModuleScope,
            false,
        );
        sort_visible_scope_defs(&mut defs);
        Ok(defs)
    }

    /// Returns names visible from `importing_module` without a qualifier.
    pub fn visible_unqualified_scope_defs(
        &self,
        importing_module: ModuleRef,
    ) -> Result<Vec<VisibleScopeDef>, PackageStoreError> {
        let mut defs = Vec::new();
        let mut shadowed = HashSet::new();

        // First-segment resolution checks the current module scope before extern roots and the
        // standard prelude. Completion follows the same namespace-specific shadowing order.
        let current_scope = path_resolution::visible_module_scope_entry_set_with_env(
            self,
            importing_module,
            importing_module,
        )?;
        push_visible_scope_defs(
            &mut defs,
            &mut shadowed,
            &current_scope,
            VisibleScopeOrigin::ModuleScope,
            false,
        );

        if let Some(def_map) = self.def_map(importing_module.target)? {
            let mut extern_roots = def_map.extern_prelude().iter().collect::<Vec<_>>();
            extern_roots.sort_by(|(left, _), (right, _)| left.cmp(right));
            for (name, module_ref) in extern_roots {
                let label = name.to_string();
                if !shadowed.insert((label.clone(), ScopeNamespace::Types)) {
                    continue;
                }
                defs.push(VisibleScopeDef {
                    label,
                    namespace: ScopeNamespace::Types,
                    def: DefId::Module(*module_ref),
                    origin: VisibleScopeOrigin::ExternRoot,
                });
            }

            if let Some(prelude) = def_map.prelude() {
                let prelude_scope = path_resolution::visible_module_scope_entry_set_with_env(
                    self,
                    importing_module,
                    prelude,
                )?;
                push_visible_scope_defs(
                    &mut defs,
                    &mut shadowed,
                    &prelude_scope,
                    VisibleScopeOrigin::Prelude,
                    true,
                );
            }
        }

        sort_visible_scope_defs(&mut defs);
        Ok(defs)
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
                target: self.target,
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
                target: self.target,
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

fn push_visible_scope_defs(
    defs: &mut Vec<VisibleScopeDef>,
    shadowed: &mut HashSet<(String, ScopeNamespace)>,
    scope: &crate::model::ModuleScopeBuilder,
    origin: VisibleScopeOrigin,
    skip_shadowed: bool,
) {
    // The visibility-aware builder keeps namespace buckets separate. Analysis filters those
    // buckets according to the syntactic context where completion was requested.
    for (name, entry) in scope.entries() {
        for binding in entry.types() {
            push_visible_scope_def(
                defs,
                shadowed,
                name.to_string(),
                ScopeNamespace::Types,
                binding.def,
                origin,
                skip_shadowed,
            );
        }
        for binding in entry.values() {
            push_visible_scope_def(
                defs,
                shadowed,
                name.to_string(),
                ScopeNamespace::Values,
                binding.def,
                origin,
                skip_shadowed,
            );
        }
        for binding in entry.macros() {
            push_visible_scope_def(
                defs,
                shadowed,
                name.to_string(),
                ScopeNamespace::Macros,
                binding.def,
                origin,
                skip_shadowed,
            );
        }
    }
}

fn push_visible_scope_def(
    defs: &mut Vec<VisibleScopeDef>,
    shadowed: &mut HashSet<(String, ScopeNamespace)>,
    label: String,
    namespace: ScopeNamespace,
    def: DefId,
    origin: VisibleScopeOrigin,
    skip_shadowed: bool,
) {
    if skip_shadowed && shadowed.contains(&(label.clone(), namespace)) {
        return;
    }
    shadowed.insert((label.clone(), namespace));
    defs.push(VisibleScopeDef {
        label,
        namespace,
        def,
        origin,
    });
}

fn sort_visible_scope_defs(defs: &mut [VisibleScopeDef]) {
    defs.sort_by(|left, right| {
        left.label
            .cmp(&right.label)
            .then(left.namespace.sort_rank().cmp(&right.namespace.sort_rank()))
            .then(format!("{:?}", left.def).cmp(&format!("{:?}", right.def)))
    });
}
