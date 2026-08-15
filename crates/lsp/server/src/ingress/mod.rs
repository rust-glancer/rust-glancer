//! Keeps editor changes and request snapshots in incoming LSP message order.
//!
//! `tower_lsp_server` decodes messages in order, but it may run their async handlers at the same
//! time. For example, if `didChange` is followed by completion, the stored text must be updated
//! before completion chooses its document snapshot. This module performs that small,
//! order-sensitive part in `Service::call`, before either handler starts.
//!
//! The work is split into three small layers:
//!
//! 1. `edit` applies every edit from one `didChange` notification and produces one complete
//!    document value. It also records how a position from the old text moves through those edits.
//! 2. `state` stores open sessions, complete synchronized text, document revisions, and the engine
//!    route assigned to each session. It takes immutable snapshots for requests; the engine
//!    receives those snapshots instead of keeping another live copy of editor state.
//! 3. `service` examines each decoded message before its handler starts. It updates editor state or
//!    takes a request snapshot, then passes any remaining per-message work to that handler.

mod edit;
mod service;
mod state;

pub(crate) use self::{
    service::{EditorIngress, completion_request, document_request, lifecycle_event},
    state::{
        CapturedDocument, DiagnosticsPublication, DocumentRevisionWatch, EditorStateHandle,
        LifecycleEvent,
    },
};
