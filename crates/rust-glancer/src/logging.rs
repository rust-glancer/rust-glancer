//! Process logging setup for the CLI and editor-facing modes.
//!
//! The standalone `analyze` command uses normal human-readable tracing output on stderr so it stays
//! useful from a terminal. The LSP server and its engine subprocesses instead emit newline-delimited
//! JSON records on stderr. The VS Code extension reads those records from the language client output
//! stream, verifies the schema, and maps the event level/message/fields into its
//! `Rust Glancer Language Server` log channel.

use std::{fmt, io::Write as _};

use serde_json::{Map, Value, json};
use tracing::{
    Event, Subscriber,
    field::{Field, Visit},
    span::{Attributes, Id, Record},
};
use tracing_subscriber::{EnvFilter, Layer, layer::Context, prelude::*, registry::LookupSpan};

const ENGINE_ID_ENV: &str = "RUST_GLANCER_ENGINE_ID";
const LOG_SCHEMA: &str = "rust-glancer-log/v1";
const RUST_GLANCER_SPAN_FIELD_PREFIX: &str = "rg.";

/// Identifies which process emitted an LSP-mode log line.
///
/// The VS Code extension uses this compact label to split server and per-engine logs without
/// teaching the Rust side about editor output channels.
#[derive(Debug, Clone)]
pub(crate) enum LogComponent {
    Server,
    Engine { id: String },
}

impl LogComponent {
    pub(crate) fn engine_from_env() -> Self {
        let id = std::env::var(ENGINE_ID_ENV)
            .ok()
            .filter(|id| !id.is_empty())
            .unwrap_or_else(|| "unknown".to_string());

        Self::Engine { id }
    }
}

/// Initializes the logger in a human-readable form.
pub(crate) fn init_plain_tracing() {
    let filter = EnvFilter::try_from_env("RUST_GLANCER_LOG")
        .unwrap_or_else(|_| EnvFilter::new("info,tarpc=warn"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .try_init()
        .ok();
}

/// Initializes the structured JSON logger consumed by the editor extension.
pub(crate) fn init_lsp_tracing(component: LogComponent) {
    let filter = EnvFilter::try_from_env("RUST_GLANCER_LOG")
        .unwrap_or_else(|_| EnvFilter::new("info,tarpc=warn"));
    tracing_subscriber::registry()
        .with(filter)
        .with(JsonLogLayer {
            component,
            writer: StderrLogWriter,
        })
        .try_init()
        .ok();
}

/// Emits one editor-facing JSON log record for each tracing event.
///
/// The layer keeps `rg.*` span fields in span extensions, then merges the active span stack into
/// each event so request context from `#[tracing::instrument]` reaches the VS Code output parser
/// without also inheriting framework-internal span fields from dependencies.
#[derive(Clone)]
struct JsonLogLayer<W = StderrLogWriter> {
    component: LogComponent,
    writer: W,
}

impl<S, W> Layer<S> for JsonLogLayer<W>
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
    W: LogWriter,
{
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(id) else {
            return;
        };

        let mut fields = JsonSpanFields::default();
        attrs.record(&mut JsonVisitor::new(&mut fields));
        span.extensions_mut().insert(fields);
    }

    fn on_record(&self, id: &Id, values: &Record<'_>, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(id) else {
            return;
        };

        let mut extensions = span.extensions_mut();
        let fields = match extensions.get_mut::<JsonSpanFields>() {
            Some(fields) => fields,
            None => {
                extensions.insert(JsonSpanFields::default());
                extensions
                    .get_mut::<JsonSpanFields>()
                    .expect("span fields were inserted into extensions")
            }
        };
        values.record(&mut JsonVisitor::new(fields));
    }

    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        let metadata = event.metadata();
        let mut event_fields = JsonEventFields::default();
        event.record(&mut JsonVisitor::new(&mut event_fields));

        // Span fields carry the request context produced by `#[tracing::instrument]`. We merge
        // them from outermost to innermost so deeper spans refine broader context, then let event
        // fields win because they describe the exact log site.
        let mut fields = Map::new();
        if let Some(scope) = ctx.event_scope(event) {
            for span in scope.from_root() {
                let extensions = span.extensions();
                if let Some(span_fields) = extensions.get::<JsonSpanFields>() {
                    merge_fields(&mut fields, &span_fields.values);
                }
            }
        }
        merge_fields(&mut fields, &event_fields.values);

        let mut log = Map::new();
        log.insert("schema".to_string(), Value::String(LOG_SCHEMA.to_string()));
        log.insert(
            "level".to_string(),
            Value::String(metadata.level().to_string()),
        );
        log.insert(
            "target".to_string(),
            Value::String(metadata.target().to_string()),
        );
        match &self.component {
            LogComponent::Server => {
                log.insert("component".to_string(), Value::String("server".to_string()));
            }
            LogComponent::Engine { id } => {
                log.insert("component".to_string(), Value::String("engine".to_string()));
                log.insert("engine".to_string(), Value::String(id.clone()));
            }
        }
        log.insert(
            "message".to_string(),
            Value::String(
                event_fields
                    .message
                    .unwrap_or_else(|| metadata.name().to_string()),
            ),
        );
        log.insert("fields".to_string(), Value::Object(fields));

        self.writer.write_log(&Value::Object(log));
    }
}

trait LogWriter: Clone + Send + Sync + 'static {
    fn write_log(&self, log: &Value);
}

#[derive(Clone, Copy)]
struct StderrLogWriter;

impl LogWriter for StderrLogWriter {
    fn write_log(&self, log: &Value) {
        let mut line = Vec::new();
        if serde_json::to_writer(&mut line, log).is_ok() {
            line.push(b'\n');
            let mut stderr = std::io::stderr().lock();
            let _ = stderr.write_all(&line);
        }
    }
}

fn merge_fields(to: &mut Map<String, Value>, from: &Map<String, Value>) {
    for (key, value) in from {
        to.insert(key.clone(), value.clone());
    }
}

#[derive(Default)]
struct JsonEventFields {
    message: Option<String>,
    values: Map<String, Value>,
}

impl JsonEventFields {
    fn message_value(value: Value) -> String {
        match value {
            Value::String(value) => value,
            value => value.to_string(),
        }
    }
}

impl JsonFieldSink for JsonEventFields {
    fn record_value(&mut self, field: &Field, value: Value) {
        if field.name() == "message" {
            self.message = Some(Self::message_value(value));
        } else {
            self.values.insert(field.name().to_string(), value);
        }
    }
}

#[derive(Default)]
struct JsonSpanFields {
    values: Map<String, Value>,
}

impl JsonFieldSink for JsonSpanFields {
    fn record_value(&mut self, field: &Field, value: Value) {
        if let Some(name) = field.name().strip_prefix(RUST_GLANCER_SPAN_FIELD_PREFIX) {
            self.values.insert(name.to_string(), value);
        }
    }
}

trait JsonFieldSink {
    fn record_value(&mut self, field: &Field, value: Value);
}

struct JsonVisitor<'a, T> {
    sink: &'a mut T,
}

impl<'a, T> JsonVisitor<'a, T> {
    fn new(sink: &'a mut T) -> Self {
        Self { sink }
    }
}

impl<T> Visit for JsonVisitor<'_, T>
where
    T: JsonFieldSink,
{
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.sink
            .record_value(field, Value::String(format!("{value:?}")));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.sink
            .record_value(field, Value::String(value.to_string()));
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.sink.record_value(field, Value::Bool(value));
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.sink.record_value(field, json!(value));
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.sink.record_value(field, json!(value));
    }

    fn record_f64(&mut self, field: &Field, value: f64) {
        self.sink.record_value(field, json!(value));
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use serde_json::Value;
    use tracing_subscriber::prelude::*;

    use super::*;

    #[derive(Clone)]
    struct BufferLogWriter {
        lines: Arc<Mutex<Vec<String>>>,
    }

    impl LogWriter for BufferLogWriter {
        fn write_log(&self, log: &Value) {
            self.lines
                .lock()
                .expect("test log buffer should not be poisoned")
                .push(serde_json::to_string(log).expect("test log should serialize"));
        }
    }

    #[test]
    fn lsp_logs_include_active_span_fields() {
        let lines = Arc::new(Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::registry().with(JsonLogLayer {
            component: LogComponent::Server,
            writer: BufferLogWriter {
                lines: Arc::clone(&lines),
            },
        });

        tracing::subscriber::with_default(subscriber, || {
            let span = tracing::info_span!(
                "hover",
                rg.position = ?(12_u32, 4_u32),
                rg.method = "textDocument/hover",
                otel.kind = "server"
            );
            let _guard = span.enter();

            tracing::info!(result_count = 1_u64, "hover request answered");
        });

        let lines = lines
            .lock()
            .expect("test log buffer should not be poisoned");
        assert_eq!(lines.len(), 1);

        let record: Value =
            serde_json::from_str(&lines[0]).expect("structured log line should be JSON");
        let fields = record["fields"]
            .as_object()
            .expect("structured log should contain object fields");
        assert_eq!(record["message"], "hover request answered");
        assert_eq!(fields["method"], "textDocument/hover");
        assert_eq!(fields["position"], "(12, 4)");
        assert_eq!(fields["result_count"], 1);
        assert!(!fields.contains_key("otel.kind"));
    }
}
