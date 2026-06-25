use crate::report::ReportValue;

pub(super) fn format_value(value: &ReportValue) -> String {
    match value {
        ReportValue::Text(value) => value.clone(),
        ReportValue::Count(value) => value.to_string(),
        ReportValue::Integer(value) => value.to_string(),
        ReportValue::Float(value) => value.to_string(),
        ReportValue::Bool(value) => value.to_string(),
        ReportValue::Bytes(value) => usize::try_from(*value)
            .map(format_bytes)
            .unwrap_or_else(|_| format!("{value} B")),
        ReportValue::BytesDelta(value) => format_byte_delta(*value),
        ReportValue::DurationMs(value) => format_duration_ms(*value),
        ReportValue::Percent(value) => format!("{value:.1}%"),
        ReportValue::Empty => "-".to_string(),
    }
}

fn format_byte_delta(delta: i64) -> String {
    let prefix = if delta >= 0 { "+" } else { "-" };
    let Some(bytes) = usize::try_from(delta.unsigned_abs()).ok() else {
        return format!("{delta} B");
    };

    format!("{prefix}{}", format_bytes(bytes))
}

fn format_bytes(bytes: usize) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];

    let mut value = bytes as f64;
    let mut unit = UNITS[0];
    for next_unit in UNITS.iter().skip(1) {
        if value < 1024.0 {
            break;
        }
        value /= 1024.0;
        unit = next_unit;
    }

    if unit == "B" {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {unit}")
    }
}

fn format_duration_ms(ms: f64) -> String {
    if ms < 1000.0 {
        format!("{ms:.0} ms")
    } else {
        format!("{:.2} s", ms / 1000.0)
    }
}
