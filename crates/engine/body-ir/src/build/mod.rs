//! Lower and infer saved and current-source bodies with shared kernels.

mod const_eval;
mod current;
mod local_items;
mod lower;
mod pattern_binding;
mod query_source;
mod saved;
mod state;

pub use self::{
    current::{
        CurrentSourceBuildCheckpoint, CurrentSourceBuildSummary, CurrentSourceBuilder,
        CurrentSourceSelection, CurrentSourceUnavailable,
    },
    saved::{PreparedBody, infer_saved_body, prepare_saved_body, saved_body},
};
