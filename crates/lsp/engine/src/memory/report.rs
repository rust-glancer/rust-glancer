//! Allocator memory reporting helpers.
//!
//! The core memory module defines what the executable can expose. This module owns the logging
//! shape: point-in-time records and human-readable byte formatting.

use super::{MemoryControl, MemoryDelta, MemoryStats, format_memory_report_field};

/// Reports allocator memory at explicit retention checkpoints.
pub(crate) struct MemoryReporter;

impl MemoryReporter {
    pub(crate) fn snapshot(memory_control: &dyn MemoryControl) -> MemoryStats {
        MemoryStats::capture(memory_control)
    }

    pub(crate) fn log_delta_debug(
        memory_control: &dyn MemoryControl,
        label: &'static str,
        phase: &'static str,
        before: MemoryStats,
    ) -> MemoryStats {
        let after = MemoryStats::capture(memory_control);
        let delta = MemoryDelta::between(before, after);
        tracing::debug!(
            target: "rg_lsp_engine::memory",
            label,
            phase,
            allocated = %format_memory_report_field(after.allocated, delta.allocated),
            active = %format_memory_report_field(after.active, delta.active),
            resident = %format_memory_report_field(after.resident, delta.resident),
            mapped = %format_memory_report_field(after.mapped, delta.mapped),
            "memory report"
        );
        after
    }

    pub(crate) fn purge_and_report(memory_control: &dyn MemoryControl, label: &'static str) {
        let before = MemoryStats::capture(memory_control);
        let after = Self::purge_after(memory_control, before);
        let delta = MemoryDelta::between(before, after);
        Self::log_report_info(label, after, delta);
    }

    pub(crate) fn purge_and_report_delta_debug(
        memory_control: &dyn MemoryControl,
        label: &'static str,
        before: MemoryStats,
    ) {
        let before_purge = MemoryStats::capture(memory_control);
        let after = Self::purge_after(memory_control, before_purge);
        let delta = MemoryDelta::between(before, after);
        Self::log_report_debug(label, after, delta);
    }

    fn purge_after(memory_control: &dyn MemoryControl, before_purge: MemoryStats) -> MemoryStats {
        if !memory_control.allocator_purge_enabled() {
            return before_purge;
        }

        if memory_control.try_purge_allocator() {
            MemoryStats::capture(memory_control)
        } else {
            before_purge
        }
    }

    fn log_report_debug(label: &'static str, after: MemoryStats, delta: MemoryDelta) {
        tracing::debug!(
            target: "rg_lsp_engine::memory",
            label,
            allocated = %format_memory_report_field(after.allocated, delta.allocated),
            active = %format_memory_report_field(after.active, delta.active),
            resident = %format_memory_report_field(after.resident, delta.resident),
            mapped = %format_memory_report_field(after.mapped, delta.mapped),
            "memory report"
        );
    }

    fn log_report_info(label: &'static str, after: MemoryStats, delta: MemoryDelta) {
        tracing::info!(
            target: "rg_lsp_engine::memory",
            label,
            allocated = %format_memory_report_field(after.allocated, delta.allocated),
            active = %format_memory_report_field(after.active, delta.active),
            resident = %format_memory_report_field(after.resident, delta.resident),
            mapped = %format_memory_report_field(after.mapped, delta.mapped),
            "memory report"
        );
    }
}
