use rg_profile::{
    ProfileCheckpointColumn, ProfileCheckpointValue, ProfileDescriptor, ProfileMeasurement,
    declare_metrics,
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

/// Build checkpoint where callers can request a detailed retained-memory breakdown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildProfileStage {
    Parse,
    CacheProbe,
    ItemTree,
    ItemTreeSyntaxEviction,
    CacheSourceFingerprints,
    DefMap,
    SemanticIr,
    ItemTreeDrop,
    BodyIr,
    ParseSyntaxEviction,
}

impl BuildProfileStage {
    pub fn label(self) -> &'static str {
        match self {
            Self::Parse => "after parse",
            Self::CacheProbe => "after cache probe",
            Self::ItemTree => "after item-tree",
            Self::ItemTreeSyntaxEviction => "after item-tree syntax eviction",
            Self::CacheSourceFingerprints => "after cache source fingerprints",
            Self::DefMap => "after def-map",
            Self::SemanticIr => "after semantic-ir",
            Self::ItemTreeDrop => "after item-tree drop",
            Self::BodyIr => "after body-ir",
            Self::ParseSyntaxEviction => "after parse syntax eviction",
        }
    }
}

/// Detailed memory accounting captured while a transient build stage is still alive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildStageMemorySnapshot {
    stage: BuildProfileStage,
    retained_bytes: usize,
    records: Vec<MemoryRecord>,
}

impl BuildStageMemorySnapshot {
    pub fn stage(&self) -> BuildProfileStage {
        self.stage
    }

    pub fn label(&self) -> &'static str {
        self.stage.label()
    }

    pub fn retained_bytes(&self) -> usize {
        self.retained_bytes
    }

    pub fn records(&self) -> &[MemoryRecord] {
        &self.records
    }
}

pub(crate) struct BuildProfiler {
    retained_memory: bool,
    process_memory_sampler: Option<ProcessMemorySampler>,
    stage_memory_target: Option<BuildProfileStage>,
    stage_memory: Option<BuildStageMemorySnapshot>,
}

impl BuildProfiler {
    pub(crate) fn disabled() -> Self {
        Self {
            retained_memory: false,
            process_memory_sampler: None,
            stage_memory_target: None,
            stage_memory: None,
        }
    }

    pub(crate) fn new(
        retained_memory: bool,
        process_memory_sampler: Option<ProcessMemorySampler>,
        stage_memory_target: Option<BuildProfileStage>,
    ) -> Self {
        Self {
            retained_memory,
            process_memory_sampler,
            stage_memory_target,
            stage_memory: None,
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

    pub(crate) fn capture_stage_memory(
        &mut self,
        stage: BuildProfileStage,
        capture: impl FnOnce(&mut MemoryRecorder),
    ) {
        if self.stage_memory_target != Some(stage) || self.stage_memory.is_some() {
            return;
        }

        let mut recorder = MemoryRecorder::new("stage");
        capture(&mut recorder);
        self.stage_memory = Some(BuildStageMemorySnapshot {
            stage,
            retained_bytes: recorder.total_bytes(),
            records: recorder.records(),
        });
    }

    pub(crate) fn record_stage_value<T>(
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

    pub(crate) fn finish(self) -> Option<BuildStageMemorySnapshot> {
        self.stage_memory
    }
}
