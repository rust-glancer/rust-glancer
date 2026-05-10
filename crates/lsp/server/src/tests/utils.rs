use std::{
    fmt::Write as _,
    path::{Path, PathBuf},
};

use test_fixture::{CrateFixture, fixture_crate};

use crate::engine_registry::routing::{
    DocumentEngineRoute, EngineId, EngineRouting, normalize_path,
};

pub(super) struct RoutingFixture {
    fixture: CrateFixture,
    routing: EngineRouting,
}

impl RoutingFixture {
    pub(super) fn new(fixture: &str) -> Self {
        Self {
            fixture: fixture_crate(fixture),
            routing: EngineRouting::default(),
        }
    }

    pub(super) fn workspace_folders<const N: usize>(mut self, paths: [&str; N]) -> Self {
        let paths = paths
            .into_iter()
            .map(|path| self.path(path))
            .collect::<Vec<_>>();
        self.routing.set_workspace_folders(paths);
        self
    }

    pub(super) fn engine(mut self, path: &str) -> Self {
        self.ensure_engine(path);
        self
    }

    pub(super) fn active_engine(mut self, path: &str) -> Self {
        let id = self.ensure_engine(path);
        self.routing.set_active_id(id);
        self
    }

    pub(super) fn render_steps(&mut self, steps: &[RoutingStep]) -> String {
        let mut rendered = String::new();
        for step in steps {
            match step {
                RoutingStep::Open { title, path } => {
                    let route = self.routing.route_document(&self.path(path));
                    self.apply_route(route.as_ref());
                    writeln!(rendered, "{title}: {}", self.render_route(route.as_ref()))
                        .expect("routing snapshot should be writable");
                }
                RoutingStep::WorkspaceAction { title } => {
                    let active = self.routing.active_id();
                    writeln!(rendered, "{title}: {}", self.render_active(active))
                        .expect("routing snapshot should be writable");
                }
            }
        }

        rendered
    }

    fn apply_route(&mut self, route: Option<&DocumentEngineRoute>) {
        if let Some(id) = route.map(DocumentEngineRoute::id) {
            self.routing.set_active_id(id);
        }
    }

    fn ensure_engine(&mut self, path: &str) -> EngineId {
        match self.routing.route_root(self.path(path)) {
            DocumentEngineRoute::Existing(id) => id,
            DocumentEngineRoute::Spawn { new_id, .. } => new_id,
        }
    }

    fn render_route(&self, route: Option<&DocumentEngineRoute>) -> String {
        match route {
            Some(DocumentEngineRoute::Existing(id)) => {
                format!("existing {}", self.render_engine(*id))
            }
            Some(DocumentEngineRoute::Spawn { new_id, root }) => {
                format!(
                    "spawn {} {}",
                    Self::render_id(*new_id),
                    self.render_path(root)
                )
            }
            None => "none".to_string(),
        }
    }

    fn render_active(&self, active: Option<EngineId>) -> String {
        match active {
            Some(id) => format!("active {}", self.render_engine(id)),
            None => "none".to_string(),
        }
    }

    fn render_engine(&self, id: EngineId) -> String {
        let root = self
            .routing
            .root_for_id(id)
            .expect("test engine id should have a root");
        format!("{} {}", Self::render_id(id), self.render_path(root))
    }

    fn render_id(id: EngineId) -> String {
        format!("e{}", id.index())
    }

    fn render_path(&self, path: &Path) -> String {
        let root = self.path("");
        let path = normalize_path(path);
        if let Ok(relative) = path.strip_prefix(root) {
            return format!("/{}", relative.display());
        }

        path.display().to_string()
    }

    fn path(&self, path: &str) -> PathBuf {
        normalize_path(self.fixture.path(path))
    }
}

pub(super) enum RoutingStep {
    Open {
        title: &'static str,
        path: &'static str,
    },
    WorkspaceAction {
        title: &'static str,
    },
}

impl RoutingStep {
    pub(super) fn open(title: &'static str, path: &'static str) -> Self {
        Self::Open { title, path }
    }

    pub(super) fn workspace_action(title: &'static str) -> Self {
        Self::WorkspaceAction { title }
    }
}

impl DocumentEngineRoute {
    fn id(&self) -> EngineId {
        match self {
            DocumentEngineRoute::Existing(id) | DocumentEngineRoute::Spawn { new_id: id, .. } => {
                *id
            }
        }
    }
}

pub(super) fn check_routing(mut fixture: RoutingFixture, steps: &[RoutingStep], expected: &str) {
    let actual = fixture.render_steps(steps);
    assert_eq!(actual.trim_end(), expected.trim());
}
