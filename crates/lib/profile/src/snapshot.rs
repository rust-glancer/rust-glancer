use std::time::Duration;

use crate::ProfileDescriptor;

/// Completed values collected by one profiling run.
#[derive(Debug, Clone, PartialEq)]
pub struct ProfileSnapshot {
    entries: Vec<ProfileEntry>,
}

impl ProfileSnapshot {
    pub(crate) fn new(entries: Vec<ProfileEntry>) -> Self {
        Self { entries }
    }

    pub fn entries(&self) -> &[ProfileEntry] {
        &self.entries
    }

    pub fn entry(&self, path: &str) -> Option<&ProfileEntry> {
        self.entries
            .iter()
            .find(|entry| entry.descriptor.path() == path)
    }

    pub fn counter(&self, path: &str) -> Option<u64> {
        match self.entry(path)?.value() {
            ProfileValue::Counter(value) => Some(*value),
            _ => None,
        }
    }

    pub fn duration(&self, path: &str) -> Option<Duration> {
        match self.entry(path)?.value() {
            ProfileValue::Duration(value) => Some(*value),
            _ => None,
        }
    }

    pub fn keyed_counter(&self, path: &str, key: &str) -> Option<u64> {
        match self.entry(path)?.value() {
            ProfileValue::KeyedCounters(counters) => counters
                .iter()
                .find(|counter| counter.key == key)
                .map(|counter| counter.count),
            _ => None,
        }
    }

    pub fn keyed_duration(&self, path: &str, key: &str) -> Option<&ProfileKeyedDuration> {
        match self.entry(path)?.value() {
            ProfileValue::KeyedDurations(durations) => {
                durations.iter().find(|duration| duration.key == key)
            }
            _ => None,
        }
    }

    pub fn checkpoints(&self, path: &str) -> Option<&[ProfileCheckpoint]> {
        match self.entry(path)?.value() {
            ProfileValue::Checkpoints(checkpoints) => Some(checkpoints),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProfileEntry {
    pub descriptor: ProfileDescriptor,
    pub value: ProfileValue,
}

impl ProfileEntry {
    pub(crate) fn new(descriptor: ProfileDescriptor, value: ProfileValue) -> Self {
        Self { descriptor, value }
    }

    pub fn descriptor(&self) -> ProfileDescriptor {
        self.descriptor
    }

    pub fn value(&self) -> &ProfileValue {
        &self.value
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ProfileValue {
    Counter(u64),
    Gauge(ProfileMeasurement),
    Duration(Duration),
    KeyedCounters(Vec<ProfileKeyedCounter>),
    KeyedDurations(Vec<ProfileKeyedDuration>),
    Checkpoints(Vec<ProfileCheckpoint>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileKeyedCounter {
    pub key: String,
    pub count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileKeyedDuration {
    pub key: String,
    pub count: u64,
    pub total: Duration,
    pub max: Duration,
}

impl ProfileKeyedDuration {
    pub fn average(&self) -> Duration {
        if self.count == 0 {
            return Duration::ZERO;
        }

        self.total / u32::try_from(self.count).unwrap_or(u32::MAX)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProfileCheckpoint {
    pub label: String,
    pub phase_elapsed: Duration,
    pub elapsed: Duration,
    pub values: Vec<ProfileCheckpointValue>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProfileCheckpointValue {
    pub key: String,
    pub value: ProfileMeasurement,
}

impl ProfileCheckpointValue {
    pub fn new(key: impl Into<String>, value: impl Into<ProfileMeasurement>) -> Self {
        Self {
            key: key.into(),
            value: value.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ProfileMeasurement {
    Empty,
    Count(u64),
    Integer(i64),
    Float(f64),
    Bool(bool),
    Bytes(u64),
    Duration(Duration),
    Text(String),
}

impl ProfileMeasurement {
    pub fn count(value: usize) -> Self {
        Self::Count(value as u64)
    }

    pub fn integer(value: i64) -> Self {
        Self::Integer(value)
    }

    pub fn float(value: f64) -> Self {
        Self::Float(value)
    }

    pub fn bool(value: bool) -> Self {
        Self::Bool(value)
    }

    pub fn bytes(value: usize) -> Self {
        Self::Bytes(value as u64)
    }

    pub fn optional_bytes(value: Option<usize>) -> Self {
        value.map(Self::bytes).unwrap_or(Self::Empty)
    }

    pub fn duration(value: Duration) -> Self {
        Self::Duration(value)
    }

    pub fn text(value: impl Into<String>) -> Self {
        Self::Text(value.into())
    }
}
