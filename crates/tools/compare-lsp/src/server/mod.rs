//! Process lifecycle and launch configuration for the LSP benchmark.

mod command;
mod process;
mod stderr;
mod uri;

use std::time::Duration;

pub(crate) use self::uri::file_uri;
pub(super) use self::{command::ServerKind, process::RunningServer};

/// Initialization facts reported with the comparison run.
#[derive(Debug)]
pub(crate) struct ServerReadiness {
    name: &'static str,
    initialize_latency: Duration,
    ready_latency: Duration,
}

impl ServerReadiness {
    pub(super) fn new(
        name: &'static str,
        initialize_latency: Duration,
        ready_latency: Duration,
    ) -> Self {
        Self {
            name,
            initialize_latency,
            ready_latency,
        }
    }

    pub(crate) fn name(&self) -> &'static str {
        self.name
    }

    pub(crate) fn initialize_latency(&self) -> Duration {
        self.initialize_latency
    }

    pub(crate) fn ready_latency(&self) -> Duration {
        self.ready_latency
    }
}
