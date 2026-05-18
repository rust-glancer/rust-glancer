/// Project lifecycle point where recently dropped transient data may still be resident in the
/// process allocator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectMemoryPurgePoint {
    AfterItemTreeSyntaxEviction,
    AfterProjectBuild,
    AfterPackageRebuild,
    AfterDirtyOverlayBuild,
}

impl ProjectMemoryPurgePoint {
    pub fn label(self) -> &'static str {
        match self {
            Self::AfterItemTreeSyntaxEviction => "after item-tree syntax eviction",
            Self::AfterProjectBuild => "after project build",
            Self::AfterPackageRebuild => "after package rebuild",
            Self::AfterDirtyOverlayBuild => "after dirty overlay",
        }
    }
}

/// Allocator cleanup hook called by project-owned build and rebuild boundaries.
///
/// The project knows when large transient phase data has died, but it deliberately does not know
/// which allocator the executable selected. Callers can provide an allocator-specific hook while
/// tests and library users keep the default no-op behavior.
pub trait ProjectMemoryHooks: std::fmt::Debug + Send + Sync {
    fn purge(&self, point: ProjectMemoryPurgePoint);
}

#[derive(Debug, Default)]
pub(crate) struct NoopProjectMemoryHooks;

impl ProjectMemoryHooks for NoopProjectMemoryHooks {
    fn purge(&self, _point: ProjectMemoryPurgePoint) {}
}
