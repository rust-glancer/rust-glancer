use rg_profile::{
    ProfileCheckpointColumn, ProfileCheckpointValue, ProfileDescriptor, declare_metrics,
};

use super::BuildProcessMemory;

static BUILD_CHECKPOINT_COLUMNS: &[ProfileCheckpointColumn] = &[
    ProfileCheckpointColumn::bytes("retained_bytes", "rg_retained"),
    ProfileCheckpointColumn::bytes("allocated_bytes", "a_allocated"),
    ProfileCheckpointColumn::bytes("active_bytes", "a_backed"),
    ProfileCheckpointColumn::bytes("resident_bytes", "a_resident"),
    ProfileCheckpointColumn::bytes("mapped_bytes", "a_mapped"),
];

declare_metrics! {
    pub(crate) mod metric {
        scope "project.build" {
            checkpoint CHECKPOINTS = "checkpoints" [columns super::BUILD_CHECKPOINT_COLUMNS, title "Build checkpoints"];
            /// Wall time for the full Ajika run, including routing, checkpointing and release.
            duration QUERY_OPERATION = "query_operation" [title "Query operation total"];
            duration DECLARATIONS = "declarations" [title "Declaration demand"];
            duration SOURCE_VALIDATION = "source_validation" [title "Source validation"];
        }

        scope "project.build.ajika" {
            counter EXECUTED = "executed" [title "Queries executed"];
            counter VALIDATED = "validated" [title "Query records validated"];
            /// Reuse or reads of stored results and outputs during the operation.
            counter HITS = "hits" [title "Cache hits"];
            counter DECODED = "decoded" [title "Values decoded"];
            counter METADATA_READS = "metadata_reads" [title "Query records read"];
            gauge PAYLOAD_BYTES_READ = "payload_bytes_read" [Bytes, title "Payload bytes read"];
            gauge PAYLOAD_BYTES_WRITTEN = "payload_bytes_written" [Bytes, title "Payload bytes written"];
            gauge METADATA_BYTES_WRITTEN = "metadata_bytes_written" [Bytes, title "Query record bytes written"];
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
    process_memory: Option<BuildProcessMemory>,
) {
    metric::CHECKPOINTS.record(
        label,
        vec![
            ProfileCheckpointValue::optional_bytes("retained_bytes", retained_bytes),
            ProfileCheckpointValue::optional_bytes(
                "allocated_bytes",
                process_memory.and_then(|memory| memory.allocated_bytes),
            ),
            ProfileCheckpointValue::optional_bytes(
                "active_bytes",
                process_memory.and_then(|memory| memory.active_bytes),
            ),
            ProfileCheckpointValue::optional_bytes(
                "resident_bytes",
                process_memory.and_then(|memory| memory.resident_bytes),
            ),
            ProfileCheckpointValue::optional_bytes(
                "mapped_bytes",
                process_memory.and_then(|memory| memory.mapped_bytes),
            ),
        ],
    );
}
