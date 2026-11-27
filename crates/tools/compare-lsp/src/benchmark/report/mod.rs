//! Render the same recorded trials as machine-readable observations or an engineering dashboard.

mod raw;

use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{BufReader, BufWriter, Write as _},
    path::Path,
};

use anyhow::Context as _;
use gen_lsp_types::ClientCapabilities;
use rg_report::{
    HtmlRenderer, ReportDocument, ReportDocumentBuilder, ReportValue, RichJsonRenderer,
    TextRenderer,
};
use serde::{Deserialize, Serialize};

use super::{
    Binary, Comparison, Mode, Trial, Workload,
    fixture::Target,
    scenario::{Observation, Status},
};
use crate::{OutputFormat, server::RunningServer};

const SCHEMA_VERSION: u32 = 4;
const PLANNED_ACTIONS_SCHEMA: u32 = 4;

fn action_label(action: &str) -> Cow<'_, str> {
    // Saved runs store the label used at capture time.
    if action.contains("viewport") || action.contains("Viewport") {
        action
            .replace("Viewport", "Editor")
            .replace("viewport", "editor")
            .into()
    } else {
        action.into()
    }
}

type TrialsByWorkload<'a> = BTreeMap<(&'a str, Workload), Vec<&'a Trial>>;

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct BenchmarkReport {
    schema_version: u32,
    comparison: Comparison,
    mode: Mode,
    pairs: u32,
    toolchain: String,
    platform: String,
    logical_cpus: Option<usize>,
    argv: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    client_capabilities: Option<ClientCapabilities>,
    pub(super) targets: Vec<Target>,
    pub(super) binaries: Vec<Binary>,
    pub(super) trials: Vec<Trial>,
    pub(super) errors: Vec<String>,
}

impl BenchmarkReport {
    pub(super) fn new(
        comparison: Comparison,
        mode: Mode,
        pairs: u32,
        toolchain: String,
        targets: Vec<Target>,
    ) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            comparison,
            mode,
            pairs,
            toolchain,
            platform: format!("{} {}", std::env::consts::OS, std::env::consts::ARCH),
            logical_cpus: std::thread::available_parallelism().ok().map(usize::from),
            argv: std::env::args_os()
                .map(|argument| argument.to_string_lossy().into_owned())
                .collect(),
            client_capabilities: Some(RunningServer::client_capabilities()),
            targets,
            binaries: Vec::new(),
            trials: Vec::new(),
            errors: Vec::new(),
        }
    }

    pub(super) fn read(path: &Path) -> anyhow::Result<Self> {
        let report: Self = serde_json::from_reader(BufReader::new(fs::File::open(path)?))
            .with_context(|| format!("read benchmark observations {}", path.display()))?;
        anyhow::ensure!(
            (1..=SCHEMA_VERSION).contains(&report.schema_version),
            "unsupported benchmark schema {}",
            report.schema_version
        );
        anyhow::ensure!(
            report.binaries.len() == 2 && report.trials.iter().all(|trial| trial.side < 2),
            "invalid benchmark server identities"
        );
        Ok(report)
    }

    pub(super) fn write(
        &self,
        directory: &Path,
        format: Option<OutputFormat>,
    ) -> anyhow::Result<()> {
        let raw = directory.join("observations.json");
        let mut writer = BufWriter::new(fs::File::create(&raw)?);
        serde_json::to_writer_pretty(&mut writer, self)?;
        writer.flush()?;

        let document = self.document();
        let html = directory.join("report.html");
        fs::write(&html, HtmlRenderer.render(&document))?;
        match format.unwrap_or(OutputFormat::Html) {
            OutputFormat::Html => println!("{}", html.display()),
            OutputFormat::Json => {
                std::io::copy(&mut fs::File::open(&raw)?, &mut std::io::stdout().lock())?;
                println!();
            }
            OutputFormat::RichJson => println!("{}", RichJsonRenderer.render(&document)?),
            OutputFormat::Text => {
                let mut text = String::new();
                TextRenderer
                    .render(&document, &mut text)
                    .context("render benchmark summary")?;
                print!("{text}");
            }
        }
        Ok(())
    }

    fn duration(value: Option<f64>) -> ReportValue {
        value.map_or(ReportValue::Empty, ReportValue::DurationMs)
    }

    fn faster_by_ten_percent([first, second]: [Option<f64>; 2]) -> Option<usize> {
        let (first, second) = (first?, second?);
        if second > 0.0 && first <= second * 0.9 {
            Some(0)
        } else if first > 0.0 && second <= first * 0.9 {
            Some(1)
        } else {
            None
        }
    }

    fn server_label(&self, side: usize) -> &str {
        match self.comparison {
            Comparison::Versions => ["Old", "New"][side],
            Comparison::RustAnalyzer => &self.binaries[side].label,
        }
    }

    fn workload_description(&self, workload: Workload) -> &'static str {
        match (self.schema_version, workload) {
            (1, Workload::UserSimulation) => {
                "After the server reports readiness, navigate existing code, add a module, and test it with paced typing and completions."
            }
            (2, Workload::UserSimulation) => {
                "Start at file open. Request editor hints, hover, and navigation on a fixed schedule, then add a module and test it with paced typing and completions."
            }
            _ => workload.description(),
        }
    }

    fn document(&self) -> ReportDocument {
        let mut groups = TrialsByWorkload::new();
        for trial in &self.trials {
            groups
                .entry((trial.project.as_str(), trial.workload))
                .or_default()
                .push(trial);
        }
        let trial_label = if self.pairs == 1 { "trial" } else { "trials" };
        let document = ReportDocument::builder("lsp_benchmark")
            .title("LSP benchmark")
            .subtitle(format!("{} paired {trial_label} per workload", self.pairs));
        let document = self.append_summary(document, &groups);
        let document = if self.comparison == Comparison::Versions {
            self.append_changes(document, &groups)
        } else {
            document
        };
        let document = self.append_actions(document, &groups);
        let document = raw::RawSample::append_section(document, self);
        let document = self.append_problems(document, &groups);
        self.append_run_details(document).build()
    }

    fn append_summary(
        &self,
        document: ReportDocumentBuilder,
        groups: &TrialsByWorkload<'_>,
    ) -> ReportDocumentBuilder {
        document.section("summary", |section| {
            section.group("summary", "Summary").untitled();
            if self.comparison == Comparison::Versions {
                section.title("Projects");
            }
            section.table("projects", |table| {
                table
                    .untitled()
                    .text_column("project")
                    .text_column("workload")
                    .text_column("server")
                    .text_column("usable_actions")
                    .column_help("usable_actions", if self.schema_version >= PLANNED_ACTIONS_SCHEMA {
                        "Usable results out of planned actions."
                    } else {
                        "Usable results out of recorded actions."
                    })
                    .duration_column_as("answer", "First result")
                    .column_help("answer", "Median time from server launch to the first usable foreground result. Workloads that start after indexing omit startup wait.")
                    .duration_column_as("ready", "Server ready")
                    .column_help("ready", "Median time from launch until the server reports that initial indexing is finished. It may answer requests before reporting readiness.")
                    .duration_column_as("runtime", "Run time")
                    .column_help("runtime", "Median time spent waiting for actions during the measured workflow. For editing workloads that start after readiness, this covers the post-ready actions. Overlapping waits count once. Green marks a timing at least 10% lower.")
                    .filter_by("project")
                    .filter_by("workload");
                for ((project, workload), trials) in groups {
                    let paired = PairedTrials::new(trials);
                    let answers = paired.medians(|trial| trial.first_answer_ms);
                    let runtimes = paired.medians(Trial::run_time);
                    let ready = paired.medians(|trial| trial.ready_ms);
                    let faster_answer = Self::faster_by_ten_percent(answers);
                    // A missing result can end a request sooner, so require completed
                    // foreground actions before highlighting a replay's shorter wait.
                    let comparable = trials.iter().all(|trial| trial.foreground_complete());
                    let faster_runtime = comparable.then(|| Self::faster_by_ten_percent(runtimes)).flatten();
                    let faster_ready = Self::faster_by_ten_percent(ready);
                    // New trials save a slot for each planned action. Read the number
                    // from the recording so it also applies to missing trials and rerenders.
                    let planned = (self.schema_version >= PLANNED_ACTIONS_SCHEMA).then(|| {
                        trials.iter().map(|trial| trial.observations.len()).max().unwrap_or(0)
                            * self.pairs as usize
                    });
                    for side in 0..self.binaries.len() {
                        let observations = trials.iter().filter(|trial| trial.side == side)
                            .flat_map(|trial| &trial.observations);
                        let total = planned.unwrap_or_else(|| observations.clone().count());
                        let usable = observations.filter(|answer| answer.status == Status::Usable).count();
                        table.row(|row| {
                            row.group(format!("{project}/{workload:?}"))
                                .text("project", *project)
                                .text("workload", workload.label())
                                .hint("workload", self.workload_description(*workload))
                                .text("server", self.server_label(side))
                                .text("usable_actions", format!("{usable}/{total}"))
                                .value("answer", Self::duration(answers[side]))
                                .value("ready", Self::duration(ready[side]))
                                .value("runtime", Self::duration(runtimes[side]));
                            if faster_answer == Some(side) {
                                row.highlight("answer");
                            }
                            if faster_runtime == Some(side) {
                                row.highlight("runtime");
                            }
                            if faster_ready == Some(side) {
                                row.highlight("ready");
                            }
                            if runtimes[side].is_none() {
                                row.hint("runtime", "No paired timing for this workload. See Problems.");
                            }
                        });
                    }
                }
            });
        })
    }

    fn append_changes(
        &self,
        document: ReportDocumentBuilder,
        groups: &TrialsByWorkload<'_>,
    ) -> ReportDocumentBuilder {
        document.section("changes", |section| {
            section.group("summary", "Summary").title("Paired changes");
            section.table("paired_changes", |table| {
                table
                    .untitled()
                    .text_column("project")
                    .text_column("workload")
                    .text_column("metric")
                    .count_column("pairs")
                    .duration_column_as("change", "New minus old (median)")
                    .text_column("assessment");
                for ((project, workload), trials) in groups {
                    let paired = PairedTrials::new(trials);
                    for (metric, select, require_complete) in [
                        (
                            "First result",
                            (|trial: &Trial| trial.first_answer_ms) as fn(&Trial) -> Option<f64>,
                            false,
                        ),
                        ("Server ready", |trial: &Trial| trial.ready_ms, false),
                        ("Run time", Trial::run_time, true),
                    ] {
                        let change = paired.change(|trial| {
                            if require_complete && !trial.foreground_complete() {
                                return None;
                            }
                            select(trial)
                        });
                        if let Some(change) = change {
                            table.row(|row| {
                                row.text("project", *project)
                                    .text("workload", workload.label())
                                    .hint("workload", self.workload_description(*workload))
                                    .text("metric", metric)
                                    .value("pairs", ReportValue::count(change.pairs))
                                    .duration_ms("change", change.median)
                                    .text("assessment", change.assessment);
                            });
                        }
                    }
                    // The startup hover is already represented by the first result above.
                    if workload.is_startup() {
                        continue;
                    }
                    let actions: BTreeSet<_> = trials
                        .iter()
                        .flat_map(|trial| trial.run_actions())
                        .map(|answer| answer.action.as_str())
                        .collect();
                    let mut changes = actions
                        .into_iter()
                        .filter_map(|action| {
                            let change = paired.change(|trial| {
                                trial
                                    .run_actions()
                                    .find(|answer| {
                                        answer.action == action && answer.status == Status::Usable
                                    })
                                    .and_then(|answer| answer.latency())
                            })?;
                            Some((action, change))
                        })
                        .collect::<Vec<_>>();
                    changes.sort_by(|left, right| {
                        right.1.median.abs().total_cmp(&left.1.median.abs())
                    });
                    for (action, change) in changes.into_iter().take(3) {
                        table.row(|row| {
                            row.text("project", *project)
                                .text("workload", workload.label())
                                .hint("workload", self.workload_description(*workload))
                                .text("metric", action_label(action))
                                .value("pairs", ReportValue::count(change.pairs))
                                .duration_ms("change", change.median)
                                .text("assessment", change.assessment);
                        });
                    }
                }
            });
        })
    }

    fn append_actions(
        &self,
        document: ReportDocumentBuilder,
        groups: &TrialsByWorkload<'_>,
    ) -> ReportDocumentBuilder {
        document.section("actions", |section| {
            section.group("actions", "Actions").untitled();
            section.table("request_results", |table| {
                table
                    .untitled()
                    .text_column("project")
                    .text_column("workload")
                    .text_column("server")
                    .text_column("action")
                    .text_column("usable")
                    .duration_column_as("median", "Median wait")
                    .column_help("median", "Elapsed time from trigger to usable response, including retries. Timing columns use pairs with a usable reply from both servers. Green marks a median at least 10% lower when all pairs qualify.")
                    .duration_column_as("reply", "Median final reply")
                    .column_help("reply", "Elapsed time from dispatch to receipt of the final usable LSP attempt, including server queueing.")
                    .duration_column_as("slowest", "Slowest usable")
                    .count_column("retries")
                    .column_help("retries", "Additional LSP attempts after the first, across all observed trials.")
                    .count_column("items")
                    .filter_by("project")
                    .filter_by("action")
                    .description_opt(Some("Click a column heading to sort pairs by the first server’s value.".into()));
                for ((project, workload), trials) in groups {
                    let paired = PairedTrials::new(trials);
                    let mut actions = BTreeMap::<_, [Vec<_>; 2]>::new();
                    for trial in trials {
                        for answer in &trial.observations {
                            actions.entry(answer.action.as_str()).or_default()[trial.side].push(answer);
                        }
                    }
                    for (action, sides) in actions {
                        let group = format!("{project}/{workload:?}/{action}");
                        let tokens = sides.iter().flatten().any(|answer| answer.method == "textDocument/semanticTokens/range");
                        let usable = sides.each_ref().map(|answers| {
                            answers.iter().filter(|answer| answer.status == Status::Usable).count()
                        });
                        let matched: Vec<_> = paired.action_samples(action).collect();
                        let waits = [0, 1].map(|side| matched.iter().filter_map(|pair| pair[side].latency()).collect::<Vec<_>>());
                        let medians = waits.each_ref().map(|values| median(values.iter().copied()));
                        let faster = if !tokens && matched.len() == self.pairs as usize {
                            Self::faster_by_ten_percent(medians)
                        } else {
                            None
                        };
                        for (side, answers) in sides.into_iter().enumerate() {
                            let reply = median(matched.iter().filter_map(|[first, second]| {
                                Some([first.final_rtt()?, second.final_rtt()?][side])
                            }));
                            let retries = answers.iter().map(|answer| answer.attempts.len().saturating_sub(1)).sum();
                            let items = median(matched.iter().filter_map(|[first, second]| {
                                Some([first.result_items()?, second.result_items()?][side] as f64)
                            }))
                                .map_or(ReportValue::Empty, ReportValue::Float);
                            let total = if self.schema_version >= PLANNED_ACTIONS_SCHEMA {
                                self.pairs as usize
                            } else {
                                answers.len()
                            };
                            table.row(|row| {
                                row.group(&group)
                                    .text("project", *project)
                                    .text("workload", workload.label())
                                    .hint("workload", self.workload_description(*workload))
                                    .text("server", self.server_label(side))
                                    .text("action", action_label(action))
                                    .text("usable", format!("{}/{total}", usable[side]))
                                    .value("median", Self::duration(medians[side]))
                                    .value("reply", Self::duration(reply))
                                    .value("slowest", Self::duration(waits[side].iter().copied().reduce(f64::max)))
                                    .value("retries", ReportValue::count(retries))
                                    .value("items", items);
                                if matched.len() < self.pairs as usize {
                                    row.hint("median", format!("Usable replies on both servers in {} of {} pairs.", matched.len(), self.pairs));
                                }
                                if tokens {
                                    row.hint("action", "Highlighting scope differs. Rust Glancer returns documentation and format-string tokens. Rust-analyzer highlights Rust code, including semantic symbol and type information.");
                                }
                                if faster == Some(side) {
                                    row.highlight("median");
                                }
                            });
                        }
                    }
                }
            });
        })
    }

    fn append_problems(
        &self,
        document: ReportDocumentBuilder,
        groups: &TrialsByWorkload<'_>,
    ) -> ReportDocumentBuilder {
        let mut issues = BTreeMap::<_, BTreeSet<_>>::new();
        for ((project, workload), trials) in groups {
            for side in 0..self.binaries.len() {
                let recorded: BTreeSet<_> = trials
                    .iter()
                    .filter(|trial| trial.side == side)
                    .map(|trial| trial.pair)
                    .collect();
                for pair in (1..=self.pairs).filter(|pair| !recorded.contains(pair)) {
                    issues
                        .entry((*project, *workload, side, "Trial", "Missing"))
                        .or_default()
                        .insert(pair);
                }
            }
            for trial in trials {
                if let Some(error) = &trial.ready_error {
                    issues
                        .entry((
                            *project,
                            *workload,
                            trial.side,
                            "Server ready",
                            error.as_str(),
                        ))
                        .or_default()
                        .insert(trial.pair);
                }
                for answer in trial
                    .observations
                    .iter()
                    .filter(|answer| answer.status.is_problem())
                {
                    issues
                        .entry((
                            *project,
                            *workload,
                            trial.side,
                            answer.action.as_str(),
                            answer.status.label(),
                        ))
                        .or_default()
                        .insert(trial.pair);
                }
            }
        }
        let run_failed = !self.errors.is_empty() || self.trials.iter().any(Trial::failed);
        document.section("problems", |section| {
            section.group("problems", "Problems").untitled();
            if issues.is_empty() && !run_failed {
                section.paragraph("No problems detected.");
            }
            if !issues.is_empty() {
                section.table("outcomes", |table| {
                    table
                        .untitled()
                        .text_column("project")
                        .text_column("workload")
                        .text_column("server")
                        .text_column("action")
                        .text_column("outcome")
                        .text_column("affected_pairs");
                    for ((project, workload, side, action, outcome), pairs) in issues {
                        table.row(|row| {
                            row.text("project", project)
                                .text("workload", workload.label())
                                .text("server", self.server_label(side))
                                .text("action", action_label(action))
                                .text("outcome", outcome)
                                .text(
                                    "affected_pairs",
                                    pairs
                                        .into_iter()
                                        .map(|pair| pair.to_string())
                                        .collect::<Vec<_>>()
                                        .join(", "),
                                );
                        });
                    }
                });
            }
            if run_failed {
                section.table("run_errors", |table| {
                    table
                        .title("Run errors")
                        .text_column("trial")
                        .text_column("error");
                    for error in &self.errors {
                        table.row(|row| {
                            row.text("trial", "Setup or checkout").text("error", error);
                        });
                    }
                    for trial in &self.trials {
                        for error in trial.error.iter().chain(&trial.cleanup_error) {
                            table.row(|row| {
                                row.text(
                                    "trial",
                                    format!(
                                        "{} / {} / {} / {}",
                                        trial.project,
                                        trial.workload.label(),
                                        trial.pair,
                                        self.server_label(trial.side)
                                    ),
                                )
                                .text("error", error);
                            });
                        }
                    }
                });
            }
        })
    }

    fn append_run_details(&self, document: ReportDocumentBuilder) -> ReportDocumentBuilder {
        document.section("run_details", |section| {
            section.group("details", "Run details").untitled();
            section.fields("summary", |fields| {
                fields
                    .text("platform", &self.platform)
                    .text("toolchain", self.toolchain.lines().next().unwrap_or(""));
                if self.schema_version >= 4 && self.comparison == Comparison::RustAnalyzer {
                    fields.text(
                        "rust_analyzer_user_config",
                        "Empty configuration directory supplied per trial",
                    );
                }
            });
            section.table("binaries", |table| {
                table.text_column("server").text_column("version");
                for (side, binary) in self.binaries.iter().enumerate() {
                    let version = binary
                        .version
                        .strip_prefix("rust-glancer ")
                        .or_else(|| binary.version.strip_prefix("rust-analyzer "))
                        .unwrap_or(&binary.version);
                    table.row(|row| {
                        row.text("server", self.server_label(side))
                            .text("version", version);
                    });
                }
            });
            section.table("revisions", |table| {
                table
                    .text_column("project")
                    .text_column("repository")
                    .text_column("revision");
                for target in &self.targets {
                    table.row(|row| {
                        row.text("project", &target.id)
                            .text("revision", &target.revision);
                        if let Some(repository) = target.repository_url() {
                            row.value(
                                "repository",
                                ReportValue::Link {
                                    label: repository.trim_start_matches("https://").to_owned(),
                                    url: repository.to_owned(),
                                },
                            );
                            if target.revision.len() == 40
                                && target.revision.bytes().all(|byte| byte.is_ascii_hexdigit())
                            {
                                row.value(
                                    "revision",
                                    ReportValue::Link {
                                        label: target.revision.clone(),
                                        url: format!("{repository}/commit/{}", target.revision),
                                    },
                                );
                            }
                        }
                    });
                }
            });
            section.table("profiles", |table| {
                table
                    .text_column("server")
                    .text_column("workload")
                    .text_column("settings");
                let mut seen = BTreeSet::new();
                for trial in &self.trials {
                    if seen.insert((trial.side, trial.workload)) {
                        table.row(|row| {
                            row.text("server", self.server_label(trial.side))
                                .text("workload", trial.workload.label())
                                .text("settings", trial.options.to_string());
                        });
                    }
                }
            });
        })
    }
}

impl Trial {
    fn run_actions(&self) -> impl Iterator<Item = &Observation> {
        self.observations.iter().filter(|answer| {
            // A blocked action has no timestamp, but it still belongs to the editing phase.
            self.editing_started_ms
                .is_none_or(|start| answer.started_ms.is_none_or(|sent| sent >= start))
        })
    }

    fn foreground_complete(&self) -> bool {
        self.run_actions()
            .all(|answer| !answer.foreground || !answer.status.is_problem())
    }

    fn run_time(&self) -> Option<f64> {
        if self.workload.is_startup() {
            // The startup workload ends with its first result. Readiness is a separate event.
            return self.first_answer_ms.map(|_| 0.0);
        }
        // Version 2 measured edits during startup and counted request wait after the first
        // result. The merged replay instead starts an editing phase after readiness.
        let first_result = if self.opened_ms.is_some() && self.editing_started_ms.is_none() {
            Some(self.first_answer_ms?)
        } else {
            None
        };
        let measured = self
            .run_actions()
            .filter(|answer| !self.workload.measures_foreground() || answer.foreground);
        let mut intervals = Vec::new();
        for answer in measured {
            if first_result.is_none() && answer.status.is_problem() {
                return None;
            }
            if let (Some(started), Some(ended)) = (answer.started_ms, answer.ended_ms) {
                intervals.push((started, ended));
            }
        }
        intervals.sort_by(|left, right| left.0.total_cmp(&right.0));
        // Several outstanding foreground requests can represent the same wait at the screen.
        // Sum their union, clipping any portion already accounted for by first result.
        let mut end = first_result.unwrap_or(0.0);
        let mut total = 0.0;
        for (started, ended) in &intervals {
            total += (ended - started.max(end)).max(0.0);
            end = end.max(*ended);
        }
        (!intervals.is_empty()).then_some(total)
    }
}

struct Change {
    median: f64,
    pairs: usize,
    assessment: &'static str,
}

struct PairedTrials<'a>(BTreeMap<u32, [Option<&'a Trial>; 2]>);

impl<'a> PairedTrials<'a> {
    fn new(trials: &[&'a Trial]) -> Self {
        let mut pairs = BTreeMap::<_, [Option<&Trial>; 2]>::new();
        for &trial in trials.iter().filter(|trial| !trial.failed()) {
            pairs.entry(trial.pair).or_default()[trial.side] = Some(trial);
        }
        Self(pairs)
    }

    fn select<T>(&self, select: impl Fn(&'a Trial) -> Option<T>) -> impl Iterator<Item = [T; 2]> {
        self.0
            .values()
            .filter_map(move |pair| Some([select(pair[0]?)?, select(pair[1]?)?]))
    }

    fn action_samples(&self, action: &str) -> impl Iterator<Item = [&'a Observation; 2]> {
        self.select(move |trial| {
            trial.observations.iter().find(|answer| {
                answer.action == action
                    && answer.status == Status::Usable
                    && answer.latency().is_some()
            })
        })
    }

    fn medians(&self, select: impl Fn(&Trial) -> Option<f64>) -> [Option<f64>; 2] {
        let matched = self.select(select).collect::<Vec<_>>();
        [0, 1].map(|side| median(matched.iter().map(|values| values[side])))
    }

    fn change(&self, select: impl Fn(&Trial) -> Option<f64>) -> Option<Change> {
        let selected = self.select(select).collect::<Vec<_>>();
        let mut deltas = selected
            .iter()
            .map(|[baseline, candidate]| candidate - baseline)
            .collect::<Vec<_>>();
        deltas.sort_by(f64::total_cmp);
        let typical = median_sorted(&deltas)?;
        let margin = (median(selected.iter().map(|[baseline, _]| *baseline))? * 0.05).max(5.0);
        // Invert the two-sided sign test for an order-statistic interval around the median.
        // With too few independent pairs, even the full observed range cannot give 95% coverage.
        let n = deltas.len();
        let mut probability = 0.5_f64.powi(n as i32);
        let mut tail = 0.0;
        let mut interval = None;
        for rank in 0..n / 2 {
            tail += probability;
            if tail > 0.025 {
                break;
            }
            interval = Some((deltas[rank], deltas[n - 1 - rank]));
            probability *= (n - rank) as f64 / (rank + 1) as f64;
        }
        let assessment = match interval {
            Some((lower, _)) if lower > margin => "Regression",
            Some((_, upper)) if upper < -margin => "Improvement",
            _ => "Inconclusive",
        };
        Some(Change {
            median: typical,
            pairs: n,
            assessment,
        })
    }
}

fn median(values: impl Iterator<Item = f64>) -> Option<f64> {
    let mut values = values.collect::<Vec<_>>();
    values.sort_by(f64::total_cmp);
    median_sorted(&values)
}

fn median_sorted(values: &[f64]) -> Option<f64> {
    let middle = values.len() / 2;
    let upper = *values.get(middle)?;
    Some(if values.len().is_multiple_of(2) {
        (values[middle - 1] + upper) / 2.0
    } else {
        upper
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{Observation, PairedTrials, Status, Trial, Workload};

    #[test]
    fn summary_wait_counts_each_interval_once() {
        for (workload, opened_ms, first_answer_ms, editing_started_ms, intervals, runtime) in [
            (
                Workload::Restart,
                None,
                Some(40.0),
                None,
                &[(10.0, 40.0)][..],
                0.0,
            ),
            (
                Workload::UserSimulation,
                Some(5.0),
                Some(40.0),
                None,
                &[(10.0, 40.0), (20.0, 55.0), (62.0, 70.0)],
                23.0,
            ),
            (
                Workload::UserSimulation,
                Some(5.0),
                Some(40.0),
                Some(60.0),
                &[(10.0, 40.0), (62.0, 72.0), (68.0, 80.0)],
                18.0,
            ),
            (
                Workload::Editor,
                None,
                None,
                None,
                &[(10.0, 40.0), (20.0, 55.0), (62.0, 70.0)],
                53.0,
            ),
        ] {
            let observations = intervals
                .iter()
                .map(|(start, end)| {
                    json!({
                        "action": "Hover",
                        "method": "textDocument/hover",
                        "foreground": true,
                        "started_ms": start,
                        "ended_ms": end,
                        "status": "usable",
                        "attempts": [],
                    })
                })
                .collect::<Vec<_>>();
            let trial: Trial = serde_json::from_value(json!({
                "project": "demo",
                "workload": workload,
                "side": 0,
                "pair": 1,
                "opened_ms": opened_ms,
                "first_answer_ms": first_answer_ms,
                "editing_started_ms": editing_started_ms,
                "observations": observations,
                "options": {},
            }))
            .expect("valid recorded trial");
            assert_eq!(trial.run_time(), Some(runtime), "{workload:?} run time");
        }
    }

    #[test]
    fn interrupted_trials_keep_planned_actions_and_match_pairs_by_id() {
        let trials: Vec<Trial> = [
            (1, 0, Status::Usable),
            (1, 1, Status::Missing),
            (2, 0, Status::Missing),
            (2, 1, Status::Usable),
            (3, 0, Status::Usable),
            (3, 1, Status::Usable),
        ]
        .into_iter()
        .map(|(pair, side, status)| {
            let mut trial: Trial = serde_json::from_value(json!({
                "project": "demo",
                "workload": Workload::UserSimulation,
                "side": side,
                "pair": pair,
                "observations": [{
                    "action": "Go to project definition",
                    "method": "textDocument/definition",
                    "foreground": true,
                    "started_ms": 10.0,
                    "ended_ms": 10.0 + f64::from(pair),
                    "status": status,
                    "attempts": [],
                }],
                "options": {},
            }))
            .expect("valid recorded trial");
            Observation::complete_plan(trial.workload, &mut trial.observations);
            assert_eq!(trial.observations.len(), 13);
            assert_eq!(trial.observations[0].status, status);
            assert!(
                trial.observations[1..]
                    .iter()
                    .all(|answer| answer.status == Status::Blocked)
            );
            trial
        })
        .collect();
        let refs: Vec<_> = trials.iter().collect();
        let paired = PairedTrials::new(&refs);
        let samples: Vec<_> = paired
            .action_samples("Go to project definition")
            .map(|pair| pair.map(Observation::latency))
            .collect();
        assert_eq!(samples, [[Some(3.0), Some(3.0)]]);
    }
}
