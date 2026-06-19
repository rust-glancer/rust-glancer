use std::{
    cell::RefCell,
    collections::BTreeMap,
    error::Error,
    fmt,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use crate::{
    ProfileCheckpoint, ProfileCheckpointValue, ProfileDescriptor, ProfileFilter,
    ProfileFilterValidationError, ProfileInstrumentKind, ProfileKeyedCounter, ProfileKeyedDuration,
    ProfileMeasurement, ProfileMemorySnapshot, ProfileRegistry, ProfileSnapshot, ProfileValue,
};

static RUNTIME: Mutex<RuntimeState> = Mutex::new(RuntimeState::new());
thread_local! {
    static ACTIVE_RUN: RefCell<Option<Arc<ActiveRun>>> = const { RefCell::new(None) };
}

struct RuntimeState {
    registry: Option<Arc<ProfileRegistry>>,
    next_run_id: u64,
}

impl RuntimeState {
    const fn new() -> Self {
        Self {
            registry: None,
            next_run_id: 1,
        }
    }
}

/// Installs the process-wide profiling vocabulary.
pub fn initialize(registry: ProfileRegistry) -> Result<(), ProfileInitializeError> {
    let mut runtime = RUNTIME
        .lock()
        .expect("profile runtime lock should not be poisoned");
    if runtime.registry.is_some() {
        return Err(ProfileInitializeError::AlreadyInitialized);
    }

    runtime.registry = Some(Arc::new(registry));
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileInitializeError {
    AlreadyInitialized,
}

impl fmt::Display for ProfileInitializeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyInitialized => f.write_str("profile registry is already initialized"),
        }
    }
}

impl Error for ProfileInitializeError {}

/// Guard for one scoped profiling run.
pub struct ProfileRun {
    id: u64,
    active: Arc<ActiveRun>,
    finished: bool,
}

impl ProfileRun {
    pub fn start(filter: ProfileFilter) -> Result<Self, ProfileRunStartError> {
        let registry = {
            let runtime = RUNTIME
                .lock()
                .expect("profile runtime lock should not be poisoned");
            runtime
                .registry
                .clone()
                .ok_or(ProfileRunStartError::Uninitialized)?
        };

        Self::start_with_shared_registry(registry, filter)
    }

    pub fn start_with_registry(
        registry: ProfileRegistry,
        filter: ProfileFilter,
    ) -> Result<Self, ProfileRunStartError> {
        Self::start_with_shared_registry(Arc::new(registry), filter)
    }

    pub fn start_with_shared_registry(
        registry: Arc<ProfileRegistry>,
        filter: ProfileFilter,
    ) -> Result<Self, ProfileRunStartError> {
        registry
            .validate_filter(&filter)
            .map_err(ProfileRunStartError::InvalidFilter)?;

        if ACTIVE_RUN.with(|active| active.borrow().is_some()) {
            return Err(ProfileRunStartError::AlreadyActive);
        }

        let mut runtime = RUNTIME
            .lock()
            .expect("profile runtime lock should not be poisoned");
        let id = runtime.next_run_id;
        runtime.next_run_id = runtime.next_run_id.saturating_add(1);
        let active = Arc::new(ActiveRun {
            id,
            started_at: Instant::now(),
            registry,
            filter,
            collector: Mutex::new(ProfileCollector::default()),
        });
        ACTIVE_RUN.with(|active_slot| {
            *active_slot.borrow_mut() = Some(Arc::clone(&active));
        });

        Ok(Self {
            id,
            active,
            finished: false,
        })
    }

    pub fn finish(mut self) -> ProfileSnapshot {
        self.finished = true;
        deactivate_run(self.id);
        self.active.snapshot()
    }
}

impl Drop for ProfileRun {
    fn drop(&mut self) {
        if !self.finished {
            deactivate_run(self.id);
        }
    }
}

fn deactivate_run(id: u64) {
    ACTIVE_RUN.with(|active_slot| {
        let mut active = active_slot.borrow_mut();
        if active.as_ref().is_some_and(|active| active.id == id) {
            *active = None;
        }
    });
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileRunStartError {
    Uninitialized,
    AlreadyActive,
    InvalidFilter(ProfileFilterValidationError),
}

impl fmt::Display for ProfileRunStartError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Uninitialized => f.write_str("profile registry is not initialized"),
            Self::AlreadyActive => f.write_str("a profile run is already active"),
            Self::InvalidFilter(error) => error.fmt(f),
        }
    }
}

impl Error for ProfileRunStartError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidFilter(error) => Some(error),
            Self::Uninitialized | Self::AlreadyActive => None,
        }
    }
}

struct ActiveRun {
    id: u64,
    started_at: Instant,
    registry: Arc<ProfileRegistry>,
    filter: ProfileFilter,
    collector: Mutex<ProfileCollector>,
}

impl ActiveRun {
    fn descriptor(
        &self,
        path: &'static str,
        expected: ProfileInstrumentKind,
    ) -> Option<ProfileDescriptor> {
        let descriptor = self.registry.descriptor(path).unwrap_or_else(|| {
            panic!("profile path `{path}` is not registered");
        });
        if descriptor.kind() != expected {
            panic!(
                "profile path `{path}` is registered as {}, not {}",
                descriptor.kind(),
                expected
            );
        }

        self.filter
            .enables_scope(descriptor.scope())
            .then_some(*descriptor)
    }

    fn snapshot(&self) -> ProfileSnapshot {
        let collector = self
            .collector
            .lock()
            .expect("profile collector lock should not be poisoned");
        collector.snapshot(&self.registry)
    }
}

fn active_run() -> Option<Arc<ActiveRun>> {
    ACTIVE_RUN.with(|active| active.borrow().clone())
}

pub(crate) fn record_counter(path: &'static str, amount: u64) {
    let Some(active) = active_run() else {
        return;
    };
    if active
        .descriptor(path, ProfileInstrumentKind::Counter)
        .is_none()
    {
        return;
    }

    active
        .collector
        .lock()
        .expect("profile collector lock should not be poisoned")
        .record_counter(path, amount);
}

pub fn record_gauge(path: &'static str, value: ProfileMeasurement) {
    let Some(active) = active_run() else {
        return;
    };
    if active
        .descriptor(path, ProfileInstrumentKind::Gauge)
        .is_none()
    {
        return;
    }

    active
        .collector
        .lock()
        .expect("profile collector lock should not be poisoned")
        .record_gauge(path, value);
}

pub fn record_duration(path: &'static str, elapsed: Duration) {
    let Some(active) = active_run() else {
        return;
    };
    if active
        .descriptor(path, ProfileInstrumentKind::Duration)
        .is_none()
    {
        return;
    }

    active
        .collector
        .lock()
        .expect("profile collector lock should not be poisoned")
        .record_duration(path, elapsed);
}

pub fn duration_enabled(path: &'static str) -> bool {
    let Some(active) = active_run() else {
        return false;
    };
    active
        .descriptor(path, ProfileInstrumentKind::Duration)
        .is_some()
}

pub fn record_keyed_counter(path: &'static str, key: impl AsRef<str>, amount: u64) {
    let Some(active) = active_run() else {
        return;
    };
    if active
        .descriptor(path, ProfileInstrumentKind::KeyedCounter)
        .is_none()
    {
        return;
    }

    active
        .collector
        .lock()
        .expect("profile collector lock should not be poisoned")
        .record_keyed_counter(path, key.as_ref(), amount);
}

pub fn record_keyed_duration(path: &'static str, key: impl AsRef<str>, elapsed: Duration) {
    let Some(active) = active_run() else {
        return;
    };
    if active
        .descriptor(path, ProfileInstrumentKind::KeyedDuration)
        .is_none()
    {
        return;
    }

    active
        .collector
        .lock()
        .expect("profile collector lock should not be poisoned")
        .record_keyed_duration(path, key.as_ref(), elapsed);
}

pub fn record_checkpoint(
    path: &'static str,
    label: impl Into<String>,
    values: Vec<ProfileCheckpointValue>,
) {
    let Some(active) = active_run() else {
        return;
    };
    if active
        .descriptor(path, ProfileInstrumentKind::CheckpointStream)
        .is_none()
    {
        return;
    }

    let elapsed = active.started_at.elapsed();
    active
        .collector
        .lock()
        .expect("profile collector lock should not be poisoned")
        .record_checkpoint(path, label.into(), elapsed, values);
}

pub fn memory_snapshot_enabled(path: &'static str) -> bool {
    let Some(active) = active_run() else {
        return false;
    };
    active
        .descriptor(path, ProfileInstrumentKind::MemorySnapshot)
        .is_some()
}

pub fn record_memory_snapshot(path: &'static str, snapshot: ProfileMemorySnapshot) {
    let Some(active) = active_run() else {
        return;
    };
    if active
        .descriptor(path, ProfileInstrumentKind::MemorySnapshot)
        .is_none()
    {
        return;
    }

    active
        .collector
        .lock()
        .expect("profile collector lock should not be poisoned")
        .record_memory_snapshot(path, snapshot);
}

pub fn timer(path: &'static str) -> ProfileTimer {
    let Some(active) = active_run() else {
        return ProfileTimer::disabled();
    };
    if active
        .descriptor(path, ProfileInstrumentKind::Duration)
        .is_none()
    {
        return ProfileTimer::disabled();
    }

    ProfileTimer {
        path,
        started_at: Some(Instant::now()),
    }
}

/// RAII duration recorder returned by [`timer`].
pub struct ProfileTimer {
    path: &'static str,
    started_at: Option<Instant>,
}

impl ProfileTimer {
    fn disabled() -> Self {
        Self {
            path: "",
            started_at: None,
        }
    }

    pub fn finish(mut self) -> Option<Duration> {
        let elapsed = self.started_at.take()?.elapsed();
        record_duration(self.path, elapsed);
        Some(elapsed)
    }
}

impl Drop for ProfileTimer {
    fn drop(&mut self) {
        if let Some(started_at) = self.started_at.take() {
            record_duration(self.path, started_at.elapsed());
        }
    }
}

#[derive(Debug, Default)]
struct ProfileCollector {
    counters: BTreeMap<&'static str, u64>,
    gauges: BTreeMap<&'static str, ProfileMeasurement>,
    durations: BTreeMap<&'static str, Duration>,
    keyed_counters: BTreeMap<&'static str, BTreeMap<String, u64>>,
    keyed_durations: BTreeMap<&'static str, BTreeMap<String, DurationStats>>,
    checkpoints: BTreeMap<&'static str, CheckpointStreamState>,
    memory_snapshots: BTreeMap<&'static str, ProfileMemorySnapshot>,
}

impl ProfileCollector {
    fn record_counter(&mut self, path: &'static str, amount: u64) {
        *self.counters.entry(path).or_default() += amount;
    }

    fn record_gauge(&mut self, path: &'static str, value: ProfileMeasurement) {
        self.gauges.insert(path, value);
    }

    fn record_duration(&mut self, path: &'static str, elapsed: Duration) {
        *self.durations.entry(path).or_default() += elapsed;
    }

    fn record_keyed_counter(&mut self, path: &'static str, key: &str, amount: u64) {
        *self
            .keyed_counters
            .entry(path)
            .or_default()
            .entry(key.to_string())
            .or_default() += amount;
    }

    fn record_keyed_duration(&mut self, path: &'static str, key: &str, elapsed: Duration) {
        self.keyed_durations
            .entry(path)
            .or_default()
            .entry(key.to_string())
            .or_default()
            .record(elapsed);
    }

    fn record_checkpoint(
        &mut self,
        path: &'static str,
        label: String,
        elapsed: Duration,
        values: Vec<ProfileCheckpointValue>,
    ) {
        self.checkpoints
            .entry(path)
            .or_default()
            .record(label, elapsed, values);
    }

    fn record_memory_snapshot(&mut self, path: &'static str, snapshot: ProfileMemorySnapshot) {
        self.memory_snapshots.insert(path, snapshot);
    }

    fn snapshot(&self, registry: &ProfileRegistry) -> ProfileSnapshot {
        let mut entries = Vec::new();

        for (path, count) in &self.counters {
            entries.push(
                ProfileEntryBuilder::new(registry, path).build(ProfileValue::Counter(*count)),
            );
        }
        for (path, gauge) in &self.gauges {
            entries.push(
                ProfileEntryBuilder::new(registry, path).build(ProfileValue::Gauge(gauge.clone())),
            );
        }
        for (path, elapsed) in &self.durations {
            entries.push(
                ProfileEntryBuilder::new(registry, path).build(ProfileValue::Duration(*elapsed)),
            );
        }
        for (path, counters) in &self.keyed_counters {
            let counters = counters
                .iter()
                .map(|(key, count)| ProfileKeyedCounter {
                    key: key.clone(),
                    count: *count,
                })
                .collect();
            entries.push(
                ProfileEntryBuilder::new(registry, path)
                    .build(ProfileValue::KeyedCounters(counters)),
            );
        }
        for (path, durations) in &self.keyed_durations {
            let durations = durations
                .iter()
                .map(|(key, duration)| ProfileKeyedDuration {
                    key: key.clone(),
                    count: duration.count,
                    total: duration.total,
                    max: duration.max,
                })
                .collect();
            entries.push(
                ProfileEntryBuilder::new(registry, path)
                    .build(ProfileValue::KeyedDurations(durations)),
            );
        }
        for (path, stream) in &self.checkpoints {
            entries.push(
                ProfileEntryBuilder::new(registry, path)
                    .build(ProfileValue::Checkpoints(stream.rows.clone())),
            );
        }
        for (path, snapshot) in &self.memory_snapshots {
            entries.push(
                ProfileEntryBuilder::new(registry, path)
                    .build(ProfileValue::MemorySnapshot(snapshot.clone())),
            );
        }

        ProfileSnapshot::new(entries)
    }
}

struct ProfileEntryBuilder {
    descriptor: ProfileDescriptor,
}

impl ProfileEntryBuilder {
    fn new(registry: &ProfileRegistry, path: &'static str) -> Self {
        let descriptor = registry
            .descriptor(path)
            .copied()
            .expect("recorded profile path should still be registered");
        Self { descriptor }
    }

    fn build(self, value: ProfileValue) -> crate::ProfileEntry {
        crate::ProfileEntry::new(self.descriptor, value)
    }
}

#[derive(Debug, Clone, Default)]
struct DurationStats {
    count: u64,
    total: Duration,
    max: Duration,
}

impl DurationStats {
    fn record(&mut self, elapsed: Duration) {
        self.count += 1;
        self.total += elapsed;
        self.max = self.max.max(elapsed);
    }
}

#[derive(Debug, Clone, Default)]
struct CheckpointStreamState {
    previous_elapsed: Duration,
    rows: Vec<ProfileCheckpoint>,
}

impl CheckpointStreamState {
    fn record(&mut self, label: String, elapsed: Duration, values: Vec<ProfileCheckpointValue>) {
        let phase_elapsed = elapsed.saturating_sub(self.previous_elapsed);
        self.previous_elapsed = elapsed;
        self.rows.push(ProfileCheckpoint {
            label,
            phase_elapsed,
            elapsed,
            values,
        });
    }
}
