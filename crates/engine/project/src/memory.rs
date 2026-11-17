/// Project lifecycle point where recently dropped query data may still be resident in the allocator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectMemoryPurgePoint {
    AfterProjectBuild,
    AfterProjectChange,
}

impl ProjectMemoryPurgePoint {
    pub fn label(self) -> &'static str {
        match self {
            Self::AfterProjectBuild => "after project build",
            Self::AfterProjectChange => "after project change",
        }
    }
}

/// Let the executable release allocator pages after a project operation.
pub trait ProjectMemoryHooks: std::fmt::Debug + Send + Sync {
    fn purge(&self, point: ProjectMemoryPurgePoint);
}

#[derive(Debug, Default)]
pub(crate) struct NoopProjectMemoryHooks;

impl ProjectMemoryHooks for NoopProjectMemoryHooks {
    fn purge(&self, _point: ProjectMemoryPurgePoint) {}
}
