//! Captures and lowers real files discovered by DefMap expansion.
//!
//! DefMap calls this loader between expansion batches. Modules receive a child file context;
//! includes retain the invocation's logical module context. Source capture, contextual ItemTree
//! lowering, path coalescing, and syntax eviction remain together at this project boundary.

use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::Arc,
};

use anyhow::Context as _;
use rg_def_map::{MacroSourceFileRequest, MacroSourceFileResolution};
use rg_ir_model::{FileId, PackageSlot};
use rg_item_tree::ItemTreeDb;
use rg_parse::ParseDb;
use rg_text::PackageNameInterners;

use crate::{
    ProjectMemoryHooks, ProjectMemoryPurgePoint, profile::metric, selection::PhasePackageSet,
};

/// Source access state for one synchronous DefMap build.
///
/// Request identity is coalesced by DefMap. This map coalesces physical paths while still lowering
/// every distinct logical module context, even when requests arrive in different batches.
/// DefMap bounds each invocation's ancestry while allowing unrelated source batches to finish.
pub(super) struct MacroSourceFileLoader<'a> {
    source_packages: &'a PhasePackageSet,
    memory_hooks: &'a dyn ProjectMemoryHooks,
    captured_files_by_path: HashMap<(PackageSlot, PathBuf), FileId>,
}

impl<'a> MacroSourceFileLoader<'a> {
    pub(super) fn new(
        source_packages: &'a PhasePackageSet,
        memory_hooks: &'a dyn ProjectMemoryHooks,
    ) -> Self {
        Self {
            source_packages,
            memory_hooks,
            captured_files_by_path: HashMap::new(),
        }
    }
}

impl rg_def_map::MacroSourceFileLoader for MacroSourceFileLoader<'_> {
    fn load_files(
        &mut self,
        parse: &mut ParseDb,
        item_tree: &mut ItemTreeDb,
        names: &mut PackageNameInterners,
        requests: &[MacroSourceFileRequest],
    ) -> anyhow::Result<Vec<MacroSourceFileResolution>> {
        let _timer = metric::TIMING_MACRO_SOURCE_FILE_LOADING.start_timer();
        metric::MACRO_SOURCE_FILE_DISCOVERY_WAVES.inc();
        metric::MACRO_SOURCE_FILE_REQUESTS.add(requests.len().try_into().unwrap_or(u64::MAX));

        let sources = parse.source_inventory_handle();
        let mut touched_packages = HashSet::new();
        let mut loaded_any_file = false;
        let mut resolutions = Vec::with_capacity(requests.len());

        for request in requests {
            anyhow::ensure!(
                self.source_packages.contains(request.package()),
                "cache-backed package {} emitted a macro source-file request",
                request.package().0,
            );
            metric::MACRO_SOURCE_FILE_UNIQUE_REQUESTS.inc();

            let resolution = match request {
                MacroSourceFileRequest::Module {
                    parent_context,
                    module_name,
                    path_override,
                    ..
                } => parent_context
                    .resolve_module_name(&sources, module_name, path_override.as_deref())
                    .with_context(|| {
                        format!(
                            "while attempting to resolve generated module {module_name} for package {}",
                            request.package().0,
                        )
                    })?
                    .map(|resolution| {
                        let (path, child_context) = resolution.into_parts();
                        let child_context = Arc::new(child_context);
                        (
                            path,
                            Arc::clone(&child_context),
                            MacroSourceFileKind::Module { child_context },
                        )
                    }),
                MacroSourceFileRequest::Include {
                    origin_file,
                    module_file_context,
                    path,
                    ..
                } => {
                    let package = parse
                        .package(request.package().0)
                        .expect("source package should exist while resolving generated include");
                    let current_file = package.parsed_file(*origin_file).with_context(|| {
                        format!("while attempting to fetch generated include origin {origin_file:?}")
                    })?;
                    path.resolve(&current_file, package.cargo_generated_sources())
                        .map(|path| {
                            (
                                path,
                                Arc::clone(module_file_context),
                                MacroSourceFileKind::Include,
                            )
                        })
                }
            };
            let Some((path, lowering_context, file_kind)) = resolution else {
                resolutions.push(MacroSourceFileResolution::Missing);
                metric::MACRO_SOURCE_FILE_MISSING_PATHS.inc();
                continue;
            };

            let package_slot = request.package();
            let requested_path_key = (package_slot, path.clone());
            let previous_file_count = parse
                .package(package_slot.0)
                .expect("source package should exist while loading a macro source file")
                .parsed_files()
                .count();
            let file_id = if let Some(file_id) = self
                .captured_files_by_path
                .get(&requested_path_key)
                .copied()
            {
                metric::MACRO_SOURCE_FILE_COALESCED_PATHS.inc();
                file_id
            } else {
                let file_id = match parse
                    .package_mut(package_slot.0)
                    .expect("source package should exist while capturing a macro source file")
                    .parse_file(&sources, &path)
                {
                    Ok(file_id) => file_id,
                    // Build outputs are optional historical evidence and can disappear after a Cargo
                    // clean. Treat that include as absent instead of making otherwise valid source
                    // analysis fail. Generated module loading keeps its existing hard-error behavior
                    // below once module resolution has selected a source path.
                    Err(_) if matches!(&file_kind, MacroSourceFileKind::Include) => {
                        resolutions.push(MacroSourceFileResolution::Missing);
                        metric::MACRO_SOURCE_FILE_MISSING_PATHS.inc();
                        continue;
                    }
                    Err(error) => {
                        return Err(error).with_context(|| {
                            format!(
                                "while attempting to capture generated module source {}",
                                path.display()
                            )
                        });
                    }
                };
                let canonical_path = parse
                    .package(package_slot.0)
                    .and_then(|package| package.file_path(file_id))
                    .expect("captured macro source file should have a canonical path")
                    .to_path_buf();
                let canonical_path_key = (package_slot, canonical_path);

                // `parse_file` can canonicalize another spelling onto a file captured earlier. Keep
                // both spellings, while treating the canonical identity as one discovered path.
                if let Some(existing_file_id) = self
                    .captured_files_by_path
                    .get(&canonical_path_key)
                    .copied()
                {
                    debug_assert_eq!(existing_file_id, file_id);
                    self.captured_files_by_path
                        .insert(requested_path_key, existing_file_id);
                    metric::MACRO_SOURCE_FILE_COALESCED_PATHS.inc();
                    existing_file_id
                } else {
                    self.captured_files_by_path
                        .insert(requested_path_key, file_id);
                    self.captured_files_by_path
                        .insert(canonical_path_key, file_id);
                    metric::MACRO_SOURCE_FILE_UNIQUE_PATHS.inc();
                    file_id
                }
            };
            touched_packages.insert(package_slot);

            // A shared file can contribute modules from more than one logical context. Its FileTree is
            // reused, but contextual source edges still need to be traversed for this request.
            let lowering = item_tree
                .lower_package_file(
                    parse,
                    package_slot.0,
                    file_id,
                    lowering_context.as_ref().clone(),
                    names,
                )
                .with_context(|| {
                    format!(
                        "while attempting to lower macro source file {}",
                        path.display()
                    )
                })?;
            metric::MACRO_SOURCE_FILE_ITEM_TREE_FILES_LOWERED
                .add(lowering.newly_lowered_files.try_into().unwrap_or(u64::MAX));
            metric::MACRO_SOURCE_FILE_ITEM_TREE_FILES_REUSED
                .add(lowering.reused_files.try_into().unwrap_or(u64::MAX));

            let current_file_count = parse
                .package(package_slot.0)
                .expect("source package should exist after module lowering")
                .parsed_files()
                .count();
            metric::MACRO_SOURCE_FILE_DISCOVERED_FILES.add(
                current_file_count
                    .saturating_sub(previous_file_count)
                    .try_into()
                    .unwrap_or(u64::MAX),
            );
            resolutions.push(match file_kind {
                MacroSourceFileKind::Module { child_context } => {
                    MacroSourceFileResolution::Module {
                        file_id,
                        child_context,
                    }
                }
                MacroSourceFileKind::Include => MacroSourceFileResolution::Include { file_id },
            });
            loaded_any_file = true;
        }

        // A coalesced request may have rehydrated syntax after the first request already lowered and
        // evicted the file. Keep the same package-level eviction boundary as ordinary ItemTree work.
        for package_slot in touched_packages {
            parse
                .package_mut(package_slot.0)
                .expect("touched source package should remain present")
                .evict_syntax_trees();
        }

        if loaded_any_file {
            // Lowering evicts syntax before the allocator cleanup boundary, once per found batch.
            self.memory_hooks
                .purge(ProjectMemoryPurgePoint::AfterItemTreeSyntaxEviction);
        }
        Ok(resolutions)
    }
}

enum MacroSourceFileKind {
    Module {
        child_context: Arc<rg_parse::ModuleFileContext>,
    },
    Include,
}
