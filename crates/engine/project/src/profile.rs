use rg_profile::{
    MemorySnapshotMetric, ProfileCheckpointColumn, ProfileCheckpointValue, ProfileDescriptor,
    ProfileMeasurement, ProfileMemoryRecord, ProfileMemorySnapshot, declare_metrics,
};
use rg_std::{MemoryRecord, MemoryRecorder, MemorySize};

pub const BUILD_PROFILE_SCOPE: &str = "project.build";
pub const BUILD_CHECKPOINTS_PROFILE_PATH: &str = "project.build.checkpoints";

static BUILD_CHECKPOINT_COLUMNS: &[ProfileCheckpointColumn] = &[
    ProfileCheckpointColumn::bytes("retained_bytes", "rg_sampled"),
    ProfileCheckpointColumn::bytes("active_retained_bytes", "rg_total"),
    ProfileCheckpointColumn::bytes("allocated_bytes", "j_allocated"),
    ProfileCheckpointColumn::bytes("active_bytes", "j_active"),
    ProfileCheckpointColumn::bytes("resident_bytes", "j_resident"),
    ProfileCheckpointColumn::bytes("mapped_bytes", "j_mapped"),
];

declare_metrics! {
    pub(crate) mod metric {
        scope "project.build" {
            checkpoint CHECKPOINTS = "checkpoints" [columns super::BUILD_CHECKPOINT_COLUMNS];
        }

        scope "project.build.parse" {
            memory_snapshot PARSE_MEMORY = "memory" [title "after parse"];
        }

        scope "project.build.cache_probe" {
            memory_snapshot CACHE_PROBE_MEMORY = "memory" [title "after cache probe"];
        }

        scope "project.build.item_tree" {
            memory_snapshot ITEM_TREE_MEMORY = "memory" [title "after item-tree"];
        }

        scope "project.build.item_tree.syntax_eviction" {
            memory_snapshot ITEM_TREE_SYNTAX_EVICTION_MEMORY = "memory" [title "after item-tree syntax eviction"];
        }

        scope "project.build.cache_source_fingerprints" {
            memory_snapshot CACHE_SOURCE_FINGERPRINTS_MEMORY = "memory" [title "after cache source fingerprints"];
        }

        scope "project.build.def_map" {
            memory_snapshot DEF_MAP_MEMORY = "memory" [title "after def-map"];
        }

        scope "project.build.semantic_ir" {
            memory_snapshot SEMANTIC_IR_MEMORY = "memory" [title "after semantic-ir"];
        }

        scope "project.build.item_tree.drop" {
            memory_snapshot ITEM_TREE_DROP_MEMORY = "memory" [title "after item-tree drop"];
        }

        scope "project.build.body_ir" {
            memory_snapshot BODY_IR_MEMORY = "memory" [title "after body-ir"];
        }

        scope "project.build.parse.syntax_eviction" {
            memory_snapshot PARSE_SYNTAX_EVICTION_MEMORY = "memory" [title "after parse syntax eviction"];
        }

        scope "project.build.cache_probe" {
            counter CACHE_PROBE_PACKAGES = "packages.total";
            counter CACHE_PROBE_RESIDENT_PACKAGES = "packages.resident";
            counter CACHE_PROBE_OFFLOADABLE_PACKAGES = "packages.offloadable";
            counter CACHE_PROBE_HITS = "results.hits";
            counter CACHE_PROBE_MISSING_ARTIFACTS = "misses.missing_artifact";
            counter CACHE_PROBE_ARTIFACT_READ_ERRORS = "misses.artifact_read_error";
            counter CACHE_PROBE_SOURCE_MISMATCHES = "misses.source_mismatch";
            counter CACHE_PROBE_SOURCE_ERRORS = "misses.source_error";
            counter CACHE_PROBE_BODY_IR_POLICY_MISMATCHES = "misses.body_ir_policy_mismatch";
            counter CACHE_PROBE_PARSE_RESTORE_ERRORS = "misses.parse_restore_error";
            counter CACHE_PROBE_UNPLANNED_PACKAGES = "misses.unplanned_package";

            duration CACHE_PROBE_ARTIFACT_READ = "timings.artifact_read";
            duration CACHE_PROBE_SOURCE_FINGERPRINT = "timings.source_fingerprint";
            duration CACHE_PROBE_PARSE_RESTORE = "timings.parse_restore";
        }
    }
}

pub(crate) fn profile_descriptors() -> &'static [ProfileDescriptor] {
    metric::descriptors()
}

/// Process allocator counters sampled by the executable during a profiled build.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuildProcessMemory {
    pub allocated_bytes: usize,
    pub active_bytes: usize,
    pub resident_bytes: usize,
    pub mapped_bytes: usize,
}

pub type ProcessMemorySampler = Box<dyn FnMut() -> Option<BuildProcessMemory>>;

pub(crate) struct BuildProfiler {
    retained_memory: bool,
    process_memory_sampler: Option<ProcessMemorySampler>,
}

impl BuildProfiler {
    pub(crate) fn disabled() -> Self {
        Self {
            retained_memory: false,
            process_memory_sampler: None,
        }
    }

    pub(crate) fn new(
        retained_memory: bool,
        process_memory_sampler: Option<ProcessMemorySampler>,
    ) -> Self {
        Self {
            retained_memory,
            process_memory_sampler,
        }
    }

    pub(crate) fn measure<T>(&self, value: &T) -> Option<usize>
    where
        T: MemorySize,
    {
        self.retained_memory.then(|| value.memory_size())
    }

    pub(crate) fn sum_retained(&self, values: &[Option<usize>]) -> Option<usize> {
        self.retained_memory
            .then(|| values.iter().flatten().copied().sum())
    }

    pub(crate) fn sample_process_memory(&mut self) -> Option<BuildProcessMemory> {
        self.process_memory_sampler
            .as_mut()
            .and_then(|sampler| sampler())
    }

    pub(crate) fn capture_memory_snapshot(
        &mut self,
        metric: MemorySnapshotMetric,
        capture: impl FnOnce(&mut MemoryRecorder),
    ) {
        if !metric.is_enabled() {
            return;
        }

        let mut recorder = MemoryRecorder::new("build");
        capture(&mut recorder);
        let records = recorder
            .records()
            .into_iter()
            .map(Self::profile_memory_record)
            .collect();
        metric.record(ProfileMemorySnapshot::new(recorder.total_bytes(), records));
    }

    fn profile_memory_record(record: MemoryRecord) -> ProfileMemoryRecord {
        ProfileMemoryRecord::new(
            record.path,
            record.type_name,
            record.kind.as_str(),
            record.bytes,
        )
    }

    pub(crate) fn record_memory_value<T>(
        recorder: &mut MemoryRecorder,
        label: &'static str,
        value: Option<&T>,
    ) where
        T: MemorySize,
    {
        if let Some(value) = value {
            recorder.scope(label, |recorder| value.record_memory_size(recorder));
        }
    }

    pub(crate) fn record(
        &mut self,
        label: &'static str,
        retained_bytes: Option<usize>,
        active_retained_bytes: Option<usize>,
        process_memory: Option<BuildProcessMemory>,
    ) {
        Self::record_dynamic_checkpoint(
            label,
            retained_bytes,
            active_retained_bytes,
            process_memory,
        );
    }

    fn record_dynamic_checkpoint(
        label: &'static str,
        retained_bytes: Option<usize>,
        active_retained_bytes: Option<usize>,
        process_memory: Option<BuildProcessMemory>,
    ) {
        metric::CHECKPOINTS.record(
            label,
            vec![
                ProfileCheckpointValue::new(
                    "retained_bytes",
                    ProfileMeasurement::optional_bytes(retained_bytes),
                ),
                ProfileCheckpointValue::new(
                    "active_retained_bytes",
                    ProfileMeasurement::optional_bytes(active_retained_bytes),
                ),
                ProfileCheckpointValue::new(
                    "allocated_bytes",
                    ProfileMeasurement::optional_bytes(
                        process_memory.map(|memory| memory.allocated_bytes),
                    ),
                ),
                ProfileCheckpointValue::new(
                    "active_bytes",
                    ProfileMeasurement::optional_bytes(
                        process_memory.map(|memory| memory.active_bytes),
                    ),
                ),
                ProfileCheckpointValue::new(
                    "resident_bytes",
                    ProfileMeasurement::optional_bytes(
                        process_memory.map(|memory| memory.resident_bytes),
                    ),
                ),
                ProfileCheckpointValue::new(
                    "mapped_bytes",
                    ProfileMeasurement::optional_bytes(
                        process_memory.map(|memory| memory.mapped_bytes),
                    ),
                ),
            ],
        );
    }
}
