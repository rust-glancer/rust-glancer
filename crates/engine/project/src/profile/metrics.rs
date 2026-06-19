use rg_profile::{
    ProfileCheckpointColumn, ProfileCheckpointValue, ProfileDescriptor, declare_metrics,
};

use super::BuildProcessMemory;

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
            checkpoint CHECKPOINTS = "checkpoints" [columns super::BUILD_CHECKPOINT_COLUMNS, title "Build checkpoints"];
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

        scope "project.build.item_tree_syntax_eviction" {
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

        scope "project.build.item_tree_drop" {
            memory_snapshot ITEM_TREE_DROP_MEMORY = "memory" [title "after item-tree drop"];
        }

        scope "project.build.body_ir" {
            memory_snapshot BODY_IR_MEMORY = "memory" [title "after body-ir"];
        }

        scope "project.build.parse_syntax_eviction" {
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

pub const BUILD_CHECKPOINTS: rg_profile::CheckpointMetric = metric::CHECKPOINTS;

pub(crate) fn profile_descriptors() -> &'static [ProfileDescriptor] {
    metric::descriptors()
}

pub(crate) fn record_build_checkpoint(
    label: &'static str,
    retained_bytes: Option<usize>,
    active_retained_bytes: Option<usize>,
    process_memory: Option<BuildProcessMemory>,
) {
    metric::CHECKPOINTS.record(
        label,
        vec![
            ProfileCheckpointValue::optional_bytes("retained_bytes", retained_bytes),
            ProfileCheckpointValue::optional_bytes("active_retained_bytes", active_retained_bytes),
            ProfileCheckpointValue::optional_bytes(
                "allocated_bytes",
                process_memory.map(|memory| memory.allocated_bytes),
            ),
            ProfileCheckpointValue::optional_bytes(
                "active_bytes",
                process_memory.map(|memory| memory.active_bytes),
            ),
            ProfileCheckpointValue::optional_bytes(
                "resident_bytes",
                process_memory.map(|memory| memory.resident_bytes),
            ),
            ProfileCheckpointValue::optional_bytes(
                "mapped_bytes",
                process_memory.map(|memory| memory.mapped_bytes),
            ),
        ],
    );
}
