use std::{
    fmt::Write as _,
    path::{Path, PathBuf},
};

use test_fixture::{CrateFixture, fixture_crate};

use crate::engine_registry::routing::{
    EngineId, EngineRouting, WorkspaceEngineRoute, normalize_path,
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

    pub(super) fn render_steps(&mut self, steps: &[RoutingStep]) -> String {
        let mut rendered = String::new();
        for step in steps {
            match step {
                RoutingStep::CachedFileOwner { title, path } => {
                    let owner = self.routing.open_file_owner(&self.path(path));
                    if let Some(id) = owner {
                        self.routing.set_active_id(id);
                    }
                    writeln!(
                        rendered,
                        "{title}: {}",
                        self.render_cached_file_owner(owner)
                    )
                    .expect("routing snapshot should be writable");
                }
                RoutingStep::WorkspaceRoot {
                    title,
                    workspace_root,
                } => {
                    let route = self.routing.route_workspace_root(self.path(workspace_root));
                    self.apply_workspace_route(route.as_ref());
                    writeln!(rendered, "{title}: {}", self.render_workspace_route(&route))
                        .expect("routing snapshot should be writable");
                }
                RoutingStep::DiscoveryWorkspace { title, path } => {
                    let workspace = self
                        .routing
                        .discovery_workspace_for(&self.path(path))
                        .map(|path| self.render_path(path))
                        .unwrap_or_else(|| "none".to_string());
                    writeln!(rendered, "{title}: {workspace}")
                        .expect("routing snapshot should be writable");
                }
                RoutingStep::OpenFile { title, path } => {
                    let active = self
                        .routing
                        .active_id()
                        .expect("open-file step should follow an active engine");
                    self.routing.set_open_file(self.path(path), active);
                    writeln!(rendered, "{title}: {}", self.render_active(Some(active)))
                        .expect("routing snapshot should be writable");
                }
                RoutingStep::CloseFile { title, path } => {
                    self.routing.remove_open_file(&self.path(path), None);
                    writeln!(rendered, "{title}: closed")
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

    fn apply_workspace_route(&mut self, route: Option<&WorkspaceEngineRoute>) {
        match route {
            None => {}
            Some(WorkspaceEngineRoute::Existing(id))
            | Some(WorkspaceEngineRoute::Spawn { new_id: id, .. }) => {
                self.routing.set_active_id(*id);
            }
        }
    }

    fn render_cached_file_owner(&self, owner: Option<EngineId>) -> String {
        match owner {
            Some(id) => format!("existing {}", self.render_engine(id)),
            None => "unowned".to_string(),
        }
    }

    fn render_workspace_route(&self, route: &Option<WorkspaceEngineRoute>) -> String {
        match route {
            Some(WorkspaceEngineRoute::Existing(id)) => {
                format!("existing {}", self.render_engine(*id))
            }
            Some(WorkspaceEngineRoute::Spawn { new_id, root }) => {
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
    CachedFileOwner {
        title: &'static str,
        path: &'static str,
    },
    WorkspaceRoot {
        title: &'static str,
        workspace_root: &'static str,
    },
    DiscoveryWorkspace {
        title: &'static str,
        path: &'static str,
    },
    OpenFile {
        title: &'static str,
        path: &'static str,
    },
    CloseFile {
        title: &'static str,
        path: &'static str,
    },
    WorkspaceAction {
        title: &'static str,
    },
}

impl RoutingStep {
    pub(super) fn cached_file_owner(title: &'static str, path: &'static str) -> Self {
        Self::CachedFileOwner { title, path }
    }

    pub(super) fn workspace_root(title: &'static str, workspace_root: &'static str) -> Self {
        Self::WorkspaceRoot {
            title,
            workspace_root,
        }
    }

    pub(super) fn discovery_workspace(title: &'static str, path: &'static str) -> Self {
        Self::DiscoveryWorkspace { title, path }
    }

    pub(super) fn open_file(title: &'static str, path: &'static str) -> Self {
        Self::OpenFile { title, path }
    }

    pub(super) fn close_file(title: &'static str, path: &'static str) -> Self {
        Self::CloseFile { title, path }
    }

    pub(super) fn workspace_action(title: &'static str) -> Self {
        Self::WorkspaceAction { title }
    }
}

pub(super) fn check_routing(mut fixture: RoutingFixture, steps: &[RoutingStep], expected: &str) {
    let actual = fixture.render_steps(steps);
    assert_eq!(actual.trim_end(), expected.trim());
}
