use rg_body_ir::BodyIrDb;
use rg_def_map::{DefMapDb, PackageSlot};
use rg_item_tree::ItemTreeDb;
use rg_memsize::{MemoryRecorder, MemorySize};
use rg_parse::ParseDb;
use rg_semantic_ir::SemanticIrDb;
use rg_text::PackageNameInterners;

use crate::{
    cache::Fingerprint,
    profile::{BuildProfileStage, BuildProfiler},
};

/// Snapshot of the phase locals that are still alive at a build checkpoint.
///
/// Each checkpoint fills the objects that exist at that moment; recording skips absent fields so
/// the call sites read as a compact list of live phase state instead of repeating recorder plumbing.
#[derive(Default)]
pub(super) struct StageMemory<'a> {
    names: Option<&'a PackageNameInterners>,
    parse: Option<&'a ParseDb>,
    build_plan: Option<&'a Vec<PackageSlot>>,
    item_tree: Option<&'a ItemTreeDb>,
    source_fingerprints: Option<&'a Vec<Option<Fingerprint>>>,
    def_map: Option<&'a DefMapDb>,
    semantic_ir: Option<&'a SemanticIrDb>,
    body_ir: Option<&'a BodyIrDb>,
}

impl<'a> StageMemory<'a> {
    pub(super) fn names(mut self, names: &'a PackageNameInterners) -> Self {
        self.names = Some(names);
        self
    }

    pub(super) fn parse(mut self, parse: &'a ParseDb) -> Self {
        self.parse = Some(parse);
        self
    }

    pub(super) fn build_plan(mut self, build_plan: &'a Vec<PackageSlot>) -> Self {
        self.build_plan = Some(build_plan);
        self
    }

    pub(super) fn item_tree(mut self, item_tree: &'a ItemTreeDb) -> Self {
        self.item_tree = Some(item_tree);
        self
    }

    pub(super) fn source_fingerprints(
        mut self,
        source_fingerprints: &'a Vec<Option<Fingerprint>>,
    ) -> Self {
        self.source_fingerprints = Some(source_fingerprints);
        self
    }

    pub(super) fn def_map(mut self, def_map: &'a DefMapDb) -> Self {
        self.def_map = Some(def_map);
        self
    }

    pub(super) fn semantic_ir(mut self, semantic_ir: &'a SemanticIrDb) -> Self {
        self.semantic_ir = Some(semantic_ir);
        self
    }

    pub(super) fn body_ir(mut self, body_ir: &'a BodyIrDb) -> Self {
        self.body_ir = Some(body_ir);
        self
    }

    pub(super) fn checkpoint<T>(
        self,
        profiler: &mut BuildProfiler,
        stage: BuildProfileStage,
        label: &'static str,
        retained: &T,
    ) -> StageMemory<'static>
    where
        T: MemorySize,
    {
        let process_memory = profiler.sample_process_memory();
        let retained_bytes = profiler.measure(retained);
        let active_retained_bytes = self.measure_retained(profiler);
        profiler.record(label, retained_bytes, active_retained_bytes, process_memory);
        self.capture_stage_memory(profiler, stage);
        StageMemory::default()
    }

    pub(super) fn checkpoint_without_retained(
        self,
        profiler: &mut BuildProfiler,
        stage: BuildProfileStage,
        label: &'static str,
    ) -> StageMemory<'static> {
        let process_memory = profiler.sample_process_memory();
        let active_retained_bytes = self.measure_retained(profiler);
        profiler.record(label, None, active_retained_bytes, process_memory);
        self.capture_stage_memory(profiler, stage);
        StageMemory::default()
    }

    fn capture_stage_memory(&self, profiler: &mut BuildProfiler, stage: BuildProfileStage) {
        profiler.capture_stage_memory(stage, |recorder| self.record(recorder));
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
        BuildProfiler::record_stage_value(recorder, "names", self.names);
        BuildProfiler::record_stage_value(recorder, "parse", self.parse);
        BuildProfiler::record_stage_value(recorder, "build_plan", self.build_plan);
        BuildProfiler::record_stage_value(recorder, "item_tree", self.item_tree);
        BuildProfiler::record_stage_value(
            recorder,
            "source_fingerprints",
            self.source_fingerprints,
        );
        BuildProfiler::record_stage_value(recorder, "def_map", self.def_map);
        BuildProfiler::record_stage_value(recorder, "semantic_ir", self.semantic_ir);
        BuildProfiler::record_stage_value(recorder, "body_ir", self.body_ir);
    }
}
