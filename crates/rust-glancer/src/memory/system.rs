use rg_lsp_engine::AllocatorStats;

pub(super) const NAME: &str = "system";

pub(super) fn capture_stats() -> Option<AllocatorStats> {
    None
}

pub(super) fn try_purge() -> bool {
    false
}
