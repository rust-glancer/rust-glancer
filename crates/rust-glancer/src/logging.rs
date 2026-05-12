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
};
use tracing_subscriber::{EnvFilter, Layer, layer::Context, prelude::*, registry::LookupSpan};

const ENGINE_ID_ENV: &str = "RUST_GLANCER_ENGINE_ID";
const LOG_SCHEMA: &str = "rust-glancer-log/v1";

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
        .with(JsonLogLayer { component })
        .try_init()
        .ok();
}

#[derive(Debug, Clone)]
struct JsonLogLayer {
    component: LogComponent,
}

impl<S> Layer<S> for JsonLogLayer
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
{
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let metadata = event.metadata();
        let mut fields = JsonFields::default();
        event.record(&mut fields);

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
                fields
                    .message
                    .unwrap_or_else(|| metadata.name().to_string()),
            ),
        );
        log.insert("fields".to_string(), Value::Object(fields.values));

        let mut stderr = std::io::stderr().lock();
        if serde_json::to_writer(&mut stderr, &Value::Object(log)).is_ok() {
            let _ = writeln!(stderr);
        }
    }
}

#[derive(Default)]
struct JsonFields {
    message: Option<String>,
    values: Map<String, Value>,
}

impl JsonFields {
    fn record_value(&mut self, field: &Field, value: Value) {
        if field.name() == "message" {
            self.message = Some(Self::message_value(value));
        } else {
            self.values.insert(field.name().to_string(), value);
        }
    }

    fn message_value(value: Value) -> String {
        match value {
            Value::String(value) => value,
            value => value.to_string(),
        }
    }
}

impl Visit for JsonFields {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.record_value(field, Value::String(format!("{value:?}")));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.record_value(field, Value::String(value.to_string()));
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.record_value(field, Value::Bool(value));
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.record_value(field, json!(value));
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.record_value(field, json!(value));
    }

    fn record_f64(&mut self, field: &Field, value: f64) {
        self.record_value(field, json!(value));
    }
}
