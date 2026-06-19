use rg_body_ir::BodyIrDb;
use rg_def_map::DefMapDb;
use rg_item_tree::ItemTreeDb;
use rg_parse::ParseDb;
use rg_profile::{MemorySnapshotMetric, ProfileMemoryRecord, ProfileMemorySnapshot};
use rg_semantic_ir::SemanticIrDb;
use rg_std::{MemoryRecord, MemoryRecorder, MemorySize};
use rg_text::PackageNameInterners;

use crate::{
    cache::Fingerprint,
    profile::{BuildMemorySampler, record_build_checkpoint},
    project::package_set::PhasePackageSet,
};

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
        sampler: &mut BuildMemorySampler,
        memory: MemorySnapshotMetric,
        retained: &T,
    ) where
        T: MemorySize,
    {
        let label = memory
            .title_text()
            .expect("build memory snapshot metrics should have report titles");
        let process_memory = sampler.sample_process_memory();
        let retained_bytes = sampler.measure_retained(retained);
        let active_retained_bytes = self.measure_retained(sampler);
        record_build_checkpoint(label, retained_bytes, active_retained_bytes, process_memory);
        self.capture_memory_snapshot(memory);
    }

    pub(super) fn checkpoint_without_retained(
        self,
        sampler: &mut BuildMemorySampler,
        memory: MemorySnapshotMetric,
    ) {
        let label = memory
            .title_text()
            .expect("build memory snapshot metrics should have report titles");
        let process_memory = sampler.sample_process_memory();
        let active_retained_bytes = self.measure_retained(sampler);
        record_build_checkpoint(label, None, active_retained_bytes, process_memory);
        self.capture_memory_snapshot(memory);
    }

    fn capture_memory_snapshot(&self, memory: MemorySnapshotMetric) {
        if !memory.is_enabled() {
            return;
        }

        let mut recorder = MemoryRecorder::new("build");
        self.record(&mut recorder);
        let records = recorder
            .records()
            .into_iter()
            .map(Self::profile_memory_record)
            .collect();
        memory.record(ProfileMemorySnapshot::new(recorder.total_bytes(), records));
    }

    fn profile_memory_record(record: MemoryRecord) -> ProfileMemoryRecord {
        ProfileMemoryRecord::new(
            record.path,
            record.type_name,
            record.kind.as_str(),
            record.bytes,
        )
    }

    fn measure_retained(&self, sampler: &BuildMemorySampler) -> Option<usize> {
        sampler.sum_retained(&[
            self.names.and_then(|value| sampler.measure_retained(value)),
            self.parse.and_then(|value| sampler.measure_retained(value)),
            self.build_plan
                .and_then(|value| sampler.measure_retained(value)),
            self.item_tree
                .and_then(|value| sampler.measure_retained(value)),
            self.source_fingerprints
                .and_then(|value| sampler.measure_retained(value)),
            self.def_map
                .and_then(|value| sampler.measure_retained(value)),
            self.semantic_ir
                .and_then(|value| sampler.measure_retained(value)),
            self.body_ir
                .and_then(|value| sampler.measure_retained(value)),
        ])
    }

    fn record(&self, recorder: &mut MemoryRecorder) {
        Self::record_memory_value(recorder, "names", self.names);
        Self::record_memory_value(recorder, "parse", self.parse);
        Self::record_memory_value(recorder, "build_plan", self.build_plan);
        Self::record_memory_value(recorder, "item_tree", self.item_tree);
        Self::record_memory_value(recorder, "source_fingerprints", self.source_fingerprints);
        Self::record_memory_value(recorder, "def_map", self.def_map);
        Self::record_memory_value(recorder, "semantic_ir", self.semantic_ir);
        Self::record_memory_value(recorder, "body_ir", self.body_ir);
    }

    fn record_memory_value<T>(recorder: &mut MemoryRecorder, label: &'static str, value: Option<&T>)
    where
        T: MemorySize,
    {
        if let Some(value) = value {
            recorder.scope(label, |recorder| value.record_memory_size(recorder));
        }
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
