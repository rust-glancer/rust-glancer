use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::Context as _;
use rg_lsp_proto::{AnalysisConfig, DiagnosticsConfig};
use tokio::sync::Notify;

use super::{
    routing::{DocumentEngineRoute, EngineId, EngineRouting},
    slot::EngineSlot,
};

/// Mutable registry state guarded by `EngineRegistry`'s mutex.
///
/// The key invariant here is that routing reservations and engine slot allocation happen together:
/// once routing hands out a fresh `EngineId`, the same lock scope pushes the corresponding slot.
#[derive(Debug, Default)]
pub(super) struct EngineRegistryInner {
    pub(super) routing: EngineRouting,
    pub(super) engines: Vec<EngineSlot>,
    pub(super) analysis_config: Option<AnalysisConfig>,
    pub(super) diagnostics_config: Option<DiagnosticsConfig>,
}

impl EngineRegistryInner {
    pub(super) fn route_document(
        &mut self,
        path: &Path,
    ) -> anyhow::Result<Option<ReservedEngineRoute>> {
        self.routing
            .route_document(path)
            .map(|route| self.reserve_route(route))
            .transpose()
    }

    pub(super) fn engine(&self, id: EngineId) -> Option<&EngineSlot> {
        self.engines.get(id.index())
    }

    fn reserve_route(&mut self, route: DocumentEngineRoute) -> anyhow::Result<ReservedEngineRoute> {
        match route {
            DocumentEngineRoute::Existing(id) => Ok(ReservedEngineRoute::Existing(id)),
            DocumentEngineRoute::Spawn { new_id, root } => {
                let config = match self.spawn_config() {
                    Ok(config) => config,
                    Err(error) => {
                        self.push_slot(
                            new_id,
                            EngineSlot::Failed {
                                root: root.clone(),
                                error: Arc::from(error.to_string()),
                            },
                        );
                        return Err(error);
                    }
                };

                // The slot is visible before the process exists. Concurrent routes for the same
                // root will now receive `Existing(new_id)` and wait on this notification.
                self.push_slot(
                    new_id,
                    EngineSlot::Starting {
                        notify: Arc::new(Notify::new()),
                    },
                );

                Ok(ReservedEngineRoute::Spawn(ReservedEngineStart {
                    id: new_id,
                    root,
                    config,
                }))
            }
        }
    }

    fn spawn_config(&self) -> anyhow::Result<EngineSpawnConfig> {
        Ok(EngineSpawnConfig {
            analysis: self
                .analysis_config
                .clone()
                .context("while attempting to spawn engine before analysis configuration")?,
            diagnostics: self
                .diagnostics_config
                .clone()
                .context("while attempting to spawn engine before diagnostics configuration")?,
        })
    }

    fn push_slot(&mut self, id: EngineId, slot: EngineSlot) {
        assert_eq!(
            id.index(),
            self.engines.len(),
            "reserved engine id should match next engine slot"
        );
        self.engines.push(slot);
    }
}

#[derive(Debug)]
pub(super) struct EngineSpawnConfig {
    pub(super) analysis: AnalysisConfig,
    pub(super) diagnostics: DiagnosticsConfig,
}

/// Registry action after routing has been resolved under the state lock.
#[derive(Debug)]
pub(super) enum ReservedEngineRoute {
    Existing(EngineId),
    Spawn(ReservedEngineStart),
}

/// Token proving that an engine id has been reserved and needs process startup.
#[derive(Debug)]
pub(super) struct ReservedEngineStart {
    pub(super) id: EngineId,
    pub(super) root: PathBuf,
    pub(super) config: EngineSpawnConfig,
}
