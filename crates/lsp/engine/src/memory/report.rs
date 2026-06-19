//! Allocator memory reporting helpers.
//!
//! The core memory module defines what the executable can expose. This module owns the logging
//! shape: point-in-time records and human-readable byte formatting.

use super::{MemoryControl, MemoryDelta, MemoryPurge, MemoryStats};

/// Reports allocator memory at explicit retention checkpoints.
pub(crate) struct MemoryReporter;

impl MemoryReporter {
    pub(crate) fn log_checkpoint(
        memory_control: &dyn MemoryControl,
        label: &'static str,
        phase: &'static str,
    ) -> MemoryStats {
        let stats = MemoryStats::capture(memory_control);
        tracing::debug!(
            target: "rg_lsp_engine::memory",
            label,
            phase,
            allocator = memory_control.allocator_name(),
            allocator_purge_enabled = memory_control.allocator_purge_enabled(),
            allocator_purged = false,
            stats = %stats,
            "temporary allocation checkpoint"
        );
        stats
    }

    pub(crate) fn log_checkpoint_delta(
        memory_control: &dyn MemoryControl,
        label: &'static str,
        phase: &'static str,
        before: MemoryStats,
    ) -> MemoryStats {
        let after = MemoryStats::capture(memory_control);
        tracing::debug!(
            target: "rg_lsp_engine::memory",
            label,
            phase,
            allocator = memory_control.allocator_name(),
            allocator_purge_enabled = memory_control.allocator_purge_enabled(),
            allocator_purged = false,
            before = %before,
            after = %after,
            delta = %MemoryDelta::between(before, after),
            "temporary allocation checkpoint delta"
        );
        after
    }

    pub(crate) fn purge_and_report(memory_control: &dyn MemoryControl, label: &'static str) {
        let before_purge = MemoryStats::capture(memory_control);
        let purge = if memory_control.allocator_purge_enabled() {
            MemoryPurge::try_purge(memory_control, before_purge)
        } else {
            None
        };
        let after = match purge {
            Some(purge) => purge.after,
            None => before_purge,
        };

        tracing::info!(
            label,
            allocator = memory_control.allocator_name(),
            allocator_purge_enabled = memory_control.allocator_purge_enabled(),
            allocator_purged = purge.is_some(),
            stats = %after,
            "allocation info"
        );

        if let Some(purge) = purge {
            tracing::info!(
                label,
                purge = %purge,
                "purge stats"
            );
        }
    }

    pub(crate) fn purge_and_report_debug(memory_control: &dyn MemoryControl, label: &'static str) {
        let before_purge = MemoryStats::capture(memory_control);
        let purge = if memory_control.allocator_purge_enabled() {
            MemoryPurge::try_purge(memory_control, before_purge)
        } else {
            None
        };
        let after = match purge {
            Some(purge) => purge.after,
            None => before_purge,
        };

        tracing::debug!(
            label,
            allocator = memory_control.allocator_name(),
            allocator_purge_enabled = memory_control.allocator_purge_enabled(),
            allocator_purged = purge.is_some(),
            stats = %after,
            "allocation info"
        );

        if let Some(purge) = purge {
            tracing::debug!(
                label,
                purge = %purge,
                "purge stats"
            );
        }
    }
}
