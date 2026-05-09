//! Allocator memory reporting helpers.
//!
//! The core memory module defines what the executable can expose. This module owns the logging
//! shape: scoped reporters, trace/info records, and human-readable byte formatting.

use std::time::{Duration, Instant};

use super::{MemoryControl, MemoryDelta, MemoryPurge, MemoryStats};

/// Reports allocator memory for scoped operations and point-in-time checkpoints.
pub(crate) struct MemoryReporter;

impl MemoryReporter {
    pub(crate) fn report_op<T>(
        memory_control: &dyn MemoryControl,
        label: &'static str,
        op: impl FnOnce() -> T,
    ) -> T {
        let trace_report =
            tracing::enabled!(target: "rg_lsp_engine::memory", tracing::Level::TRACE);
        let purge_enabled = memory_control.allocator_purge_enabled();
        if !trace_report && !purge_enabled {
            return op();
        }

        let before = if trace_report {
            Some(MemoryStats::capture(memory_control))
        } else {
            None
        };
        let started = Instant::now();
        let result = op();
        let elapsed = started.elapsed();
        let after_operation = MemoryStats::capture(memory_control);
        let purge = if purge_enabled {
            MemoryPurge::try_purge(memory_control, after_operation)
        } else {
            None
        };

        if let Some(before) = before {
            Self::trace_operation(
                memory_control,
                label,
                elapsed,
                before,
                after_operation,
                purge,
            );
        }

        result
    }

    pub(crate) fn report_current(memory_control: &dyn MemoryControl, label: &'static str) {
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

    fn trace_operation(
        memory_control: &dyn MemoryControl,
        label: &'static str,
        elapsed: Duration,
        before: MemoryStats,
        after_operation: MemoryStats,
        purge: Option<MemoryPurge>,
    ) {
        tracing::trace!(
            target: "rg_lsp_engine::memory",
            label,
            elapsed_ms = elapsed.as_millis(),
            allocator = memory_control.allocator_name(),
            allocator_purge_enabled = memory_control.allocator_purge_enabled(),
            allocator_purged = purge.is_some(),
            "memory report"
        );
        tracing::trace!(
            target: "rg_lsp_engine::memory",
            label,
            before = %before,
            "memory before"
        );
        tracing::trace!(
            target: "rg_lsp_engine::memory",
            label,
            after = %after_operation,
            delta = %MemoryDelta::between(before, after_operation),
            "memory after operation"
        );
        if let Some(purge) = purge {
            tracing::trace!(
                target: "rg_lsp_engine::memory",
                label,
                purge = %purge,
                "memory purge"
            );
        }
    }
}
