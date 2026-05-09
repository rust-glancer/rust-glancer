mod report;

pub(crate) use self::report::MemoryReporter;

/// Runtime memory controls supplied by the executable.
///
/// The default implementation is intentionally empty. The binary can provide allocator-specific
/// controls, while tests and non-jemalloc builds keep the server behavior deterministic.
pub trait MemoryControl: std::fmt::Debug + Send + Sync {
    fn allocator_name(&self) -> &'static str {
        "unknown"
    }

    fn allocator_purge_enabled(&self) -> bool {
        false
    }

    fn allocator_stats(&self) -> Option<AllocatorStats> {
        None
    }

    fn try_purge_allocator(&self) -> Option<AllocatorPurgeResult> {
        None
    }
}

impl MemoryControl for () {}

/// Allocator counters collected by the executable that selected the allocator.
///
/// The LSP crate receives these through `MemoryControl`, so it can observe allocator behavior
/// without depending on, or accidentally choosing, a concrete global allocator itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AllocatorStats {
    pub allocated_bytes: usize,
    pub active_bytes: usize,
    pub resident_bytes: usize,
    pub mapped_bytes: usize,
    pub retained_bytes: usize,
}

/// Outcome of one allocator purge attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AllocatorPurgeResult {
    pub tcache_flushed: bool,
    pub arenas_purged: bool,
}

#[derive(Clone, Copy, derive_more::Debug, derive_more::Display)]
#[display("{:?}", self)]
pub(crate) struct MemoryStats {
    #[debug("{}", format_optional_bytes(*allocated))]
    allocated: Option<usize>,
    #[debug("{}", format_optional_bytes(*active))]
    active: Option<usize>,
    #[debug("{}", format_optional_bytes(*resident))]
    resident: Option<usize>,
    #[debug("{}", format_optional_bytes(*mapped))]
    mapped: Option<usize>,
    #[debug("{}", format_optional_bytes(*retained))]
    retained: Option<usize>,
}

impl MemoryStats {
    fn capture(memory_control: &dyn MemoryControl) -> Self {
        let allocator = memory_control.allocator_stats();
        Self {
            allocated: allocator.map(|stats| stats.allocated_bytes),
            active: allocator.map(|stats| stats.active_bytes),
            resident: allocator.map(|stats| stats.resident_bytes),
            mapped: allocator.map(|stats| stats.mapped_bytes),
            retained: allocator.map(|stats| stats.retained_bytes),
        }
    }
}

#[derive(Debug, Clone, Copy, derive_more::Display)]
#[display("{:?}", self)]
pub(crate) struct MemoryPurge {
    tcache_flushed: bool,
    arenas_purged: bool,
    after: MemoryStats,
    delta: MemoryDelta,
}

impl MemoryPurge {
    pub(crate) fn try_purge(
        memory_control: &dyn MemoryControl,
        before: MemoryStats,
    ) -> Option<Self> {
        let result = memory_control.try_purge_allocator()?;
        let after = MemoryStats::capture(memory_control);

        Some(Self {
            tcache_flushed: result.tcache_flushed,
            arenas_purged: result.arenas_purged,
            after,
            delta: MemoryDelta::between(before, after),
        })
    }
}

/// Difference between two allocator snapshots, formatted for memory logs.
#[derive(Clone, Copy, derive_more::Debug, derive_more::Display)]
#[display("{:?}", self)]
pub(crate) struct MemoryDelta {
    #[debug("{}", format_optional_byte_delta(*allocated))]
    allocated: Option<i64>,
    #[debug("{}", format_optional_byte_delta(*active))]
    active: Option<i64>,
    #[debug("{}", format_optional_byte_delta(*resident))]
    resident: Option<i64>,
    #[debug("{}", format_optional_byte_delta(*mapped))]
    mapped: Option<i64>,
    #[debug("{}", format_optional_byte_delta(*retained))]
    retained: Option<i64>,
}

impl MemoryDelta {
    fn between(before: MemoryStats, after: MemoryStats) -> Self {
        Self {
            allocated: byte_delta(after.allocated, before.allocated),
            active: byte_delta(after.active, before.active),
            resident: byte_delta(after.resident, before.resident),
            mapped: byte_delta(after.mapped, before.mapped),
            retained: byte_delta(after.retained, before.retained),
        }
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

// Used by `derive_more::Debug` field formatting above; dead-code analysis does not look inside
// derive expansion.
#[allow(dead_code)]
fn format_optional_bytes(bytes: Option<usize>) -> String {
    bytes.map(format_bytes).unwrap_or_else(|| "-".to_string())
}

fn byte_delta(after: Option<usize>, before: Option<usize>) -> Option<i64> {
    let after = i64::try_from(after?).ok()?;
    let before = i64::try_from(before?).ok()?;
    Some(after - before)
}

// Used by `derive_more::Debug` field formatting above; dead-code analysis does not look inside
// derive expansion.
#[allow(dead_code)]
fn format_optional_byte_delta(delta: Option<i64>) -> String {
    let Some(delta) = delta else {
        return "-".to_string();
    };

    let prefix = if delta >= 0 { "+" } else { "-" };
    let bytes = delta.unsigned_abs();
    let bytes = usize::try_from(bytes).ok().map(format_bytes);
    match bytes {
        Some(bytes) => format!("{prefix}{bytes}"),
        None => format!("{delta} B"),
    }
}
