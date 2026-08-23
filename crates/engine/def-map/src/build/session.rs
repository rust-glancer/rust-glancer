//! Resumable package-scoped DefMap construction.
//!
//! A build collects fresh mutable state only for selected packages. Finalization reads that state
//! for selected packages and falls through to the frozen baseline for everything else. Generated
//! module requests can pause the build while the project grows Parse and ItemTree; completing the
//! session swaps the selected package payloads into a cloned database.

use std::sync::Arc;

use anyhow::Context as _;

use rg_item_tree::ItemTreeDb;
use rg_macro_runtime::MacroExpansionPerformancePreference;
use rg_parse::{FileId, ModuleFileContext};
use rg_text::PackageNameInterners;
use rg_workspace::WorkspaceMetadata;

use super::{
    GeneratedModuleResolution, GeneratedModuleResolutions,
    collect::collect_package_crate_states,
    finalize::{
        FinalizeCrateStates, FinalizeScopeSession, finalize_scopes, freeze_package, select_preludes,
    },
    implicit_roots::build_implicit_roots,
};
use crate::{DefMapBuildProgress, DefMapDb, DefMapReadTxn, GeneratedModuleRequest, PackageSlot};

/// Selected-package construction retained across project-owned source capture waves.
///
/// The session owns every generated-module resolution recorded during the build. This keeps the
/// request ledger tied to the mutable crate states that consume it and prevents a caller from
/// accidentally resuming one session with another session's resolutions.
pub struct DefMapBuildSession {
    baseline: DefMapDb,
    packages: Vec<PackageSlot>,
    crate_states: FinalizeCrateStates,
    scope_session: FinalizeScopeSession,
    generated_module_resolutions: GeneratedModuleResolutions,
    awaiting_generated_modules: Vec<GeneratedModuleRequest>,
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
        interners: &mut PackageNameInterners,
        performance_preference: MacroExpansionPerformancePreference,
    ) -> anyhow::Result<Self> {
        let packages = normalized_package_slots(packages);
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
            crate_states,
            scope_session: FinalizeScopeSession::new(performance_preference),
            generated_module_resolutions: GeneratedModuleResolutions::default(),
            awaiting_generated_modules: Vec::new(),
            complete: false,
        })
    }

    /// Records a requested source after Parse captured it and ItemTree lowered its child context.
    pub fn record_generated_module(
        &mut self,
        request: GeneratedModuleRequest,
        file_id: FileId,
        child_context: Arc<ModuleFileContext>,
    ) -> anyhow::Result<()> {
        self.record_generated_module_resolution(
            request,
            GeneratedModuleResolution::Found {
                file_id,
                child_context,
            },
        )
    }

    /// Records that the project probed every supported path without finding the requested source.
    pub fn record_missing_generated_module(
        &mut self,
        request: GeneratedModuleRequest,
    ) -> anyhow::Result<()> {
        self.record_generated_module_resolution(request, GeneratedModuleResolution::Missing)
    }

    fn record_generated_module_resolution(
        &mut self,
        request: GeneratedModuleRequest,
        resolution: GeneratedModuleResolution,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.awaiting_generated_modules.contains(&request),
            "generated module {} for package {} was not requested by this DefMap build step",
            request.module_name(),
            request.package().0,
        );
        anyhow::ensure!(
            !self.generated_module_resolutions.contains_key(&request),
            "generated module {} for package {} was already resolved",
            request.module_name(),
            request.package().0,
        );
        let previous = self
            .generated_module_resolutions
            .insert(request, resolution);
        debug_assert!(previous.is_none());
        Ok(())
    }

    /// Continue the same mutable crate states after Parse and ItemTree gained requested files.
    ///
    /// Every request from the previous step must be recorded before calling this method. The method
    /// then refreshes the reachable package-file map and resumes the import/macro fixed point. New
    /// requests pause the session again; a request-free result freezes every selected package once.
    pub fn advance(
        &mut self,
        baseline_read: &DefMapReadTxn<'_>,
        parse: &rg_parse::ParseDb,
        item_tree: &ItemTreeDb,
        interners: &mut PackageNameInterners,
    ) -> anyhow::Result<DefMapBuildProgress> {
        anyhow::ensure!(!self.complete, "DefMap build session is already complete");
        if let Some(request) = self
            .awaiting_generated_modules
            .iter()
            .find(|request| !self.generated_module_resolutions.contains_key(request))
        {
            anyhow::bail!(
                "generated module {} for package {} must be resolved before DefMap construction can continue",
                request.module_name(),
                request.package().0,
            );
        }
        self.awaiting_generated_modules.clear();
        self.crate_states.clear_generated_module_requests();
        self.crate_states
            .refresh_known_module_files(parse.packages(), item_tree);

        finalize_scopes(
            Some(baseline_read),
            item_tree,
            &mut self.crate_states,
            interners,
            &mut self.scope_session,
            Some(&self.generated_module_resolutions),
        )
        .context("while attempting to resume generated-module DefMap construction")?;

        let requests = self.crate_states.generated_module_requests();
        if !requests.is_empty() {
            self.awaiting_generated_modules = requests.clone();
            return Ok(DefMapBuildProgress::NeedsGeneratedModules(requests));
        }

        let mut next = self.baseline.clone();
        for package_slot in self.packages.iter().copied() {
            let package_states = self.crate_states.package(package_slot).with_context(|| {
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
            next.mutator()
                .replace_package(package_slot, freeze_package(parse_package, package_states))
                .with_context(|| {
                    format!(
                        "while attempting to replace def-map package {}",
                        package_slot.0
                    )
                })?;
        }
        next.mutator().compact_packages(&self.packages);
        self.complete = true;
        Ok(DefMapBuildProgress::Complete(next))
    }
}

fn normalized_package_slots(packages: &[PackageSlot]) -> Vec<PackageSlot> {
    let mut slots = packages.to_vec();
    slots.sort_by_key(|slot| slot.0);
    slots.dedup();
    slots
}
