use std::{cell::RefCell, fmt, path::PathBuf};

use anyhow::Context as _;
use rg_analysis::{ReferenceLocation, ReferenceQuery};
use rg_def_map::TargetRef;
use rg_project::{FileContext, PackageResidencyPolicy, Project, StartupCacheLoad};
use rg_workspace::WorkspaceMetadata;

use super::BenchTarget;

// Query benchmarks exercise the LSP-facing project layer, including package offloading. Keeping
// every package offloadable is the most useful stress case: each query has to go through the same
// package-store loading path a memory-constrained LSP session would use.
const QUERY_RESIDENCY_POLICY: PackageResidencyPolicy = PackageResidencyPolicy::AllOffloadable;

/// Named editor-style query cases collected by both Divan and Gungraun benchmarks.
///
/// The enum is the public surface used by benchmark macros. `BenchQuery::case` below keeps the
/// fixture path, cursor marker, and query kind close to that stable benchmark name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BenchQuery {
    HoverWorkspaceSummary,
    GotoWorkspaceConstructor,
    GotoWorkspaceType,
    GotoWorkspaceImplementation,
    ReferencesWorkspaceSummary,
    DocumentHighlightSummary,
    CompletionWorkspaceSummary,
    DocumentSymbolsApi,
    WorkspaceSymbolsWorkspace,
}

impl BenchQuery {
    const DIVAN: [Self; 9] = [
        Self::HoverWorkspaceSummary,
        Self::GotoWorkspaceConstructor,
        Self::GotoWorkspaceType,
        Self::GotoWorkspaceImplementation,
        Self::ReferencesWorkspaceSummary,
        Self::DocumentHighlightSummary,
        Self::CompletionWorkspaceSummary,
        Self::DocumentSymbolsApi,
        Self::WorkspaceSymbolsWorkspace,
    ];

    fn case(self) -> QueryCase {
        match self {
            Self::HoverWorkspaceSummary => QueryCase {
                target: BenchTarget::SmallApp,
                relative_path: Some("crates/api/src/lib.rs"),
                cursor_marker: Some("Vec<Workspace$0Summary>"),
                kind: QueryKind::Hover,
            },
            Self::GotoWorkspaceConstructor => QueryCase {
                target: BenchTarget::SmallApp,
                relative_path: Some("crates/api/src/lib.rs"),
                cursor_marker: Some("Workspace::from_$0request(request)"),
                kind: QueryKind::GotoDefinition,
            },
            Self::GotoWorkspaceType => QueryCase {
                target: BenchTarget::SmallApp,
                relative_path: Some("crates/api/src/lib.rs"),
                cursor_marker: Some("let summary = work$0space.summary();"),
                kind: QueryKind::GotoTypeDefinition,
            },
            Self::GotoWorkspaceImplementation => QueryCase {
                target: BenchTarget::SmallApp,
                relative_path: Some("crates/api/src/lib.rs"),
                cursor_marker: Some("Work$0space::from_request(request)"),
                kind: QueryKind::GotoImplementation,
            },
            Self::ReferencesWorkspaceSummary => QueryCase {
                target: BenchTarget::SmallApp,
                relative_path: Some("crates/api/src/lib.rs"),
                cursor_marker: Some("Vec<Workspace$0Summary>"),
                kind: QueryKind::References,
            },
            Self::DocumentHighlightSummary => QueryCase {
                target: BenchTarget::SmallApp,
                relative_path: Some("crates/api/src/lib.rs"),
                cursor_marker: Some("let sum$0mary = workspace.summary();"),
                kind: QueryKind::DocumentHighlight,
            },
            Self::CompletionWorkspaceSummary => QueryCase {
                target: BenchTarget::SmallApp,
                relative_path: Some("crates/api/src/lib.rs"),
                cursor_marker: Some("workspace.$0summary();"),
                kind: QueryKind::Completion,
            },
            Self::DocumentSymbolsApi => QueryCase {
                target: BenchTarget::SmallApp,
                relative_path: Some("crates/api/src/lib.rs"),
                cursor_marker: None,
                kind: QueryKind::DocumentSymbols,
            },
            Self::WorkspaceSymbolsWorkspace => QueryCase {
                target: BenchTarget::SmallApp,
                relative_path: None,
                cursor_marker: None,
                kind: QueryKind::WorkspaceSymbols("Workspace"),
            },
        }
    }
}

impl fmt::Display for BenchQuery {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::HoverWorkspaceSummary => "hover_workspace_summary",
            Self::GotoWorkspaceConstructor => "goto_workspace_constructor",
            Self::GotoWorkspaceType => "goto_workspace_type",
            Self::GotoWorkspaceImplementation => "goto_workspace_implementation",
            Self::ReferencesWorkspaceSummary => "references_workspace_summary",
            Self::DocumentHighlightSummary => "document_highlight_summary",
            Self::CompletionWorkspaceSummary => "completion_workspace_summary",
            Self::DocumentSymbolsApi => "document_symbols_api",
            Self::WorkspaceSymbolsWorkspace => "workspace_symbols_workspace",
        };
        write!(f, "{}::{name}", self.case().target)
    }
}

// One concrete source position or whole-workspace query.
//
// Cursor markers use `$0` inside a source substring rather than absolute line/column pairs. That
// makes the benchmark cases easier to maintain when fixtures are reformatted or small surrounding
// edits are made.
#[derive(Debug, Clone, Copy)]
struct QueryCase {
    target: BenchTarget,
    relative_path: Option<&'static str>,
    cursor_marker: Option<&'static str>,
    kind: QueryKind,
}

#[derive(Debug, Clone, Copy)]
enum QueryKind {
    Hover,
    GotoDefinition,
    GotoTypeDefinition,
    GotoImplementation,
    References,
    DocumentHighlight,
    Completion,
    DocumentSymbols,
    WorkspaceSymbols(&'static str),
}

/// Frozen `Project` prepared once per target/residency policy.
///
/// Building the project is setup for query benchmarks, not the thing being measured here. The
/// fixture is leaked intentionally so Divan/Gungraun can reuse it across benchmark samples without
/// cloning or rebuilding the analysis graph.
#[derive(Debug)]
pub struct BenchProjectFixture {
    pub project: Project,
    pub project_root: PathBuf,
}

impl BenchProjectFixture {
    fn get(target: BenchTarget, policy: PackageResidencyPolicy) -> &'static Self {
        thread_local! {
            static FIXTURES: RefCell<Vec<((BenchTarget, PackageResidencyPolicy), &'static BenchProjectFixture)>> = const {
                RefCell::new(Vec::new())
            };
        }

        FIXTURES.with(|fixtures| {
            if let Some((_, cached)) =
                fixtures
                    .borrow()
                    .iter()
                    .find(|((cached_target, cached_policy), _)| {
                        *cached_target == target && *cached_policy == policy
                    })
            {
                return *cached;
            }

            let loaded = Box::leak(Box::new(Self::load(target, policy)));
            fixtures.borrow_mut().push(((target, policy), loaded));

            loaded
        })
    }

    fn load(target: BenchTarget, policy: PackageResidencyPolicy) -> Self {
        target.prepare();

        // Startup cache loading is enabled to model a warm, offloaded workspace. If cache artifacts
        // already exist, offloadable packages can be loaded from disk during project construction;
        // either way, the timed query body sees a frozen project with the requested residency plan.
        let workspace = WorkspaceMetadata::from_manifest_path(target.manifest_path())
            .unwrap_or_else(|error| panic!("{target} Cargo metadata should load: {error}"));
        let project = Project::builder(workspace)
            .package_residency_policy(policy)
            .startup_cache_load(StartupCacheLoad::Enabled)
            .build()
            .unwrap_or_else(|error| {
                panic!(
                    "{target} project should build with {} residency: {error}",
                    policy.config_name()
                )
            })
            .into_project();

        Self {
            project,
            project_root: target.project_root(),
        }
    }
}

/// Fully resolved benchmark input for one query case.
///
/// Preparing a query resolves filesystem paths, source markers, file contexts, and target contexts.
/// That mirrors the LSP worker's request setup, but it is intentionally outside the timed closure
/// so the reported number is about analysis query execution rather than test harness plumbing.
#[derive(Debug, Clone)]
pub struct PreparedQuery {
    pub query: BenchQuery,
    pub fixture: &'static BenchProjectFixture,
    pub sites: Vec<QuerySite>,
    pub analysis_targets: Vec<TargetRef>,
}

impl PreparedQuery {
    pub fn new(query: BenchQuery) -> Self {
        Self::try_new(query)
            .unwrap_or_else(|error| panic!("{query} benchmark query should prepare: {error:#}"))
    }

    fn try_new(query: BenchQuery) -> anyhow::Result<Self> {
        let case = query.case();
        let fixture = BenchProjectFixture::get(case.target, QUERY_RESIDENCY_POLICY);
        let mut sites = Vec::new();
        let mut analysis_targets = Vec::new();

        if let Some(relative_path) = case.relative_path {
            // Path and marker resolution are deliberately explicit here. If a fixture changes and a
            // marker no longer exists, benchmark preparation fails loudly instead of silently
            // measuring a nearby but different symbol.
            let path = fixture.project_root.join(relative_path);
            let offset = match case.cursor_marker {
                Some(cursor_marker) => Some(resolve_cursor_offset(&path, cursor_marker)?),
                None => None,
            };

            // A single file can belong to multiple target contexts, for example a library module
            // reused by a binary. Query benchmarks run every visible target context and deduplicate
            // results the same way the LSP worker does.
            let snapshot = fixture.project.snapshot();
            let contexts = snapshot
                .file_contexts_for_path(&path)
                .with_context(|| format!("while attempting to resolve {}", path.display()))?;

            for context in contexts {
                for target in &context.targets {
                    push_unique_target(&mut analysis_targets, *target);
                    sites.push(QuerySite {
                        context: context.clone(),
                        target: *target,
                        offset: offset.unwrap_or_default(),
                    });
                }
            }
        }

        Ok(Self {
            query,
            fixture,
            sites,
            analysis_targets,
        })
    }

    pub fn run(&self) -> usize {
        self.try_run()
            .unwrap_or_else(|error| panic!("{} benchmark query should run: {error:#}", self.query))
    }

    fn try_run(&self) -> anyhow::Result<usize> {
        match self.query.case().kind {
            QueryKind::Hover => self.run_hover(),
            QueryKind::GotoDefinition => self.run_navigation(NavigationKind::Definition),
            QueryKind::GotoTypeDefinition => self.run_navigation(NavigationKind::TypeDefinition),
            QueryKind::GotoImplementation => self.run_navigation(NavigationKind::Implementation),
            QueryKind::References => self.run_references(),
            QueryKind::DocumentHighlight => self.run_document_highlight(),
            QueryKind::Completion => self.run_completion(),
            QueryKind::DocumentSymbols => self.run_document_symbols(),
            QueryKind::WorkspaceSymbols(query) => self.run_workspace_symbols(query),
        }
    }

    fn run_hover(&self) -> anyhow::Result<usize> {
        let snapshot = self.fixture.project.snapshot();
        let analysis = snapshot
            .analysis_for_targets(&self.analysis_targets)
            .context("while attempting to create target-scoped analysis")?;
        let mut count = 0;

        for site in &self.sites {
            if analysis
                .hover(site.target, site.context.file, site.offset)
                .context("while attempting to run hover query")?
                .is_some()
            {
                count += 1;
            }
        }

        Ok(count)
    }

    fn run_navigation(&self, kind: NavigationKind) -> anyhow::Result<usize> {
        let snapshot = self.fixture.project.snapshot();
        let analysis = snapshot
            .analysis_for_targets(&self.analysis_targets)
            .context("while attempting to create target-scoped analysis")?;
        let mut count = 0;

        for site in &self.sites {
            let targets = match kind {
                NavigationKind::Definition => analysis
                    .goto_definition(site.target, site.context.file, site.offset)
                    .context("while attempting to run goto-definition query")?,
                NavigationKind::TypeDefinition => analysis
                    .goto_type_definition(site.target, site.context.file, site.offset)
                    .context("while attempting to run goto-type-definition query")?,
                NavigationKind::Implementation => analysis
                    .goto_implementation(site.target, site.context.file, site.offset)
                    .context("while attempting to run goto-implementation query")?,
            };
            count += targets.len();
        }

        Ok(count)
    }

    fn run_references(&self) -> anyhow::Result<usize> {
        let snapshot = self.fixture.project.snapshot();
        // Find-references is intentionally full-analysis today. The LSP worker first resolves
        // declaration targets, then asks the project snapshot which targets should be searched from
        // that origin package. Keep that composition here so the benchmark tracks real request
        // behavior.
        let analysis = snapshot
            .full_analysis()
            .context("while attempting to create full analysis")?;
        let mut references = Vec::<ReferenceLocation>::new();

        for site in &self.sites {
            let declaration_targets = analysis
                .goto_definition(site.target, site.context.file, site.offset)
                .context("while attempting to resolve reference declaration targets")?
                .into_iter()
                .map(|target| target.target)
                .collect::<Vec<_>>();
            let search_targets =
                snapshot.reference_search_targets(site.context.package, &declaration_targets);

            for reference in analysis
                .references(
                    site.target,
                    site.context.file,
                    site.offset,
                    ReferenceQuery::find_references(&search_targets, true),
                )
                .context("while attempting to run references query")?
            {
                push_unique_reference(&mut references, reference);
            }
        }

        Ok(references.len())
    }

    fn run_document_highlight(&self) -> anyhow::Result<usize> {
        let snapshot = self.fixture.project.snapshot();
        // Document highlight is not a separate analysis primitive. It is the file-scoped reference
        // query used by the LSP worker before converting reference spans into LSP highlights.
        let analysis = snapshot
            .analysis_for_targets(&self.analysis_targets)
            .context("while attempting to create target-scoped analysis")?;
        let mut highlights = Vec::<ReferenceLocation>::new();

        for site in &self.sites {
            for reference in analysis
                .references(
                    site.target,
                    site.context.file,
                    site.offset,
                    ReferenceQuery::file_scoped(site.target, site.context.file),
                )
                .context("while attempting to run document-highlight references query")?
            {
                if reference.target.package != site.context.package
                    || reference.file_id != site.context.file
                {
                    continue;
                }

                push_unique_reference(&mut highlights, reference);
            }
        }

        Ok(highlights.len())
    }

    fn run_completion(&self) -> anyhow::Result<usize> {
        let snapshot = self.fixture.project.snapshot();
        let analysis = snapshot
            .analysis_for_targets(&self.analysis_targets)
            .context("while attempting to create target-scoped analysis")?;
        let mut completions = Vec::new();

        for site in &self.sites {
            for completion in analysis
                .completions_at_dot(site.target, site.context.file, site.offset)
                .context("while attempting to run completion query")?
            {
                if !completions.contains(&completion) {
                    completions.push(completion);
                }
            }
        }

        Ok(completions.len())
    }

    fn run_document_symbols(&self) -> anyhow::Result<usize> {
        let snapshot = self.fixture.project.snapshot();
        let analysis = snapshot
            .analysis_for_targets(&self.analysis_targets)
            .context("while attempting to create target-scoped analysis")?;
        let mut count = 0;

        for site in &self.sites {
            count += analysis
                .document_symbols(site.target, site.context.file)
                .context("while attempting to run document-symbols query")?
                .len();
        }

        Ok(count)
    }

    fn run_workspace_symbols(&self, query: &str) -> anyhow::Result<usize> {
        let snapshot = self.fixture.project.snapshot();
        let analysis = snapshot
            .full_analysis()
            .context("while attempting to create full analysis")?;

        Ok(analysis
            .workspace_symbols(query)
            .context("while attempting to run workspace-symbols query")?
            .len())
    }
}

#[derive(Debug, Clone)]
pub struct QuerySite {
    pub context: FileContext,
    pub target: TargetRef,
    pub offset: u32,
}

#[derive(Debug, Clone, Copy)]
enum NavigationKind {
    Definition,
    TypeDefinition,
    Implementation,
}

pub fn divan_queries() -> Vec<BenchQuery> {
    BenchQuery::DIVAN.to_vec()
}

fn resolve_cursor_offset(path: &PathBuf, cursor_marker: &str) -> anyhow::Result<u32> {
    // The marker describes both the source text to find and where the cursor should be placed
    // inside that text. Example: `Workspace::from_$0request(request)` resolves to the byte offset
    // inside `from_request`.
    let marker_offset = cursor_marker
        .find("$0")
        .with_context(|| format!("while attempting to find cursor marker in {cursor_marker:?}"))?;
    let needle = cursor_marker.replace("$0", "");
    let source = std::fs::read_to_string(path)
        .with_context(|| format!("while attempting to read {}", path.display()))?;
    let source_offset = source.find(&needle).with_context(|| {
        format!(
            "while attempting to find query marker {needle:?} in {}",
            path.display()
        )
    })?;

    u32::try_from(source_offset + marker_offset)
        .context("while attempting to convert query marker offset")
}

fn push_unique_target(targets: &mut Vec<TargetRef>, target: TargetRef) {
    if !targets.contains(&target) {
        targets.push(target);
    }
}

fn push_unique_reference(references: &mut Vec<ReferenceLocation>, reference: ReferenceLocation) {
    if !references.contains(&reference) {
        references.push(reference);
    }
}
