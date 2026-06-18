use rg_body_ir::BodyIrDb;
use rg_def_map::DefMapDb;
use rg_item_tree::ItemTreeDb;
use rg_parse::ParseDb;
use rg_profile::MemorySnapshotMetric;
use rg_semantic_ir::SemanticIrDb;
use rg_std::{MemoryRecorder, MemorySize};
use rg_text::PackageNameInterners;

use crate::{cache::Fingerprint, profile::BuildProfiler, project::package_set::PhasePackageSet};

/// Snapshot of the phase locals that are still alive at a build checkpoint.
///
/// Each checkpoint fills the objects that exist at that moment; recording skips absent fields so
/// the call sites read as a compact list of live phase state instead of repeating recorder plumbing.
#[derive(Default)]
pub(super) struct CheckpointMemory<'a> {
    names: Option<&'a PackageNameInterners>,
    parse: Option<&'a ParseDb>,
    build_plan: Option<&'a PhasePackageSet>,
    item_tree: Option<&'a ItemTreeDb>,
    source_fingerprints: Option<&'a Vec<Option<Fingerprint>>>,
    def_map: Option<&'a DefMapDb>,
    semantic_ir: Option<&'a SemanticIrDb>,
    body_ir: Option<&'a BodyIrDb>,
}

impl<'a> CheckpointMemory<'a> {
    pub(super) fn merge(self, other: Self) -> Self {
        Self {
            names: self.names.or(other.names),
            parse: self.parse.or(other.parse),
            build_plan: self.build_plan.or(other.build_plan),
            item_tree: self.item_tree.or(other.item_tree),
            source_fingerprints: self.source_fingerprints.or(other.source_fingerprints),
            def_map: self.def_map.or(other.def_map),
            semantic_ir: self.semantic_ir.or(other.semantic_ir),
            body_ir: self.body_ir.or(other.body_ir),
        }
    }

    pub(super) fn checkpoint<T>(
        self,
        profiler: &mut BuildProfiler,
        memory: MemorySnapshotMetric,
        retained: &T,
    ) where
        T: MemorySize,
    {
        let label = memory
            .title_text()
            .expect("build memory snapshot metrics should have report titles");
        let process_memory = profiler.sample_process_memory();
        let retained_bytes = profiler.measure(retained);
        let active_retained_bytes = self.measure_retained(profiler);
        profiler.record(label, retained_bytes, active_retained_bytes, process_memory);
        self.capture_memory_snapshot(profiler, memory);
    }

    pub(super) fn checkpoint_without_retained(
        self,
        profiler: &mut BuildProfiler,
        memory: MemorySnapshotMetric,
    ) {
        let label = memory
            .title_text()
            .expect("build memory snapshot metrics should have report titles");
        let process_memory = profiler.sample_process_memory();
        let active_retained_bytes = self.measure_retained(profiler);
        profiler.record(label, None, active_retained_bytes, process_memory);
        self.capture_memory_snapshot(profiler, memory);
    }

    fn capture_memory_snapshot(&self, profiler: &mut BuildProfiler, memory: MemorySnapshotMetric) {
        profiler.capture_memory_snapshot(memory, |recorder| self.record(recorder));
    }

    fn measure_retained(&self, profiler: &BuildProfiler) -> Option<usize> {
        profiler.sum_retained(&[
            self.names.and_then(|value| profiler.measure(value)),
            self.parse.and_then(|value| profiler.measure(value)),
            self.build_plan.and_then(|value| profiler.measure(value)),
            self.item_tree.and_then(|value| profiler.measure(value)),
            self.source_fingerprints
                .and_then(|value| profiler.measure(value)),
            self.def_map.and_then(|value| profiler.measure(value)),
            self.semantic_ir.and_then(|value| profiler.measure(value)),
            self.body_ir.and_then(|value| profiler.measure(value)),
        ])
    }

    fn record(&self, recorder: &mut MemoryRecorder) {
        BuildProfiler::record_memory_value(recorder, "names", self.names);
        BuildProfiler::record_memory_value(recorder, "parse", self.parse);
        BuildProfiler::record_memory_value(recorder, "build_plan", self.build_plan);
        BuildProfiler::record_memory_value(recorder, "item_tree", self.item_tree);
        BuildProfiler::record_memory_value(
            recorder,
            "source_fingerprints",
            self.source_fingerprints,
        );
        BuildProfiler::record_memory_value(recorder, "def_map", self.def_map);
        BuildProfiler::record_memory_value(recorder, "semantic_ir", self.semantic_ir);
        BuildProfiler::record_memory_value(recorder, "body_ir", self.body_ir);
    }
}

macro_rules! impl_checkpoint_memory_from {
    ($ty:ty, $field:ident) => {
        impl<'a> From<&'a $ty> for CheckpointMemory<'a> {
            fn from(value: &'a $ty) -> Self {
                Self {
                    $field: Some(value),
                    ..Self::default()
                }
            }
        }
    };
}

impl_checkpoint_memory_from!(PackageNameInterners, names);
impl_checkpoint_memory_from!(ParseDb, parse);
impl_checkpoint_memory_from!(PhasePackageSet, build_plan);
impl_checkpoint_memory_from!(ItemTreeDb, item_tree);
impl_checkpoint_memory_from!(Vec<Option<Fingerprint>>, source_fingerprints);
impl_checkpoint_memory_from!(DefMapDb, def_map);
impl_checkpoint_memory_from!(SemanticIrDb, semantic_ir);
impl_checkpoint_memory_from!(BodyIrDb, body_ir);
