//! Resumable package-scoped DefMap construction.
//!
//! A build collects fresh mutable state only for selected packages. Finalization reads that state
//! for selected packages and falls through to the frozen baseline for everything else.
//!
//! The session boundary exists for source edges that are visible only after macro expansion. If an
//! expansion produces `mod generated;`, DefMap must wait for the project to lower the child file.
//! If it produces an `OUT_DIR` `include!`, it must wait for the generated file and then splice its
//! items into the caller's module. In both cases the session keeps collected declarations, import
//! scopes, macro runtime state, and recursion budgets alive while Parse and ItemTree grow outside
//! this crate. Completing the session swaps the selected package payloads into a cloned database.

use std::sync::Arc;

use anyhow::Context as _;

use rg_item_tree::ItemTreeDb;
use rg_macro_runtime::MacroExpansionPerformancePreference;
use rg_parse::{FileId, ModuleFileContext};
use rg_text::PackageNameInterners;
use rg_workspace::WorkspaceMetadata;

use super::{
    MacroSourceFileResolution, MacroSourceFileResolutions,
    collect::collect_package_crate_states,
    finalize::{
        FinalizeCrateStates, FinalizeScopeSession, finalize_scopes, freeze_package, select_preludes,
    },
    implicit_roots::build_implicit_roots,
};
use crate::{
    DefMapBuildOutput, DefMapBuildProgress, DefMapDb, DefMapReadTxn, GeneratedItemStores,
    MacroSourceFileRequest, PackageSlot,
};

/// Selected-package construction retained across project-owned source capture waves.
///
/// The session owns every macro source-file resolution recorded during the build. This keeps the
/// request ledger tied to the mutable crate states that consume it and prevents a caller from
/// accidentally resuming one session with another session's resolutions. A caller repeatedly
/// invokes [`DefMapBuildSession::advance`], answers every returned request on this same value, and
/// advances again until [`DefMapBuildProgress::Complete`] is returned.
///
/// Completion first freezes every selected package. It then copy-compacts the subset supplied when
/// the session starts. Project builds derive that subset from residency, while fixtures pass their
/// complete package set. Keeping the choice on the session matters because a macro source request
/// can pause between collection and the final package replacement.
pub struct DefMapBuildSession {
    baseline: DefMapDb,
    packages: Vec<PackageSlot>,
    copy_compact_packages: Vec<PackageSlot>,
    crate_states: FinalizeCrateStates,
    scope_session: FinalizeScopeSession,
    macro_source_file_resolutions: MacroSourceFileResolutions,
    awaiting_macro_source_files: Vec<MacroSourceFileRequest>,
    complete: bool,
}

impl DefMapBuildSession {
    /// Collects selected package state once and prepares it for resumable finalization.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn start(
        baseline: &DefMapDb,
        baseline_read: &DefMapReadTxn<'_>,
        workspace: &WorkspaceMetadata,
        parse: &rg_parse::ParseDb,
        item_tree: &ItemTreeDb,
        packages: &[PackageSlot],
        copy_compact_packages: &[PackageSlot],
        interners: &mut PackageNameInterners,
        performance_preference: MacroExpansionPerformancePreference,
    ) -> anyhow::Result<Self> {
        let packages = normalized_package_slots(packages);
        let copy_compact_packages = normalized_package_slots(copy_compact_packages)
            .into_iter()
            .filter(|package| packages.binary_search(package).is_ok())
            .collect();
        let implicit_roots = build_implicit_roots(workspace, parse.packages(), interners)
            .context("while attempting to build implicit crate roots")?;
        let mut crate_states = FinalizeCrateStates::empty(parse.packages().len());

        for package_slot in &packages {
            let parse_package = parse.package(package_slot.0).with_context(|| {
                format!(
                    "while attempting to fetch parsed package {}",
                    package_slot.0
                )
            })?;
            let item_tree_package = item_tree.package(package_slot.0).with_context(|| {
                format!(
                    "while attempting to fetch item-tree package {}",
                    package_slot.0
                )
            })?;
            let package_states = collect_package_crate_states(
                package_slot.0,
                parse_package,
                item_tree_package,
                implicit_roots.as_slice(),
            )
            .with_context(|| {
                format!(
                    "while attempting to collect crate states for package {}",
                    parse_package.package_name()
                )
            })?;
            crate_states
                .replace_package(*package_slot, package_states)
                .with_context(|| {
                    format!(
                        "while attempting to replace crate states for package {}",
                        package_slot.0
                    )
                })?;
        }

        select_preludes(
            Some(baseline_read),
            workspace,
            parse.packages(),
            &mut crate_states,
            interners,
        )
        .context("while attempting to select crate preludes")?;

        Ok(Self {
            baseline: baseline.clone(),
            packages,
            copy_compact_packages,
            crate_states,
            scope_session: FinalizeScopeSession::new(performance_preference),
            macro_source_file_resolutions: MacroSourceFileResolutions::default(),
            awaiting_macro_source_files: Vec::new(),
            complete: false,
        })
    }

    /// Records a module file after Parse captured it and ItemTree lowered it in its child context.
    pub fn record_module_file(
        &mut self,
        request: MacroSourceFileRequest,
        file_id: FileId,
        child_context: Arc<ModuleFileContext>,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            matches!(&request, MacroSourceFileRequest::Module { .. }),
            "an include-file request cannot be answered as a module file",
        );
        self.record_macro_source_file_resolution(
            request,
            MacroSourceFileResolution::Module {
                file_id,
                child_context,
            },
        )
    }

    /// Records one file that ItemTree lowered for a macro-generated builtin include.
    pub fn record_include_file(
        &mut self,
        request: MacroSourceFileRequest,
        file_id: FileId,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            matches!(&request, MacroSourceFileRequest::Include { .. }),
            "a module-file request cannot be answered as an include file",
        );
        self.record_macro_source_file_resolution(
            request,
            MacroSourceFileResolution::Include { file_id },
        )
    }

    /// Records that the project completed a supported lookup without finding a source.
    pub fn record_missing_macro_source_file(
        &mut self,
        request: MacroSourceFileRequest,
    ) -> anyhow::Result<()> {
        self.record_macro_source_file_resolution(request, MacroSourceFileResolution::Missing)
    }

    fn record_macro_source_file_resolution(
        &mut self,
        request: MacroSourceFileRequest,
        resolution: MacroSourceFileResolution,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.awaiting_macro_source_files.contains(&request),
            "{} was not requested by this DefMap build step",
            request.description(),
        );
        anyhow::ensure!(
            !self.macro_source_file_resolutions.contains_key(&request),
            "{} was already resolved",
            request.description(),
        );
        let previous = self
            .macro_source_file_resolutions
            .insert(request, resolution);
        debug_assert!(previous.is_none());
        Ok(())
    }

    /// Continues the same mutable crate states after Parse and ItemTree gained requested files.
    ///
    /// Every request from the previous step must be recorded before calling this method. The method
    /// then refreshes the reachable package-file map and resumes the import/macro fixed point at
    /// the exact batch that paused. New requests pause the session again; a request-free result
    /// freezes every selected package once.
    pub fn advance(
        &mut self,
        baseline_read: &DefMapReadTxn<'_>,
        parse: &rg_parse::ParseDb,
        item_tree: &ItemTreeDb,
        interners: &mut PackageNameInterners,
    ) -> anyhow::Result<DefMapBuildProgress> {
        anyhow::ensure!(!self.complete, "DefMap build session is already complete");
        if let Some(request) = self
            .awaiting_macro_source_files
            .iter()
            .find(|request| !self.macro_source_file_resolutions.contains_key(request))
        {
            anyhow::bail!(
                "{} must be resolved before DefMap construction can continue",
                request.description(),
            );
        }
        self.awaiting_macro_source_files.clear();
        self.crate_states.clear_macro_source_file_requests();
        self.crate_states
            .refresh_known_module_files(parse.packages(), item_tree);

        finalize_scopes(
            Some(baseline_read),
            item_tree,
            &mut self.crate_states,
            interners,
            &mut self.scope_session,
            Some(&self.macro_source_file_resolutions),
        )
        .context("while attempting to resume macro source-file DefMap construction")?;

        let requests = self.crate_states.macro_source_file_requests();
        if !requests.is_empty() {
            self.awaiting_macro_source_files = requests.clone();
            return Ok(DefMapBuildProgress::NeedsMacroSourceFiles(requests));
        }

        let mut next = self.baseline.clone();
        let mut generated_items = GeneratedItemStores::default();
        for package_slot in self.packages.iter().copied() {
            let package_states =
                self.crate_states
                    .take_package(package_slot)
                    .with_context(|| {
                        format!(
                            "while attempting to fetch completed crate states for package {}",
                            package_slot.0
                        )
                    })?;
            let parse_package = parse.package(package_slot.0).with_context(|| {
                format!(
                    "while attempting to fetch parsed package {}",
                    package_slot.0
                )
            })?;
            let package = freeze_package(parse_package, package_states, &mut generated_items);
            next.mutator()
                .replace_package(package_slot, package)
                .with_context(|| {
                    format!(
                        "while attempting to replace def-map package {}",
                        package_slot.0
                    )
                })?;
        }
        next.mutator().compact_packages(&self.copy_compact_packages);
        self.complete = true;
        Ok(DefMapBuildProgress::Complete(DefMapBuildOutput::new(
            next,
            generated_items,
        )))
    }
}

fn normalized_package_slots(packages: &[PackageSlot]) -> Vec<PackageSlot> {
    let mut slots = packages.to_vec();
    slots.sort_by_key(|slot| slot.0);
    slots.dedup();
    slots
}
