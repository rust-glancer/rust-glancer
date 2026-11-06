//! Project construction options.

/// Controls whether a fresh project build can seed offloadable packages from cache artifacts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StartupCacheLoad {
    Disabled,
    #[default]
    Enabled,
}

impl StartupCacheLoad {
    pub(crate) fn is_enabled(self) -> bool {
        matches!(self, Self::Enabled)
    }
}
