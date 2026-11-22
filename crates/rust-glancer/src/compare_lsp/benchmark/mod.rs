//! Pinned editor workloads and paired server trials, driven through public LSP.

mod fixture;
mod report;
mod scenario;

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::Context as _;
use clap::{Args, ValueEnum};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use self::{
    fixture::{Checkout, Site, TOOLCHAIN, Target},
    report::BenchmarkReport,
    scenario::{Document, EditorScenario, LibraryEditor, Observation, Status, milliseconds},
};
use super::{
    OutputFormat,
    server::{RunningServer, ServerKind},
};

#[derive(Debug, Args)]
pub(crate) struct BenchmarkArgs {
    /// Choose the editor smoke run or the release benchmark suite.
    #[arg(long, value_enum, conflicts_with = "fixture")]
    mode: Option<Mode>,
    /// Run only this project (repeatable).
    #[arg(long = "target", conflicts_with = "fixture")]
    targets: Vec<String>,
    /// Number of paired trials per workload.
    #[arg(long, conflicts_with = "fixture", value_parser = clap::value_parser!(u32).range(1..=100))]
    pairs: Option<u32>,
    /// Override the Rust Glancer executable.
    #[arg(long, conflicts_with = "fixture")]
    candidate: Option<PathBuf>,
    /// Compare two Rust Glancer binaries instead of using rust-analyzer.
    #[arg(long, conflicts_with = "fixture")]
    baseline: Option<PathBuf>,
    /// Override the rust-analyzer executable.
    #[arg(long, conflicts_with_all = ["fixture", "baseline"])]
    rust_analyzer: Option<PathBuf>,
    /// Directory containing cached pinned source checkouts.
    #[arg(long, conflicts_with = "fixture")]
    templates: Option<PathBuf>,
    /// Parent directory for the benchmark artifact and its scratch checkouts.
    #[arg(long, conflicts_with = "fixture")]
    output_dir: Option<PathBuf>,
    /// Render a previously recorded benchmark without starting either server.
    #[arg(long, conflicts_with_all = ["fixture", "mode", "targets", "pairs", "candidate", "baseline", "rust_analyzer", "templates", "workload"])]
    from_run: Option<PathBuf>,
    /// Select one workload. Restart seeds a clean checkout before measuring.
    #[arg(long, value_enum, conflicts_with = "fixture")]
    workload: Option<Workload>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Mode {
    Fast,
    Full,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Workload {
    #[value(name = "synthetic-edit")]
    Editor,
    #[value(skip)]
    FirstOpen,
    Restart,
    FirstHints,
    BuildOutputs,
    LibraryEditing,
    IsolatedRequests,
    UserSimulation,
}

impl Workload {
    const PROJECT_WORKLOADS: [Self; 5] = [
        Self::Editor,
        Self::Restart,
        Self::FirstHints,
        Self::IsolatedRequests,
        Self::UserSimulation,
    ];

    const fn label(self) -> &'static str {
        match self {
            Self::Editor => "Synthetic edit",
            Self::FirstOpen => "First open",
            Self::Restart => "Restart",
            Self::FirstHints => "First editor hints",
            Self::BuildOutputs => "Startup with build outputs",
            Self::LibraryEditing => "Library editing",
            Self::IsolatedRequests => "Isolated requests",
            Self::UserSimulation => "User simulation",
        }
    }

    const fn description(self) -> &'static str {
        match self {
            Self::Editor => {
                "This scenario starts after the server reports that indexing is complete. Timings cover the editing actions that follow."
            }
            Self::FirstOpen => {
                "Launch with an empty analysis cache. Record the ready signal and the first useful hover after opening a file."
            }
            Self::Restart => {
                "Reopen an unedited checkout after a clean shutdown. Rust Glancer can reuse its disk cache. Rust-analyzer starts with an empty in-memory analysis cache."
            }
            Self::FirstHints => {
                "In a fresh process, request editor hints for the visible lines after the ready signal, then repeat the same request."
            }
            Self::BuildOutputs => {
                "Prepare generated files with Cargo before launch, then request hover on a generated function. During startup, rust-analyzer runs a warm Cargo check to load build-script output."
            }
            Self::LibraryEditing => {
                "Add an API and its tests. Replay jittered typing at about 350 characters per minute, accept completions and imports, and query the saved API across files."
            }
            Self::IsolatedRequests => {
                "Fresh checkout and server. After readiness, request hover and editor hints twice each, one at a time, on an unchanged file."
            }
            Self::UserSimulation => {
                "Open a fresh workspace and measure the first usable hover. After the server reports readiness, request editor hints, navigate existing code, and add a module and tests with paced typing and completions."
            }
        }
    }

    const fn is_startup(self) -> bool {
        matches!(self, Self::FirstOpen | Self::Restart | Self::BuildOutputs)
    }

    const fn measures_foreground(self) -> bool {
        matches!(
            self,
            Self::Editor | Self::LibraryEditing | Self::UserSimulation
        )
    }

    const fn directory(self) -> &'static str {
        match self {
            Self::Editor => "editor",
            Self::FirstOpen | Self::Restart => "startup",
            Self::FirstHints => "first-hints",
            Self::BuildOutputs => "build-outputs",
            Self::LibraryEditing => "library-editing",
            Self::IsolatedRequests => "isolated-requests",
            Self::UserSimulation => "user-simulation",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Comparison {
    RustAnalyzer,
    Versions,
}

struct PreparedTarget<'a> {
    target: &'a Target,
    template: PathBuf,
}

impl<'a> PreparedTarget<'a> {
    fn prepare(
        target: &'a Target,
        artifact: &Path,
        templates: &Path,
        managed: bool,
    ) -> anyhow::Result<Self> {
        let template = target.prepare(templates, managed)?;
        let directory = artifact.join(&target.id).join("prepare");
        let preparation = target.checkout(&directory, &template)?;
        fixture::run(preparation.cargo().args(["fetch", "--locked"]))?;
        fs::remove_dir_all(directory)?;
        Ok(Self { target, template })
    }
}

#[derive(Clone, Copy)]
enum TrialPlan<'a> {
    Editor(&'a PreparedTarget<'a>),
    RestartSeed(&'a PreparedTarget<'a>),
    Restart(&'a PreparedTarget<'a>),
    FirstHints(&'a PreparedTarget<'a>),
    IsolatedRequests(&'a PreparedTarget<'a>),
    UserSimulation(&'a PreparedTarget<'a>),
    BuildOutputs,
    LibraryEditing,
}

impl<'a> TrialPlan<'a> {
    fn for_target(workload: Workload, target: &'a PreparedTarget<'a>) -> Self {
        match workload {
            Workload::Editor => Self::Editor(target),
            Workload::Restart => Self::Restart(target),
            Workload::FirstHints => Self::FirstHints(target),
            Workload::IsolatedRequests => Self::IsolatedRequests(target),
            Workload::UserSimulation => Self::UserSimulation(target),
            _ => unreachable!("only project workloads are selected for a target"),
        }
    }

    fn workload(self) -> Workload {
        match self {
            Self::Editor(_) => Workload::Editor,
            Self::RestartSeed(_) => Workload::FirstOpen,
            Self::Restart(_) => Workload::Restart,
            Self::FirstHints(_) => Workload::FirstHints,
            Self::IsolatedRequests(_) => Workload::IsolatedRequests,
            Self::UserSimulation(_) => Workload::UserSimulation,
            Self::BuildOutputs => Workload::BuildOutputs,
            Self::LibraryEditing => Workload::LibraryEditing,
        }
    }

    fn project(self) -> &'a str {
        match self {
            Self::Editor(target)
            | Self::RestartSeed(target)
            | Self::Restart(target)
            | Self::FirstHints(target)
            | Self::IsolatedRequests(target)
            | Self::UserSimulation(target) => &target.target.id,
            Self::BuildOutputs => "build-outputs",
            Self::LibraryEditing => "library-editing",
        }
    }

    fn checkout(self, directory: &Path) -> anyhow::Result<Checkout> {
        match self {
            Self::Editor(target)
            | Self::RestartSeed(target)
            | Self::Restart(target)
            | Self::FirstHints(target)
            | Self::IsolatedRequests(target)
            | Self::UserSimulation(target) => target.target.checkout(directory, &target.template),
            Self::BuildOutputs => Checkout::built_fixture(directory),
            Self::LibraryEditing => LibraryEditor::checkout(directory),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct Trial {
    project: String,
    workload: Workload,
    pair: u32,
    side: usize,
    options: Value,
    prepare_ms: Option<f64>,
    initialize_ms: Option<f64>,
    ready_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ready_error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    opened_ms: Option<f64>,
    first_answer_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    editing_started_ms: Option<f64>,
    observations: Vec<Observation>,
    error: Option<String>,
    cleanup_error: Option<String>,
}

impl Trial {
    fn failed(&self) -> bool {
        self.error.is_some() || self.cleanup_error.is_some()
    }

    fn record_first_answer(&mut self, answer: Observation) {
        if answer.status == Status::Usable {
            self.first_answer_ms = answer.ended_ms;
        }
        self.observations.push(answer);
    }

    async fn observe(
        &mut self,
        checkout: &Checkout,
        plan: TrialPlan<'_>,
        server: &mut RunningServer,
    ) -> anyhow::Result<()> {
        self.initialize_ms = Some(milliseconds(
            server
                .initialize(&checkout.source, self.options.clone())
                .await?,
        ));
        let ready = match plan {
            TrialPlan::Editor(target) => {
                EditorScenario::load(&checkout.source, target.target)?
                    .run(server, &mut self.observations)
                    .await?
            }
            TrialPlan::LibraryEditing => {
                LibraryEditor::run(&checkout.source, self.pair, server, &mut self.observations)
                    .await?
            }
            TrialPlan::UserSimulation(target) => {
                let target = target.target;
                let (entry, call) = Document::load_at(&checkout.source, &target.entry)?;
                entry.open(server).await?;
                self.opened_ms = Some(milliseconds(server.spawned_at().elapsed()));
                self.record_first_answer(
                    entry
                        .first_hover(server, call, &target.return_type.before)
                        .await,
                );

                // Keep the same open document throughout the session. The ready notification
                // carries its arrival timestamp even if the hover completed later.
                let ready = server.wait_until_ready().await?;
                self.ready_ms = Some(milliseconds(ready.duration_since(server.spawned_at())));
                self.editing_started_ms = Some(milliseconds(server.spawned_at().elapsed()));
                return LibraryEditor::run_in_project(
                    &checkout.source,
                    target,
                    self.pair,
                    server,
                    entry,
                    call,
                    &mut self.observations,
                )
                .await;
            }
            TrialPlan::FirstHints(target) => {
                let (ready, observations) =
                    scenario::first_hints(&checkout.source, target.target, server).await?;
                self.observations = observations;
                ready
            }
            TrialPlan::IsolatedRequests(target) => {
                let (ready, observations) =
                    scenario::isolated_requests(&checkout.source, target.target, server).await?;
                self.observations = observations;
                ready
            }
            TrialPlan::RestartSeed(_) | TrialPlan::Restart(_) | TrialPlan::BuildOutputs => {
                let generated = Site {
                    file: "src/lib.rs".into(),
                    anchor: "    generated_|value()".into(),
                };
                let (site, expected) = match plan {
                    TrialPlan::RestartSeed(target) | TrialPlan::Restart(target) => (
                        &target.target.entry,
                        target.target.return_type.before.as_str(),
                    ),
                    _ => (&generated, "u16"),
                };
                let answer = scenario::startup(&checkout.source, site, expected, server).await?;
                self.record_first_answer(answer);
                // A server can answer before it emits its ready signal, or could omit the
                // signal entirely. Preserve the completed hover either way.
                server.wait_until_ready().await?
            }
        };
        self.ready_ms = Some(milliseconds(ready.duration_since(server.spawned_at())));
        Ok(())
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct Binary {
    label: String,
    executable: PathBuf,
    version: String,
}

struct Server {
    kind: ServerKind,
    binary: Binary,
}

impl Server {
    fn resolve(
        kind: ServerKind,
        override_path: Option<&Path>,
        label: &str,
    ) -> anyhow::Result<Self> {
        let spec = kind.command_spec(override_path)?;
        // Resolve PATH entries up front because subprocesses run from different checkouts.
        let executable = PathBuf::from(spec.executable);
        let executable = if executable.components().count() == 1 && !executable.is_absolute() {
            std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
                .map(|path| path.join(&executable))
                .flat_map(|path| {
                    let with_extension = path.with_extension(std::env::consts::EXE_EXTENSION);
                    [path, with_extension]
                })
                .find(|path| path.is_file())
                .with_context(|| format!("find {} on PATH", executable.display()))?
        } else {
            executable
        };
        // Keep the rustup proxy's filename. Canonicalizing a symlink to rustup would change
        // which command it dispatches. Only the directory needs to become absolute.
        let executable = std::path::absolute(executable)?;
        let version = fixture::run(
            Command::new(&executable)
                .arg("--version")
                .env("RUSTUP_TOOLCHAIN", TOOLCHAIN),
        )?;
        Ok(Self {
            kind,
            binary: Binary {
                label: label.into(),
                executable,
                version: version.trim().into(),
            },
        })
    }

    async fn trial(
        &self,
        checkout: &Checkout,
        plan: TrialPlan<'_>,
        pair: u32,
        side: usize,
    ) -> Trial {
        let workload = plan.workload();
        let options = self.kind.initialization_options(
            workload == Workload::Restart,
            workload == Workload::BuildOutputs,
        );
        let mut trial = Trial {
            project: plan.project().into(),
            workload,
            pair,
            side,
            options,
            prepare_ms: None,
            initialize_ms: None,
            ready_ms: None,
            ready_error: None,
            opened_ms: None,
            first_answer_ms: None,
            editing_started_ms: None,
            observations: Vec::new(),
            error: None,
            cleanup_error: None,
        };
        let result = async {
            if workload == Workload::BuildOutputs {
                let started = Instant::now();
                fixture::run(checkout.cargo().args(["check", "--locked", "--offline"]))?;
                trial.prepare_ms = Some(milliseconds(started.elapsed()));
            }
            let mut server = RunningServer::spawn(
                self.kind,
                Some(&self.binary.executable),
                Some((&checkout.source, &checkout.cargo_output, TOOLCHAIN)),
            )?;
            let result = trial.observe(checkout, plan, &mut server).await;
            // Run shutdown even after a protocol or workload failure. A failed shutdown is
            // tracked independently because the next trial needs a clean process boundary.
            if let Err(error) = server.shutdown().await {
                trial.cleanup_error = Some(format!("{error:#}"));
            }
            result
        }
        .await;
        if let Err(error) = result {
            trial.error = Some(format!("{error:#}"));
        }
        Observation::complete_plan(workload, &mut trial.observations);
        trial
    }
}

pub(crate) async fn run(args: BenchmarkArgs, format: Option<OutputFormat>) -> anyhow::Result<()> {
    let defaults = PathBuf::from("target/compare-lsp");
    let parent = args.output_dir.as_deref().unwrap_or(&defaults);
    fs::create_dir_all(parent)?;
    let parent = std::path::absolute(parent)?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system time before UNIX epoch")?;
    let artifact = parent.join(format!("{}-{}", timestamp.as_nanos(), std::process::id()));
    fs::create_dir(&artifact)?;
    if let Some(input) = &args.from_run {
        let report = BenchmarkReport::read(input)?;
        return report.write(&artifact, format);
    }
    tracing::info!(directory = %artifact.display(), "writing benchmark artifacts");
    let managed_templates = args.templates.is_none();
    let templates = args.templates.unwrap_or_else(|| defaults.join("templates"));
    fs::create_dir_all(&templates)?;
    let templates = std::path::absolute(templates)?;
    let mode = args.mode.unwrap_or(Mode::Fast);
    let pairs = args.pairs.unwrap_or(match mode {
        Mode::Fast => 1,
        Mode::Full => 5,
    });
    let catalog = Target::catalog()?;
    for id in &args.targets {
        anyhow::ensure!(
            catalog.iter().any(|target| &target.id == id)
                || id == "library-editing"
                || id == "build-outputs"
                    && (mode == Mode::Full || args.workload == Some(Workload::BuildOutputs)),
            "unknown target {id:?} for {mode:?}"
        );
    }
    let wanted = |workload, default| {
        args.workload
            .map_or(default, |selected| selected == workload)
    };
    let build_outputs = wanted(Workload::BuildOutputs, mode == Mode::Full)
        && (args.targets.is_empty() || args.targets.iter().any(|id| id == "build-outputs"));
    let library_target = args.targets.iter().any(|id| id == "library-editing");
    let library_editing = wanted(
        Workload::LibraryEditing,
        mode == Mode::Full || library_target,
    ) && (args.targets.is_empty() || library_target);
    let selected_workloads: Vec<_> = Workload::PROJECT_WORKLOADS
        .into_iter()
        .filter(|&workload| wanted(workload, mode == Mode::Full || workload == Workload::Editor))
        .collect();
    let targets: Vec<_> = catalog
        .into_iter()
        .filter(|_| !selected_workloads.is_empty())
        .filter(|target| {
            if args.targets.is_empty() {
                mode == Mode::Full || target.id == "ripgrep"
            } else {
                args.targets.contains(&target.id)
            }
        })
        .collect();
    anyhow::ensure!(
        !targets.is_empty() || build_outputs || library_editing,
        "no projects match the selected workload"
    );

    let candidate = Server::resolve(
        ServerKind::RustGlancer,
        args.candidate.as_deref(),
        if args.baseline.is_some() {
            "New"
        } else {
            "Rust Glancer"
        },
    )?;
    let reference = match &args.baseline {
        Some(path) => Server::resolve(ServerKind::RustGlancer, Some(path), "Old")?,
        None => Server::resolve(
            ServerKind::RustAnalyzer,
            args.rust_analyzer.as_deref(),
            "rust-analyzer",
        )?,
    };
    let servers = [&reference, &candidate];
    let toolchain = fixture::run(
        Command::new("rustc")
            .arg("-vV")
            .env("RUSTUP_TOOLCHAIN", TOOLCHAIN),
    )?;
    let sysroot = fixture::run(
        Command::new("rustc")
            .args(["--print", "sysroot"])
            .env("RUSTUP_TOOLCHAIN", TOOLCHAIN),
    )?;
    anyhow::ensure!(
        Path::new(sysroot.trim())
            .join("lib/rustlib/src/rust/library/core/src/lib.rs")
            .is_file(),
        "install rust-src for benchmark toolchain {TOOLCHAIN}"
    );
    let comparison = if args.baseline.is_some() {
        Comparison::Versions
    } else {
        Comparison::RustAnalyzer
    };
    let mut report = BenchmarkReport::new(comparison, mode, pairs, toolchain, targets);
    let mut trials = Vec::new();
    let mut errors = Vec::new();

    // Resolve and fetch all inputs before spawning a server. Cargo runs offline throughout the
    // timed workloads, so a missing dependency becomes a setup error instead of a timing sample.
    let prepared: Vec<_> = report
        .targets
        .iter()
        .filter_map(|target| {
            match PreparedTarget::prepare(target, &artifact, &templates, managed_templates) {
                Ok(prepared) => Some(prepared),
                Err(error) => {
                    errors.push(format!("{}: {error:#}", target.id));
                    None
                }
            }
        })
        .collect();
    let plans = prepared
        .iter()
        .flat_map(|target| {
            selected_workloads
                .iter()
                .map(move |&workload| TrialPlan::for_target(workload, target))
        })
        .chain(library_editing.then_some(TrialPlan::LibraryEditing))
        .chain(build_outputs.then_some(TrialPlan::BuildOutputs));

    // Only the explicit restart retains the previous process's checkout and output directory.
    'workloads: for (index, plan) in plans.enumerate() {
        let project = plan.project();
        let workload = plan.workload();
        for pair in 1..=pairs {
            // Alternate within each workload and rotate which server gets the extra
            // first slot when the number of pairs is odd.
            for side in if (index + pair as usize).is_multiple_of(2) {
                [1, 0]
            } else {
                [0, 1]
            } {
                let directory = artifact
                    .join(project)
                    .join(workload.directory())
                    .join(format!("{pair}-{side}"));
                tracing::info!(
                    project,
                    ?workload,
                    pair,
                    server = servers[side].binary.label,
                    "running benchmark trial"
                );
                let checkout = match plan.checkout(&directory) {
                    Ok(checkout) => checkout,
                    Err(error) => {
                        errors.push(format!("{project} {workload:?} {pair}-{side}: {error:#}"));
                        continue;
                    }
                };
                if let TrialPlan::Restart(target) = plan {
                    let seed = servers[side]
                        .trial(&checkout, TrialPlan::RestartSeed(target), pair, side)
                        .await;
                    // Restart reopens an unedited checkout after a clean shutdown. The
                    // seeding run prepares its cache and the restart supplies the sample.
                    if seed.failed() || seed.first_answer_ms.is_none() {
                        let problem = seed
                            .cleanup_error
                            .as_deref()
                            .or(seed.error.as_deref())
                            .unwrap_or("no usable startup answer");
                        errors.push(format!("{project} restart {pair}-{side} seed: {problem}"));
                        if seed.cleanup_error.is_some() {
                            break 'workloads;
                        }
                        continue;
                    }
                }

                let trial = servers[side].trial(&checkout, plan, pair, side).await;
                let keep_checkout = trial.failed();
                let cleanup_failed = trial.cleanup_error.is_some();
                trials.push(trial);
                if cleanup_failed {
                    break 'workloads;
                }
                if !keep_checkout && let Err(error) = fs::remove_dir_all(&directory) {
                    errors.push(format!(
                        "remove trial checkout {}: {error}",
                        directory.display()
                    ));
                }
            }
        }
    }

    report.binaries = vec![reference.binary, candidate.binary];
    report.trials = trials;
    report.errors = errors;
    report.write(&artifact, format)?;
    let failures =
        report.errors.len() + report.trials.iter().filter(|trial| trial.failed()).count();
    anyhow::ensure!(
        failures == 0,
        "{failures} benchmark setup or lifecycle errors; see {}",
        artifact.display()
    );
    Ok(())
}
