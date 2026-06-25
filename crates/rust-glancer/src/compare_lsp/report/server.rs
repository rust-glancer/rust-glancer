//! Server readiness report section.

use std::time::Duration;

use serde::Serialize;

use crate::report::{ReportDocumentBuilder, ReportTableBuilder};

use super::duration_ms;

#[derive(Debug, Serialize)]
pub(crate) struct ServerReport {
    name: String,
    command: String,
    initialize_ms: f64,
}

impl ServerReport {
    pub(crate) fn capture(name: &str, command: &str, initialize_latency: Duration) -> Self {
        Self {
            name: name.to_string(),
            command: command.to_string(),
            initialize_ms: duration_ms(initialize_latency),
        }
    }

    pub(super) fn append_section(
        document: ReportDocumentBuilder,
        servers: &[Self],
    ) -> ReportDocumentBuilder {
        document.section("servers", |section| {
            section.group("summary", "Summary");
            section.table("servers", |table| {
                Self::configure_table(table);
                for server in servers {
                    server.append_row(table);
                }
            });
        })
    }

    fn configure_table(table: &mut ReportTableBuilder) {
        table
            .text_column("server")
            .text_column("command")
            .duration_column_as("initialize_ms", "Initialize");
    }

    fn append_row(&self, table: &mut ReportTableBuilder) {
        table.row(|row| {
            row.text("server", &self.name)
                .text("command", &self.command)
                .duration_ms("initialize_ms", self.initialize_ms);
        });
    }
}
