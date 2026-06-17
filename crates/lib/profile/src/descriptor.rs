use std::{error::Error, fmt};

/// Validates the dotted path used to identify a registered profile item or selector.
pub fn validate_profile_path(path: &str) -> Result<(), ProfilePathError> {
    if path.is_empty() {
        return Err(ProfilePathError::new(path, "path must not be empty"));
    }

    for segment in path.split('.') {
        validate_profile_key(path, segment)?;
    }

    Ok(())
}

/// Validates a non-empty path segment or row value key.
pub fn validate_profile_key(path: &str, key: &str) -> Result<(), ProfilePathError> {
    if key.is_empty() {
        return Err(ProfilePathError::new(path, "segments must not be empty"));
    }

    if !key
        .bytes()
        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    {
        return Err(ProfilePathError::new(
            path,
            "segments must use lowercase ASCII letters, digits, or underscores",
        ));
    }

    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfilePathError {
    path: String,
    message: &'static str,
}

impl ProfilePathError {
    fn new(path: &str, message: &'static str) -> Self {
        Self {
            path: path.to_string(),
            message,
        }
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn message(&self) -> &'static str {
        self.message
    }
}

impl fmt::Display for ProfilePathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid profile path `{}`: {}", self.path, self.message)
    }
}

impl Error for ProfilePathError {}

/// The storage shape expected for one registered profile item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileInstrumentKind {
    Counter,
    Gauge,
    Duration,
    KeyedCounter,
    KeyedDuration,
    CheckpointStream,
}

impl fmt::Display for ProfileInstrumentKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Self::Counter => "counter",
            Self::Gauge => "gauge",
            Self::Duration => "duration",
            Self::KeyedCounter => "keyed counter",
            Self::KeyedDuration => "keyed duration",
            Self::CheckpointStream => "checkpoint stream",
        };
        f.write_str(text)
    }
}

/// Unit metadata used by renderers when turning a snapshot into report fields or tables.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileUnit {
    None,
    Count,
    Bytes,
    Duration,
    Percent,
}

/// Optional rendering hints for table-like profile values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ProfileReport {
    pub sort: Option<ProfileReportSort>,
    pub limit: Option<usize>,
}

impl ProfileReport {
    pub const fn new() -> Self {
        Self {
            sort: None,
            limit: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileReportSort {
    KeyAscending,
    CountDescending,
    TotalDurationDescending,
}

/// Declares one value column carried by a checkpoint stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProfileCheckpointColumn {
    pub key: &'static str,
    pub title: &'static str,
    pub unit: ProfileUnit,
}

impl ProfileCheckpointColumn {
    pub const fn new(key: &'static str, title: &'static str, unit: ProfileUnit) -> Self {
        Self { key, title, unit }
    }

    pub const fn count(key: &'static str, title: &'static str) -> Self {
        Self::new(key, title, ProfileUnit::Count)
    }

    pub const fn bytes(key: &'static str, title: &'static str) -> Self {
        Self::new(key, title, ProfileUnit::Bytes)
    }

    pub const fn duration(key: &'static str, title: &'static str) -> Self {
        Self::new(key, title, ProfileUnit::Duration)
    }
}

const EMPTY_CHECKPOINT_COLUMNS: &[ProfileCheckpointColumn] = &[];

/// Static declaration for one profile item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProfileDescriptor {
    path: &'static str,
    scope: &'static str,
    kind: ProfileInstrumentKind,
    unit: ProfileUnit,
    title: Option<&'static str>,
    report: ProfileReport,
    checkpoint_columns: &'static [ProfileCheckpointColumn],
}

impl ProfileDescriptor {
    pub const fn counter(path: &'static str, scope: &'static str) -> Self {
        Self::new(
            path,
            scope,
            ProfileInstrumentKind::Counter,
            ProfileUnit::Count,
        )
    }

    pub const fn gauge(path: &'static str, scope: &'static str, unit: ProfileUnit) -> Self {
        Self::new(path, scope, ProfileInstrumentKind::Gauge, unit)
    }

    pub const fn duration(path: &'static str, scope: &'static str) -> Self {
        Self::new(
            path,
            scope,
            ProfileInstrumentKind::Duration,
            ProfileUnit::Duration,
        )
    }

    pub const fn keyed_counter(path: &'static str, scope: &'static str) -> Self {
        Self::new(
            path,
            scope,
            ProfileInstrumentKind::KeyedCounter,
            ProfileUnit::Count,
        )
    }

    pub const fn keyed_duration(path: &'static str, scope: &'static str) -> Self {
        Self::new(
            path,
            scope,
            ProfileInstrumentKind::KeyedDuration,
            ProfileUnit::Duration,
        )
    }

    pub const fn checkpoint_stream(path: &'static str, scope: &'static str) -> Self {
        Self::new(
            path,
            scope,
            ProfileInstrumentKind::CheckpointStream,
            ProfileUnit::None,
        )
    }

    const fn new(
        path: &'static str,
        scope: &'static str,
        kind: ProfileInstrumentKind,
        unit: ProfileUnit,
    ) -> Self {
        Self {
            path,
            scope,
            kind,
            unit,
            title: None,
            report: ProfileReport::new(),
            checkpoint_columns: EMPTY_CHECKPOINT_COLUMNS,
        }
    }

    pub fn title(mut self, title: &'static str) -> Self {
        self.title = Some(title);
        self
    }

    pub fn report(mut self, report: ProfileReport) -> Self {
        self.report = report;
        self
    }

    pub fn checkpoint_columns(mut self, columns: &'static [ProfileCheckpointColumn]) -> Self {
        self.checkpoint_columns = columns;
        self
    }

    pub fn path(self) -> &'static str {
        self.path
    }

    pub fn scope(self) -> &'static str {
        self.scope
    }

    pub fn kind(self) -> ProfileInstrumentKind {
        self.kind
    }

    pub fn unit(self) -> ProfileUnit {
        self.unit
    }

    pub fn title_text(self) -> Option<&'static str> {
        self.title
    }

    pub fn report_hints(self) -> ProfileReport {
        self.report
    }

    pub fn checkpoint_columns_slice(self) -> &'static [ProfileCheckpointColumn] {
        self.checkpoint_columns
    }
}
