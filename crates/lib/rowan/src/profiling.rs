//! Temporary profiling hooks for the bump-arena fork.
//!
//! These hooks are deliberately simple and env-gated. They exist to make it
//! easy for downstream batch tools to print rowan allocation checkpoints while
//! investigating peak memory, and can be removed once the data has pointed us
//! at the right representation changes.

use std::{
    collections::HashMap,
    env,
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex, MutexGuard, OnceLock,
    },
};

pub const PROFILING_ENV_VAR: &str = "ROWAN_PROFILE";

static PROFILING_ENABLED: OnceLock<bool> = OnceLock::new();
static NEXT_ARENA_ID: AtomicU64 = AtomicU64::new(1);

pub static PROFILING_DATA: OnceLock<Mutex<ProfilingData>> = OnceLock::new();

const ARENA_ORIGIN_COUNT: usize = 7;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ArenaOrigin {
    Builder,
    StandaloneNodeNew,
    ReplacementRebuild,
    CloneSubtree,
    StandaloneTokenNew,
    TokenToOwned,
    Unknown,
}

impl ArenaOrigin {
    const ALL: [ArenaOrigin; ARENA_ORIGIN_COUNT] = [
        ArenaOrigin::Builder,
        ArenaOrigin::StandaloneNodeNew,
        ArenaOrigin::ReplacementRebuild,
        ArenaOrigin::CloneSubtree,
        ArenaOrigin::StandaloneTokenNew,
        ArenaOrigin::TokenToOwned,
        ArenaOrigin::Unknown,
    ];

    fn index(self) -> usize {
        match self {
            ArenaOrigin::Builder => 0,
            ArenaOrigin::StandaloneNodeNew => 1,
            ArenaOrigin::ReplacementRebuild => 2,
            ArenaOrigin::CloneSubtree => 3,
            ArenaOrigin::StandaloneTokenNew => 4,
            ArenaOrigin::TokenToOwned => 5,
            ArenaOrigin::Unknown => 6,
        }
    }

    fn label(self) -> &'static str {
        match self {
            ArenaOrigin::Builder => "builder",
            ArenaOrigin::StandaloneNodeNew => "green_node_new",
            ArenaOrigin::ReplacementRebuild => "replacement_rebuild",
            ArenaOrigin::CloneSubtree => "clone_subtree",
            ArenaOrigin::StandaloneTokenNew => "green_token_new",
            ArenaOrigin::TokenToOwned => "token_to_owned",
            ArenaOrigin::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Default)]
pub struct ProfilingData {
    live_arenas: HashMap<u64, ArenaStats>,
    created_arenas: u64,
    dropped_arenas: u64,
    created_by_origin: [u64; ARENA_ORIGIN_COUNT],
    dropped_by_origin: [u64; ARENA_ORIGIN_COUNT],
    current: ProfilingTotals,
    peak: ProfilingTotals,
    cumulative: ProfilingTotals,
    cumulative_by_origin: [ProfilingTotals; ARENA_ORIGIN_COUNT],
    dropped: ProfilingTotals,
    dropped_by_origin_totals: [ProfilingTotals; ARENA_ORIGIN_COUNT],
    notes: Vec<String>,
}

#[derive(Debug, Clone, Copy)]
struct ArenaStats {
    origin: ArenaOrigin,
    totals: ProfilingTotals,
}

impl Default for ArenaStats {
    fn default() -> Self {
        ArenaStats { origin: ArenaOrigin::Unknown, totals: ProfilingTotals::default() }
    }
}

#[derive(Debug, Default, Clone, Copy)]
struct OriginLiveStats {
    arenas: u64,
    totals: ProfilingTotals,
}

#[derive(Debug, Default, Clone, Copy)]
struct ProfilingTotals {
    arenas: u64,
    allocations: u64,
    requested_bytes: u64,
    bump_allocated_bytes: u64,
    bump_allocated_including_metadata_bytes: u64,
    current_chunk_remaining_bytes: u64,
    struct_bytes: u64,
    child_slice_bytes: u64,
    token_text_bytes: u64,
    nodes: u64,
    tokens: u64,
    child_slots: u64,
}

pub fn profiling_enabled() -> bool {
    *PROFILING_ENABLED.get_or_init(|| {
        let Some(value) = env::var_os(PROFILING_ENV_VAR) else {
            return false;
        };
        let value = value.to_string_lossy();
        !matches!(value.as_ref(), "" | "0" | "false" | "FALSE" | "off" | "OFF")
    })
}

pub fn record_profiling(note: impl Into<String>) {
    modify_profiling(|profiling| profiling.record_note(note));
}

pub fn modify_profiling(f: impl FnOnce(&mut ProfilingData)) {
    if !profiling_enabled() {
        return;
    }

    let mut profiling = profiling_data();
    f(&mut profiling);
}

pub fn print_profiling_data(checkpoint: impl AsRef<str>) {
    if !profiling_enabled() {
        return;
    }

    let profiling = profiling_data();
    let live_by_origin = profiling.live_by_origin();

    eprintln!("[rowan profile] {}", checkpoint.as_ref());
    eprintln!(
        "  arenas: live={} created={} dropped={}",
        profiling.current.arenas, profiling.created_arenas, profiling.dropped_arenas
    );
    eprintln!(
        "  live: requested={} reserved={} reserved_meta={} reserved_over_requested={} current_chunk_remaining={}",
        format_bytes(profiling.current.requested_bytes),
        format_bytes(profiling.current.bump_allocated_bytes),
        format_bytes(profiling.current.bump_allocated_including_metadata_bytes),
        format_bytes(
            profiling
                .current
                .bump_allocated_bytes
                .saturating_sub(profiling.current.requested_bytes),
        ),
        format_bytes(profiling.current.current_chunk_remaining_bytes),
    );
    eprintln!(
        "  live breakdown: allocations={} nodes={} tokens={} child_slots={} structs={} child_slices={} token_text={}",
        profiling.current.allocations,
        profiling.current.nodes,
        profiling.current.tokens,
        profiling.current.child_slots,
        format_bytes(profiling.current.struct_bytes),
        format_bytes(profiling.current.child_slice_bytes),
        format_bytes(profiling.current.token_text_bytes),
    );
    eprintln!(
        "  peak: requested={} reserved={} reserved_meta={} reserved_over_requested={} arenas={}",
        format_bytes(profiling.peak.requested_bytes),
        format_bytes(profiling.peak.bump_allocated_bytes),
        format_bytes(profiling.peak.bump_allocated_including_metadata_bytes),
        format_bytes(
            profiling.peak.bump_allocated_bytes.saturating_sub(profiling.peak.requested_bytes),
        ),
        profiling.peak.arenas,
    );
    eprintln!(
        "  cumulative: allocations={} nodes={} tokens={} child_slots={} requested={} dropped_reserved={}",
        profiling.cumulative.allocations,
        profiling.cumulative.nodes,
        profiling.cumulative.tokens,
        profiling.cumulative.child_slots,
        format_bytes(profiling.cumulative.requested_bytes),
        format_bytes(profiling.dropped.bump_allocated_bytes),
    );

    if profiling.has_origin_data(&live_by_origin) {
        eprintln!("  arena origins:");
        for origin in ArenaOrigin::ALL {
            let index = origin.index();
            let live = live_by_origin[index];
            let cumulative = profiling.cumulative_by_origin[index];
            let created = profiling.created_by_origin[index];
            let dropped = profiling.dropped_by_origin[index];

            if live.arenas == 0 && created == 0 && dropped == 0 && cumulative.allocations == 0 {
                continue;
            }

            eprintln!(
                "    {}: live={} created={} dropped={} allocations={} requested={} reserved={} overhead={} nodes={} tokens={} child_slots={} cumulative_requested={}",
                origin.label(),
                live.arenas,
                created,
                dropped,
                live.totals.allocations,
                format_bytes(live.totals.requested_bytes),
                format_bytes(live.totals.bump_allocated_bytes),
                format_bytes(
                    live.totals
                        .bump_allocated_bytes
                        .saturating_sub(live.totals.requested_bytes),
                ),
                live.totals.nodes,
                live.totals.tokens,
                live.totals.child_slots,
                format_bytes(cumulative.requested_bytes),
            );
        }
    }

    if !profiling.notes.is_empty() {
        eprintln!("  notes:");
        for note in &profiling.notes {
            eprintln!("    - {note}");
        }
    }
}

impl ProfilingData {
    pub fn record_note(&mut self, note: impl Into<String>) {
        self.notes.push(note.into());
    }

    pub fn clear_notes(&mut self) {
        self.notes.clear();
    }

    fn register_arena(&mut self, id: u64, origin: ArenaOrigin) {
        self.created_arenas += 1;
        self.created_by_origin[origin.index()] += 1;
        self.current.arenas += 1;
        self.cumulative.arenas += 1;
        self.live_arenas.insert(id, ArenaStats { origin, totals: ProfilingTotals::default() });
        self.update_peak();
    }

    fn record_arena_allocation(
        &mut self,
        id: u64,
        kind: AllocationKind,
        requested_bytes: usize,
        bump_allocated_bytes: usize,
        bump_allocated_including_metadata_bytes: usize,
        current_chunk_remaining_bytes: usize,
    ) {
        let (origin, old, new) = {
            let arena = self.live_arenas.entry(id).or_default();
            let old = arena.totals;

            arena.totals.allocations += 1;
            arena.totals.requested_bytes += requested_bytes as u64;
            arena.totals.bump_allocated_bytes = bump_allocated_bytes as u64;
            arena.totals.bump_allocated_including_metadata_bytes =
                bump_allocated_including_metadata_bytes as u64;
            arena.totals.current_chunk_remaining_bytes = current_chunk_remaining_bytes as u64;

            match kind {
                AllocationKind::Struct => {
                    arena.totals.struct_bytes += requested_bytes as u64;
                }
                AllocationKind::ChildSlice { slots } => {
                    arena.totals.child_slice_bytes += requested_bytes as u64;
                    arena.totals.child_slots += slots as u64;
                }
                AllocationKind::TokenText => {
                    arena.totals.token_text_bytes += requested_bytes as u64;
                }
            }

            (arena.origin, old, arena.totals)
        };

        self.current.add_delta(old, new);
        add_cumulative_allocation(&mut self.cumulative, old, new, requested_bytes);
        add_cumulative_allocation(
            &mut self.cumulative_by_origin[origin.index()],
            old,
            new,
            requested_bytes,
        );
        self.update_peak();
    }

    fn record_node(&mut self, id: u64) {
        let origin = {
            let arena = self.live_arenas.entry(id).or_default();
            arena.totals.nodes += 1;
            arena.origin
        };
        self.current.nodes += 1;
        self.cumulative.nodes += 1;
        self.cumulative_by_origin[origin.index()].nodes += 1;
        self.update_peak();
    }

    fn record_token(&mut self, id: u64) {
        let origin = {
            let arena = self.live_arenas.entry(id).or_default();
            arena.totals.tokens += 1;
            arena.origin
        };
        self.current.tokens += 1;
        self.cumulative.tokens += 1;
        self.cumulative_by_origin[origin.index()].tokens += 1;
        self.update_peak();
    }

    fn drop_arena(
        &mut self,
        id: u64,
        bump_allocated_bytes: usize,
        bump_allocated_including_metadata_bytes: usize,
        current_chunk_remaining_bytes: usize,
    ) {
        let Some(mut arena) = self.live_arenas.remove(&id) else {
            return;
        };

        arena.totals.bump_allocated_bytes = bump_allocated_bytes as u64;
        arena.totals.bump_allocated_including_metadata_bytes =
            bump_allocated_including_metadata_bytes as u64;
        arena.totals.current_chunk_remaining_bytes = current_chunk_remaining_bytes as u64;

        self.dropped_arenas += 1;
        self.dropped_by_origin[arena.origin.index()] += 1;
        self.current.arenas = self.current.arenas.saturating_sub(1);
        self.current.subtract(arena.totals);
        self.dropped.add(arena.totals);
        self.dropped_by_origin_totals[arena.origin.index()].add(arena.totals);
    }

    fn update_peak(&mut self) {
        self.peak.max_assign(self.current);
    }

    fn live_by_origin(&self) -> [OriginLiveStats; ARENA_ORIGIN_COUNT] {
        let mut origins = [OriginLiveStats::default(); ARENA_ORIGIN_COUNT];
        for arena in self.live_arenas.values() {
            let origin = &mut origins[arena.origin.index()];
            origin.arenas += 1;
            origin.totals.add(arena.totals);
        }
        origins
    }

    fn has_origin_data(&self, live_by_origin: &[OriginLiveStats; ARENA_ORIGIN_COUNT]) -> bool {
        ArenaOrigin::ALL.iter().any(|origin| {
            let index = origin.index();
            live_by_origin[index].arenas > 0
                || self.created_by_origin[index] > 0
                || self.dropped_by_origin[index] > 0
                || self.cumulative_by_origin[index].allocations > 0
        })
    }
}

fn add_cumulative_allocation(
    cumulative: &mut ProfilingTotals,
    old: ProfilingTotals,
    new: ProfilingTotals,
    requested_bytes: usize,
) {
    cumulative.allocations += 1;
    cumulative.requested_bytes += requested_bytes as u64;
    cumulative.struct_bytes += new.struct_bytes.saturating_sub(old.struct_bytes);
    cumulative.child_slice_bytes += new.child_slice_bytes.saturating_sub(old.child_slice_bytes);
    cumulative.token_text_bytes += new.token_text_bytes.saturating_sub(old.token_text_bytes);
    cumulative.child_slots += new.child_slots.saturating_sub(old.child_slots);
}

impl ProfilingTotals {
    fn add_delta(&mut self, old: ProfilingTotals, new: ProfilingTotals) {
        self.allocations += new.allocations.saturating_sub(old.allocations);
        self.requested_bytes += new.requested_bytes.saturating_sub(old.requested_bytes);
        self.bump_allocated_bytes =
            self.bump_allocated_bytes.saturating_sub(old.bump_allocated_bytes)
                + new.bump_allocated_bytes;
        self.bump_allocated_including_metadata_bytes = self
            .bump_allocated_including_metadata_bytes
            .saturating_sub(old.bump_allocated_including_metadata_bytes)
            + new.bump_allocated_including_metadata_bytes;
        self.current_chunk_remaining_bytes =
            self.current_chunk_remaining_bytes.saturating_sub(old.current_chunk_remaining_bytes)
                + new.current_chunk_remaining_bytes;
        self.struct_bytes += new.struct_bytes.saturating_sub(old.struct_bytes);
        self.child_slice_bytes += new.child_slice_bytes.saturating_sub(old.child_slice_bytes);
        self.token_text_bytes += new.token_text_bytes.saturating_sub(old.token_text_bytes);
        self.child_slots += new.child_slots.saturating_sub(old.child_slots);
    }

    fn subtract(&mut self, rhs: ProfilingTotals) {
        self.allocations = self.allocations.saturating_sub(rhs.allocations);
        self.requested_bytes = self.requested_bytes.saturating_sub(rhs.requested_bytes);
        self.bump_allocated_bytes =
            self.bump_allocated_bytes.saturating_sub(rhs.bump_allocated_bytes);
        self.bump_allocated_including_metadata_bytes = self
            .bump_allocated_including_metadata_bytes
            .saturating_sub(rhs.bump_allocated_including_metadata_bytes);
        self.current_chunk_remaining_bytes =
            self.current_chunk_remaining_bytes.saturating_sub(rhs.current_chunk_remaining_bytes);
        self.struct_bytes = self.struct_bytes.saturating_sub(rhs.struct_bytes);
        self.child_slice_bytes = self.child_slice_bytes.saturating_sub(rhs.child_slice_bytes);
        self.token_text_bytes = self.token_text_bytes.saturating_sub(rhs.token_text_bytes);
        self.nodes = self.nodes.saturating_sub(rhs.nodes);
        self.tokens = self.tokens.saturating_sub(rhs.tokens);
        self.child_slots = self.child_slots.saturating_sub(rhs.child_slots);
    }

    fn add(&mut self, rhs: ProfilingTotals) {
        self.allocations += rhs.allocations;
        self.requested_bytes += rhs.requested_bytes;
        self.bump_allocated_bytes += rhs.bump_allocated_bytes;
        self.bump_allocated_including_metadata_bytes += rhs.bump_allocated_including_metadata_bytes;
        self.current_chunk_remaining_bytes += rhs.current_chunk_remaining_bytes;
        self.struct_bytes += rhs.struct_bytes;
        self.child_slice_bytes += rhs.child_slice_bytes;
        self.token_text_bytes += rhs.token_text_bytes;
        self.nodes += rhs.nodes;
        self.tokens += rhs.tokens;
        self.child_slots += rhs.child_slots;
    }

    fn max_assign(&mut self, rhs: ProfilingTotals) {
        self.arenas = self.arenas.max(rhs.arenas);
        self.allocations = self.allocations.max(rhs.allocations);
        self.requested_bytes = self.requested_bytes.max(rhs.requested_bytes);
        self.bump_allocated_bytes = self.bump_allocated_bytes.max(rhs.bump_allocated_bytes);
        self.bump_allocated_including_metadata_bytes = self
            .bump_allocated_including_metadata_bytes
            .max(rhs.bump_allocated_including_metadata_bytes);
        self.current_chunk_remaining_bytes =
            self.current_chunk_remaining_bytes.max(rhs.current_chunk_remaining_bytes);
        self.struct_bytes = self.struct_bytes.max(rhs.struct_bytes);
        self.child_slice_bytes = self.child_slice_bytes.max(rhs.child_slice_bytes);
        self.token_text_bytes = self.token_text_bytes.max(rhs.token_text_bytes);
        self.nodes = self.nodes.max(rhs.nodes);
        self.tokens = self.tokens.max(rhs.tokens);
        self.child_slots = self.child_slots.max(rhs.child_slots);
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum AllocationKind {
    Struct,
    ChildSlice { slots: usize },
    TokenText,
}

pub(crate) fn register_arena(origin: ArenaOrigin) -> Option<u64> {
    if !profiling_enabled() {
        return None;
    }

    let id = NEXT_ARENA_ID.fetch_add(1, Ordering::Relaxed);
    let mut profiling = profiling_data();
    profiling.register_arena(id, origin);
    Some(id)
}

pub(crate) fn record_arena_allocation(
    id: Option<u64>,
    kind: AllocationKind,
    requested_bytes: usize,
    bump_allocated_bytes: usize,
    bump_allocated_including_metadata_bytes: usize,
    current_chunk_remaining_bytes: usize,
) {
    let Some(id) = id else {
        return;
    };

    let mut profiling = profiling_data();
    profiling.record_arena_allocation(
        id,
        kind,
        requested_bytes,
        bump_allocated_bytes,
        bump_allocated_including_metadata_bytes,
        current_chunk_remaining_bytes,
    );
}

pub(crate) fn record_node(id: Option<u64>) {
    let Some(id) = id else {
        return;
    };

    let mut profiling = profiling_data();
    profiling.record_node(id);
}

pub(crate) fn record_token(id: Option<u64>) {
    let Some(id) = id else {
        return;
    };

    let mut profiling = profiling_data();
    profiling.record_token(id);
}

pub(crate) fn drop_arena(
    id: Option<u64>,
    bump_allocated_bytes: usize,
    bump_allocated_including_metadata_bytes: usize,
    current_chunk_remaining_bytes: usize,
) {
    let Some(id) = id else {
        return;
    };

    let mut profiling = profiling_data();
    profiling.drop_arena(
        id,
        bump_allocated_bytes,
        bump_allocated_including_metadata_bytes,
        current_chunk_remaining_bytes,
    );
}

fn profiling_data() -> MutexGuard<'static, ProfilingData> {
    PROFILING_DATA
        .get_or_init(|| Mutex::new(ProfilingData::default()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn format_bytes(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;

    let bytes = bytes as f64;
    if bytes >= GIB {
        format!("{:.1} GiB", bytes / GIB)
    } else if bytes >= MIB {
        format!("{:.1} MiB", bytes / MIB)
    } else if bytes >= KIB {
        format!("{:.1} KiB", bytes / KIB)
    } else {
        format!("{bytes:.0} B")
    }
}
