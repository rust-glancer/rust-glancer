use serde::Serialize;
use std::fmt;

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

impl fmt::Display for AllocatorReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "allocator: {}", self.name)?;
        if let Some(stats) = self.stats {
            writeln!(f, "allocator stats: {stats}")?;
        }
        if let Some(purge) = &self.purge {
            write!(f, "allocator purge after build: {purge}")?;
        }

        Ok(())
    }
}

impl fmt::Display for AllocatorStatsReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "allocated {}, active {}, resident {}, mapped {}, retained {}",
            format_bytes(self.allocated_bytes),
            format_bytes(self.active_bytes),
            format_bytes(self.resident_bytes),
            format_bytes(self.mapped_bytes),
            format_bytes(self.retained_bytes),
        )
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
}

impl fmt::Display for AllocatorPurgeReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "tcache_flushed {}, arenas_purged {}",
            self.tcache_flushed, self.arenas_purged,
        )?;

        if let (Some(before), Some(after)) = (self.before, self.after) {
            write!(
                f,
                "\nallocator purge stats: active {} -> {} ({}), resident {} -> {} ({}), mapped {} -> {} ({})",
                format_bytes(before.active_bytes),
                format_bytes(after.active_bytes),
                format_byte_delta(Some(after.active_bytes), Some(before.active_bytes)),
                format_bytes(before.resident_bytes),
                format_bytes(after.resident_bytes),
                format_byte_delta(Some(after.resident_bytes), Some(before.resident_bytes)),
                format_bytes(before.mapped_bytes),
                format_bytes(after.mapped_bytes),
                format_byte_delta(Some(after.mapped_bytes), Some(before.mapped_bytes)),
            )?;
        }

        Ok(())
    }
}

pub(crate) fn format_bytes(bytes: usize) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];

    let mut value = bytes as f64;
    let mut unit = UNITS[0];
    for next_unit in UNITS.iter().skip(1) {
        if value < 1024.0 {
            break;
        }
        value /= 1024.0;
        unit = next_unit;
    }

    if unit == "B" {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {unit}")
    }
}

pub(crate) fn format_byte_delta(after: Option<usize>, before: Option<usize>) -> String {
    let Some(after) = after.and_then(|value| i64::try_from(value).ok()) else {
        return "-".to_string();
    };
    let Some(before) = before.and_then(|value| i64::try_from(value).ok()) else {
        return "-".to_string();
    };
    let delta = after - before;
    let prefix = if delta >= 0 { "+" } else { "-" };
    let Some(bytes) = usize::try_from(delta.unsigned_abs()).ok() else {
        return format!("{delta} B");
    };
    format!("{prefix}{}", format_bytes(bytes))
}
