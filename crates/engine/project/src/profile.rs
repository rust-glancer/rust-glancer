use std::time::{Duration, Instant};

use rg_memsize::{MemoryRecord, MemoryRecorder, MemorySize};

/// Build-time memory and timing report for the project pipeline.
///
/// This is intentionally a facts-only API: callers can inspect coarse checkpoints without
/// receiving references to transient phase databases such as ItemTree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildProfile {
    checkpoints: Vec<BuildCheckpoint>,
    cache_probe: Option<CacheProbeProfile>,
    stage_memory: Option<BuildStageMemorySnapshot>,
}

impl BuildProfile {
    pub(crate) fn new(
        checkpoints: Vec<BuildCheckpoint>,
        cache_probe: Option<CacheProbeProfile>,
        stage_memory: Option<BuildStageMemorySnapshot>,
    ) -> Self {
        Self {
            checkpoints,
            cache_probe,
            stage_memory,
        }
    }

    pub fn checkpoints(&self) -> &[BuildCheckpoint] {
        &self.checkpoints
    }

    pub fn cache_probe(&self) -> Option<&CacheProbeProfile> {
        self.cache_probe.as_ref()
    }

    pub fn stage_memory(&self) -> Option<&BuildStageMemorySnapshot> {
        self.stage_memory.as_ref()
    }
}

/// One profiling sample collected while the project pipeline is building.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildCheckpoint {
    pub label: &'static str,
    /// Time spent since the previous checkpoint, or since build start for the first checkpoint.
    pub phase_elapsed: Duration,
    /// Time spent since build start.
    pub elapsed: Duration,
    /// Retained size of the object sampled at this checkpoint.
    pub retained_bytes: Option<usize>,
    /// Retained size of all live phase state known at this checkpoint.
    pub active_retained_bytes: Option<usize>,
    /// Runtime heap bytes allocated through the process allocator, if available.
    pub allocated_bytes: Option<usize>,
    /// Runtime heap bytes held in active allocator pages, if available.
    pub active_bytes: Option<usize>,
    /// Runtime resident memory reported by the executable, if available.
    pub resident_bytes: Option<usize>,
}

/// Process allocator counters sampled by the executable during a profiled build.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuildProcessMemory {
    pub allocated_bytes: usize,
    pub active_bytes: usize,
    pub resident_bytes: usize,
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

/// Startup-cache probe summary collected while selecting packages to rebuild from source.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CacheProbeProfile {
    pub package_count: usize,
    pub resident_count: usize,
    pub offloadable_count: usize,
    pub hit_count: usize,
    pub missing_artifact_count: usize,
    pub artifact_read_error_count: usize,
    pub source_mismatch_count: usize,
    pub source_error_count: usize,
    pub body_ir_policy_mismatch_count: usize,
    pub restore_error_count: usize,
    pub unplanned_package_count: usize,
    pub artifact_read_elapsed: Duration,
    pub source_fingerprint_elapsed: Duration,
    pub parse_restore_elapsed: Duration,
}

impl CacheProbeProfile {
    pub fn miss_count(&self) -> usize {
        self.missing_artifact_count
            + self.artifact_read_error_count
            + self.source_mismatch_count
            + self.source_error_count
            + self.body_ir_policy_mismatch_count
            + self.restore_error_count
            + self.unplanned_package_count
    }
}

/// Keeps cache-probe counters out of the build pipeline's control flow.
///
/// The cache probe should read as hit/miss policy; this wrapper owns the lower-level details of
/// which public profile field corresponds to each outcome.
pub(crate) struct CacheProbeRecorder {
    profile: CacheProbeProfile,
}

impl CacheProbeRecorder {
    pub(crate) fn new(package_count: usize) -> Self {
        Self {
            profile: CacheProbeProfile {
                package_count,
                ..CacheProbeProfile::default()
            },
        }
    }

    pub(crate) fn resident_package(&mut self) {
        self.profile.resident_count += 1;
    }

    pub(crate) fn offloadable_package(&mut self) {
        self.profile.offloadable_count += 1;
    }

    pub(crate) fn hit(&mut self) {
        self.profile.hit_count += 1;
    }

    pub(crate) fn missing_artifact(&mut self) {
        self.profile.missing_artifact_count += 1;
    }

    pub(crate) fn artifact_read_error(&mut self) {
        self.profile.artifact_read_error_count += 1;
    }

    pub(crate) fn source_mismatch(&mut self) {
        self.profile.source_mismatch_count += 1;
    }

    pub(crate) fn source_error(&mut self) {
        self.profile.source_error_count += 1;
    }

    pub(crate) fn body_ir_policy_mismatch(&mut self) {
        self.profile.body_ir_policy_mismatch_count += 1;
    }

    pub(crate) fn restore_error(&mut self) {
        self.profile.restore_error_count += 1;
    }

    pub(crate) fn unplanned_package(&mut self) {
        self.profile.unplanned_package_count += 1;
    }

    pub(crate) fn time_artifact_read<T>(&mut self, action: impl FnOnce() -> T) -> T {
        Self::time(&mut self.profile.artifact_read_elapsed, action)
    }

    pub(crate) fn time_source_fingerprint<T>(&mut self, action: impl FnOnce() -> T) -> T {
        Self::time(&mut self.profile.source_fingerprint_elapsed, action)
    }

    pub(crate) fn time_parse_restore<T>(&mut self, action: impl FnOnce() -> T) -> T {
        Self::time(&mut self.profile.parse_restore_elapsed, action)
    }

    pub(crate) fn finish(self) -> Option<CacheProbeProfile> {
        (self.profile.offloadable_count > 0).then_some(self.profile)
    }

    fn time<T>(elapsed: &mut Duration, action: impl FnOnce() -> T) -> T {
        let started = Instant::now();
        let result = action();
        *elapsed += started.elapsed();
        result
    }
}

pub(crate) struct BuildProfiler {
    started_at: Instant,
    timing: bool,
    retained_memory: bool,
    process_memory_sampler: Option<ProcessMemorySampler>,
    stage_memory_target: Option<BuildProfileStage>,
    stage_memory: Option<BuildStageMemorySnapshot>,
    checkpoints: Vec<BuildCheckpoint>,
    cache_probe: Option<CacheProbeProfile>,
}

impl BuildProfiler {
    pub(crate) fn disabled() -> Self {
        Self {
            started_at: Instant::now(),
            timing: false,
            retained_memory: false,
            process_memory_sampler: None,
            stage_memory_target: None,
            stage_memory: None,
            checkpoints: Vec::new(),
            cache_probe: None,
        }
    }

    pub(crate) fn new(
        timing: bool,
        retained_memory: bool,
        process_memory_sampler: Option<ProcessMemorySampler>,
        stage_memory_target: Option<BuildProfileStage>,
    ) -> Self {
        Self {
            started_at: Instant::now(),
            timing,
            retained_memory,
            process_memory_sampler,
            stage_memory_target,
            stage_memory: None,
            checkpoints: Vec::new(),
            cache_probe: None,
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
        if !self.is_enabled() {
            return;
        }

        let elapsed = self.started_at.elapsed();
        let previous_elapsed = self
            .checkpoints
            .last()
            .map(|checkpoint| checkpoint.elapsed)
            .unwrap_or_default();

        self.checkpoints.push(BuildCheckpoint {
            label,
            phase_elapsed: elapsed.saturating_sub(previous_elapsed),
            elapsed,
            retained_bytes,
            active_retained_bytes,
            allocated_bytes: process_memory.map(|memory| memory.allocated_bytes),
            active_bytes: process_memory.map(|memory| memory.active_bytes),
            resident_bytes: process_memory.map(|memory| memory.resident_bytes),
        });
    }

    pub(crate) fn record_cache_probe(&mut self, cache_probe: Option<CacheProbeProfile>) {
        if !self.is_enabled() {
            return;
        }

        self.cache_probe = cache_probe;
    }

    pub(crate) fn finish(self) -> BuildProfile {
        BuildProfile::new(self.checkpoints, self.cache_probe, self.stage_memory)
    }

    fn is_enabled(&self) -> bool {
        self.timing
            || self.retained_memory
            || self.process_memory_sampler.is_some()
            || self.stage_memory_target.is_some()
    }
}
