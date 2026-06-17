use serde::Serialize;

use crate::analyze::report::{ReportFieldsBuilder, ReportSectionBuilder, ReportTableBuilder};

#[derive(Debug, Serialize)]
pub(crate) struct AllocatorReport {
    pub(crate) name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) stats: Option<AllocatorStatsReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) purge: Option<AllocatorPurgeReport>,
}

impl AllocatorReport {
    pub(crate) fn capture(
        name: &str,
        stats: Option<rg_lsp_engine::AllocatorStats>,
        purge: Option<AllocatorPurgeReport>,
    ) -> Self {
        Self {
            name: name.to_string(),
            stats: stats.map(AllocatorStatsReport::from),
            purge,
        }
    }

    pub(super) fn append_document(&self, section: &mut ReportSectionBuilder) {
        section.fields("summary", |fields| {
            fields.text("name", &self.name);
        });

        if let Some(stats) = self.stats {
            section.fields("stats", |fields| stats.append_fields(fields));
        }

        if let Some(purge) = &self.purge {
            section.fields("purge", |fields| purge.append_fields(fields));
            if purge.has_stats() {
                section.table("purge_stats", |table| purge.append_stats_table(table));
            }
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize)]
pub(crate) struct AllocatorStatsReport {
    pub(crate) allocated_bytes: usize,
    pub(crate) active_bytes: usize,
    pub(crate) resident_bytes: usize,
    pub(crate) mapped_bytes: usize,
    pub(crate) retained_bytes: usize,
}

impl From<rg_lsp_engine::AllocatorStats> for AllocatorStatsReport {
    fn from(stats: rg_lsp_engine::AllocatorStats) -> Self {
        Self {
            allocated_bytes: stats.allocated_bytes,
            active_bytes: stats.active_bytes,
            resident_bytes: stats.resident_bytes,
            mapped_bytes: stats.mapped_bytes,
            retained_bytes: stats.retained_bytes,
        }
    }
}

impl AllocatorStatsReport {
    fn append_fields(&self, fields: &mut ReportFieldsBuilder) {
        fields
            .bytes_as("allocated_bytes", "allocated", self.allocated_bytes)
            .bytes_as("active_bytes", "active", self.active_bytes)
            .bytes_as("resident_bytes", "resident", self.resident_bytes)
            .bytes_as("mapped_bytes", "mapped", self.mapped_bytes)
            .bytes_as("retained_bytes", "retained", self.retained_bytes);
    }
}

#[derive(Debug, Clone, Copy, Serialize)]
pub(crate) struct AllocatorPurgeReport {
    pub(crate) tcache_flushed: bool,
    pub(crate) arenas_purged: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) before: Option<AllocatorStatsReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) after: Option<AllocatorStatsReport>,
}

impl AllocatorPurgeReport {
    pub(crate) fn purge_memory_and_collect(
        memory_control: &dyn rg_lsp_engine::MemoryControl,
    ) -> Option<Self> {
        let before = memory_control.allocator_stats();
        let result = memory_control.try_purge_allocator()?;
        let after = memory_control.allocator_stats();

        Some(Self {
            tcache_flushed: result.tcache_flushed,
            arenas_purged: result.arenas_purged,
            before: before.map(AllocatorStatsReport::from),
            after: after.map(AllocatorStatsReport::from),
        })
    }

    fn append_fields(&self, fields: &mut ReportFieldsBuilder) {
        fields
            .bool("tcache_flushed", self.tcache_flushed)
            .bool("arenas_purged", self.arenas_purged);
    }

    fn append_stats_table(&self, table: &mut ReportTableBuilder) {
        table
            .text_column("metric")
            .bytes_column("before")
            .bytes_column("after")
            .bytes_column("delta");

        let (Some(before), Some(after)) = (self.before, self.after) else {
            return;
        };

        for (title, before, after) in [
            ("active", before.active_bytes, after.active_bytes),
            ("resident", before.resident_bytes, after.resident_bytes),
            ("mapped", before.mapped_bytes, after.mapped_bytes),
        ] {
            table.row(|row| {
                row.text("metric", title)
                    .bytes("before", before)
                    .bytes("after", after)
                    .byte_delta("delta", after, before);
            });
        }
    }

    fn has_stats(&self) -> bool {
        self.before.is_some() && self.after.is_some()
    }
}
