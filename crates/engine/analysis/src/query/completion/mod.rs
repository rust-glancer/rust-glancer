//! Completion assembly for source positions.
//!
//! Completion has two inputs. Indexed source views supply semantic scope and identity for shapes
//! such as `user.na$0`, `Widget::ne$0`, `let value = inp$0`, and `User { na$0 }`.
//! A speculative parse of the request buffer recovers source that is intentionally incomplete,
//! such as `fn re$0` in a trait impl, `mod pars$0`, `#[derive(Cl$0)]`, or
//! `format!("{na$0}")`.
//!
//! `CompletionResolver` chooses exactly one primary syntax/semantic family, then adds the small
//! overlays valid at that site. The `resolvers` module owns those result-producing flows;
//! sibling modules provide site detection, syntax classification, candidate lookup, rendering,
//! and source-edit policy. Examples use `$0` to mark the cursor.
//!
//! Auto-import is an overlay for ordinary names and methods. At `let _: HashM$0`, the row
//! carries the path to `HashMap` so the editor can ask for the `use` edit after selecting it.
//! This keeps source-edit planning out of the search that runs while the user types.

mod candidates;
mod pattern;
mod render;
mod resolvers;
mod site;
mod syntax;

use std::{
    fmt,
    time::{Duration, Instant},
};

use rg_ir_model::{CrateRef, FileId};
use rg_ir_view::{
    display::syntax::SyntaxRenderer, lookup::importable::ImportableNameSearch, member::MemberView,
    ty::locals::BodyView,
};

use self::{candidates::CompletionCandidateSource, site::CompletionSite};
pub(crate) use self::{
    resolvers::CompletionResolver,
    site::{CompletionSiteDetector, PathCompletionSite},
    syntax::CompletionSyntaxContext,
};
use crate::{
    Analysis,
    model::{CompletionAdditionalEdit, CompletionEdit, CompletionImport, CompletionKind},
    query::import::{ImportContext, ImportEditPlan, ImportEditPlanner},
};

/// Parsed and classified editor text used by the selected completion query.
///
/// Preparing the speculative tree separately keeps syntax recovery outside semantic resolution.
pub struct CompletionSource<'source> {
    offset: u32,
    syntax: CompletionSyntaxContext<'source>,
}

impl fmt::Debug for CompletionSource<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CompletionSource")
            .field("offset", &self.offset)
            .field("source_len", &self.source_text().len())
            .finish_non_exhaustive()
    }
}

impl<'source> CompletionSource<'source> {
    /// Parses `source_text` once after replacing the partial cursor prefix with a stable marker.
    pub fn new(source_text: &'source str, offset: u32) -> Option<Self> {
        Some(Self {
            offset,
            syntax: CompletionSyntaxContext::at(source_text, offset)?,
        })
    }

    /// Returns the UTF-8 cursor offset this syntax analysis describes.
    pub fn offset(&self) -> u32 {
        self.offset
    }

    /// Returns the exact editor buffer used to build the speculative tree.
    pub fn source_text(&self) -> &'source str {
        self.syntax.source()
    }
}

/// Editor capabilities that affect how completion items should be rendered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CompletionClientCapabilities {
    /// Whether insertion text may contain LSP snippet placeholders.
    pub snippet_support: bool,
    /// Whether the caller can ask for the import edit of a selected completion.
    pub auto_import: bool,
}

// Leave time for candidate rendering and protocol conversion after the optional search.
const AUTO_IMPORT_BUDGET: Duration = Duration::from_millis(4);

/// One source-position completion query plus the editor state needed for request-local syntax.
///
/// Source text and speculative syntax may be supplied by the caller. Otherwise completion reads
/// the active snapshot and prepares its syntax there; both paths use the same completion rules.
#[derive(Debug, Clone, Copy)]
pub struct CompletionQuery<'a> {
    /// Crate context used to interpret paths and visibility from `file_id`.
    pub crate_ref: CrateRef,
    /// Source file containing the completion position.
    pub file_id: FileId,
    /// UTF-8 byte offset of the cursor in `source_text` and the matching analysis snapshot.
    pub offset: u32,
    /// Exact buffer for this snapshot. When omitted, completion reads the file from the snapshot.
    pub source_text: Option<&'a str>,
    completion_source: Option<&'a CompletionSource<'a>>,
    /// Client features used when building and applying completion items.
    pub client_capabilities: CompletionClientCapabilities,
}

impl<'a> CompletionQuery<'a> {
    pub fn new(crate_ref: CrateRef, file_id: FileId, offset: u32) -> Self {
        Self {
            crate_ref,
            file_id,
            offset,
            source_text: None,
            completion_source: None,
            client_capabilities: CompletionClientCapabilities::default(),
        }
    }

    pub fn with_source_text(mut self, source_text: &'a str) -> Self {
        self.source_text = Some(source_text);
        self.completion_source = None;
        self
    }

    /// Reuses the speculative syntax prepared for this source position.
    pub fn with_completion_source(mut self, completion_source: &'a CompletionSource<'a>) -> Self {
        self.source_text = Some(completion_source.source_text());
        self.completion_source = Some(completion_source);
        self
    }

    pub fn with_client_capabilities(
        mut self,
        client_capabilities: CompletionClientCapabilities,
    ) -> Self {
        self.client_capabilities = client_capabilities;
        self
    }

    pub(crate) fn completion_source(self) -> Option<&'a CompletionSource<'a>> {
        self.completion_source
            .filter(|source| source.offset() == self.offset)
    }
}

/// Source and request settings shared by completion providers after input preparation.
/// Text is always present, even when the caller supplied only a file and cursor.
#[derive(Clone, Copy)]
pub(crate) struct CompletionContext<'a> {
    pub(crate) crate_ref: CrateRef,
    pub(crate) file_id: FileId,
    pub(crate) offset: u32,
    pub(crate) source_text: &'a str,
    pub(crate) client_capabilities: CompletionClientCapabilities,
}

impl CompletionContext<'_> {
    /// Start the search budget when a resolver reaches an eligible prefix.
    pub(crate) fn auto_import_deadline(&self, prefix: &str) -> Option<Instant> {
        (self.client_capabilities.auto_import && ImportableNameSearch::supports_prefix(prefix))
            .then(|| Instant::now() + AUTO_IMPORT_BUDGET)
    }
}

/// Check the selected path and plan its `use` in the module containing the current cursor.
pub(crate) fn resolve_import(
    analysis: &Analysis<'_>,
    query: CompletionQuery<'_>,
    source: &str,
    import: &CompletionImport,
    selected: Option<(&str, CompletionKind)>,
) -> anyhow::Result<Option<CompletionAdditionalEdit>> {
    let Some(syntax) = CompletionSyntaxContext::at(source, query.offset) else {
        return Ok(None);
    };
    let db = analysis.view_db();
    let site = CompletionSiteDetector::new(analysis).site_at(
        query.crate_ref,
        query.file_id,
        query.offset,
        Some(syntax.site_syntax()),
    )?;
    let replace = match site {
        Some(CompletionSite::Unqualified(site)) if !import.anonymous => {
            if !CompletionCandidateSource::new(db).can_import_selected_name(
                &site,
                &import.path,
                selected,
            )? {
                return Ok(None);
            }
            site.replace_span()
        }
        Some(CompletionSite::Dot(site)) if import.anonymous => {
            let Some(context) = ImportContext::for_dot_site(db, site.source())? else {
                return Ok(None);
            };
            let Some(trait_ref) =
                ImportableNameSearch::new(db).trait_for_import(context.module(), &import.path)?
            else {
                return Ok(None);
            };
            if let Some((name, kind)) = selected {
                let receiver = site.source().receiver();
                let Some(ty) = BodyView::new(db).expr_ty(receiver.body_ir(), receiver.expr_id())?
                else {
                    return Ok(None);
                };
                if kind != CompletionKind::TraitMethod
                    || !MemberView::new(db).needs_method_import(
                        site.source().scope(),
                        &ty,
                        trait_ref,
                        name.strip_prefix("r#").unwrap_or(name),
                    )?
                {
                    return Ok(None);
                }
            }
            site.replace_span()
        }
        _ => return Ok(None),
    };

    let rendered_path = SyntaxRenderer::new(db.crate_edition(query.crate_ref)?)
        .path(&import.path)
        .to_string();
    // Build the edit against this buffer. The row's original replacement span may have moved
    // or may already contain the completed name.
    let planner =
        ImportEditPlanner::for_completion(&syntax, CompletionEdit { replace }, import.anonymous);
    Ok(match planner.plan(&import.path, &rendered_path) {
        ImportEditPlan::Edit(edit) => Some(CompletionAdditionalEdit {
            replace: edit.replace,
            new_text: edit.new_text,
        }),
        ImportEditPlan::AlreadyImported | ImportEditPlan::Unavailable => None,
    })
}
