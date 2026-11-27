use serde::Serialize;

/// Typed report value.
///
/// Renderers need to know whether `123` is bytes, a count, or just a number. Keep that here so we
/// do not format values into strings too early.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
#[allow(dead_code)]
pub enum ReportValue {
    Text(String),
    Link { label: String, url: String },
    Count(u64),
    Integer(i64),
    Float(f64),
    Bool(bool),
    Bytes(u64),
    BytesDelta(i64),
    DurationMs(f64),
    Percent(f64),
    Empty,
}

impl ReportValue {
    pub fn text(value: impl Into<String>) -> Self {
        Self::Text(value.into())
    }

    pub fn count(value: usize) -> Self {
        Self::Count(value as u64)
    }

    pub fn bytes(value: usize) -> Self {
        Self::Bytes(value as u64)
    }
}

/// Text alignment for a column.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)]
pub enum ReportAlign {
    Left,
    Right,
    Center,
}

/// Unit for values and columns.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)]
pub enum ReportUnit {
    Bytes,
    Duration,
    Percent,
    Count,
}
