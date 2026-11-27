//! Individual observations from the paired trials.

use std::{collections::BTreeMap, fmt::Write as _};

use rg_report::{ReportAlign, ReportDocumentBuilder, ReportValue};

use super::{BenchmarkReport, action_label};
use crate::benchmark::scenario::Observation;

#[derive(Clone, Copy)]
pub(super) enum RawSample<'a> {
    Action(&'a Observation),
    Phase(f64),
}

impl<'a> RawSample<'a> {
    pub(super) fn append_section(
        document: ReportDocumentBuilder,
        report: &'a BenchmarkReport,
    ) -> ReportDocumentBuilder {
        // Pair by action and trial. Request retries stay under the action that triggered them so
        // unequal attempt counts cannot pull either server out of its pair.
        let mut samples = BTreeMap::<_, [Option<Self>; 2]>::new();
        for trial in &report.trials {
            let prefix = (trial.project.as_str(), trial.workload, trial.pair);
            for action in &trial.observations {
                samples.entry((prefix, action.action.as_str())).or_default()[trial.side] =
                    Some(Self::Action(action));
            }
            for (label, duration) in [
                ("Initialize", trial.initialize_ms),
                ("Open first file", trial.opened_ms),
                ("Server ready", trial.ready_ms),
                ("First result from launch", trial.first_answer_ms),
                ("Start editing from launch", trial.editing_started_ms),
                ("Cargo preparation", trial.prepare_ms),
            ] {
                if let Some(duration) = duration {
                    samples.entry((prefix, label)).or_default()[trial.side] =
                        Some(Self::Phase(duration));
                }
            }
        }

        document.section("raw_data", |section| {
            section.group("raw", "Raw data").untitled();
            section.table("observations", |table| {
                table.untitled()
                    .description_opt(Some("Each pair shows one trial per server. Expand the attempt count for request timings and retry outcomes.".into()))
                    .text_column("project").text_column("workload").count_column("pair")
                    .text_column("action").text_column("method").text_column("server")
                    .text_column("outcome")
                    .column_as("start", "Start (ms)", ReportAlign::Right, None)
                    .column_help("start", "Time from process launch to the editor action.")
                    .column_as("elapsed", "Elapsed (ms)", ReportAlign::Right, None)
                    .column_as("reply", "Final RTT (ms)", ReportAlign::Right, None)
                    .column_help("reply", "Send to receipt of the last request attempt, including server queueing.")
                    .count_column("attempts")
                    .filter_by("project").filter_by("workload").filter_by("action").filter_by("method")
                    .page_size(30);
                for (((project, workload, pair), action), sides) in samples {
                    let group = format!("{project}/{workload:?}/{pair}/{action}");
                    let method = sides.iter().find_map(|sample| match sample {
                        Some(Self::Action(answer)) => Some(answer.method.as_str()),
                        _ => None,
                    }).unwrap_or("Startup phase");
                    for (side, sample) in sides.into_iter().enumerate() {
                        table.row(|row| {
                            row.group(&group).text("project", project)
                                .text("workload", workload.label())
                                .hint("workload", report.workload_description(workload))
                                .value("pair", ReportValue::Count(u64::from(pair)))
                                .text("action", action_label(action)).text("method", method).text("server", report.server_label(side));
                            match sample {
                                Some(Self::Action(answer)) => {
                                    row.text("outcome", answer.status.label())
                                        .value("start", Self::precise_ms(answer.started_ms))
                                        .value("elapsed", Self::precise_ms(answer.latency()))
                                        .value("reply", Self::precise_ms(answer.final_rtt()))
                                        .value("attempts", ReportValue::count(answer.attempts.len()));
                                    if !answer.attempts.is_empty() {
                                        let mut details = String::new();
                                        for (index, attempt) in answer.attempts.iter().enumerate() {
                                            let _ = writeln!(details, "{}. {} · {:.3} → {:.3} ms (+{:.3} ms)",
                                                index + 1, attempt.status.label(), attempt.sent_ms,
                                                attempt.received_ms, attempt.received_ms - attempt.sent_ms);
                                        }
                                        row.details("attempts", details);
                                    }
                                }
                                Some(Self::Phase(duration)) => {
                                    row.text("outcome", "Recorded")
                                        .value("elapsed", Self::precise_ms(Some(duration)));
                                }
                                None => { row.text("outcome", "Not recorded"); }
                            }
                        });
                    }
                }
            });
        })
    }

    fn precise_ms(value: Option<f64>) -> ReportValue {
        value.map_or(ReportValue::Empty, |ms| {
            ReportValue::Float((ms * 1000.0).round() / 1000.0)
        })
    }
}
