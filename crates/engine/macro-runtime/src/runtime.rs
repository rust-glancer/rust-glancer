//! Resolved declarative macro expansion runtime.
//!
//! Callers still decide which macro definition is visible at a call site. Once they have that
//! answer, the runtime owns the rest of the expansion lifecycle: compile the definition, reuse or
//! prepare cached expansion work, execute pending work, and update the expansion cache.

use std::time::Duration;

use anyhow::Context as _;

use rg_ir_model::LocalDefRef;
use rg_ir_storage::MacroDefinitionData;
use rg_parse::{FileId, Span};
use rg_tt::TopSubtree;
use rg_workspace::RustEdition;

use super::{
    ExpansionParseKind, ExpansionSyntax,
    cache::{
        CachedPreparedMacroExpansion, MacroCompileRecord, MacroExpandRecord, MacroExpansionCache,
    },
    executor::{
        MacroExpansionExecutor, MacroExpansionJob, MacroExpansionOutput,
        MacroExpansionPerformancePreference, MacroExpansionWork,
    },
    syntax::{macro_edition, tt_span_for_parse_span},
};

/// Owns declarative macro compilation, expansion caching, and worker-pool execution.
pub struct MacroExpansionRuntime {
    cache: MacroExpansionCache,
    executor: Option<MacroExpansionExecutor>,
    performance_preference: MacroExpansionPerformancePreference,
}

impl MacroExpansionRuntime {
    pub fn new(performance_preference: MacroExpansionPerformancePreference) -> Self {
        Self {
            cache: MacroExpansionCache::default(),
            executor: None,
            performance_preference,
        }
    }

    /// Prepare one resolved macro call for use by a policy layer such as def-map finalization.
    pub fn prepare_expansion(
        &mut self,
        request: MacroExpansionRequest<'_>,
    ) -> PreparedMacroExpansionResult {
        let compile_result = self.cache.compile(
            request.def_ref,
            request.definition,
            macro_edition(request.definition.edition),
        );
        let Some(macro_) = compile_result.macro_ else {
            return PreparedMacroExpansionResult {
                expansion: PreparedMacroExpansion::Failed,
                compile: compile_result.record,
                expand: None,
            };
        };

        let call_site = tt_span_for_parse_span(
            request.call_file_id,
            request.call_span,
            macro_edition(request.call_edition),
        );
        let prepared_expansion = self.cache.prepare_expansion(
            request.def_ref,
            macro_,
            request.path_text,
            request.args,
            call_site,
            request.parse_kind,
        );

        let expansion = match prepared_expansion.expansion {
            CachedPreparedMacroExpansion::Syntax(syntax) => PreparedMacroExpansion::Syntax(syntax),
            CachedPreparedMacroExpansion::Failed => PreparedMacroExpansion::Failed,
            CachedPreparedMacroExpansion::Work(work) => {
                PreparedMacroExpansion::Pending(PendingMacroExpansion { work })
            }
        };

        PreparedMacroExpansionResult {
            expansion,
            compile: compile_result.record,
            expand: Some(prepared_expansion.record),
        }
    }

    /// Expand one resolved call immediately and cache the result.
    pub fn expand_now(&mut self, request: MacroExpansionRequest<'_>) -> Option<ExpansionSyntax> {
        match self.prepare_expansion(request).expansion {
            PreparedMacroExpansion::Syntax(syntax) => Some(syntax),
            PreparedMacroExpansion::Failed => None,
            PreparedMacroExpansion::Pending(pending) => {
                let syntax = pending.work.expand_syntax();
                let generated_syntax = syntax.generated_syntax;
                self.cache
                    .insert_expansion(syntax.key, generated_syntax.clone());
                generated_syntax
            }
        }
    }

    /// Execute pending expansion work and update the runtime cache before returning results.
    pub fn expand_pending_batch(
        &mut self,
        jobs: Vec<(usize, PendingMacroExpansion)>,
    ) -> anyhow::Result<Vec<CompletedMacroExpansion>> {
        if jobs.is_empty() {
            return Ok(Vec::new());
        }

        let jobs = jobs
            .into_iter()
            .map(|(id, pending)| MacroExpansionJob {
                id,
                work: pending.work,
            })
            .collect::<Vec<_>>();

        let outputs = self
            .executor()?
            .expand_jobs(jobs)
            .into_iter()
            .map(|output| self.record_output(output))
            .collect::<Vec<_>>();

        Ok(outputs)
    }

    fn executor(&mut self) -> anyhow::Result<&MacroExpansionExecutor> {
        if self.executor.is_none() {
            self.executor = Some(
                MacroExpansionExecutor::new(self.performance_preference)
                    .context("while attempting to initialize macro expansion runtime")?,
            );
        }

        Ok(self
            .executor
            .as_ref()
            .expect("macro expansion executor should be initialized"))
    }

    fn record_output(&mut self, output: MacroExpansionOutput) -> CompletedMacroExpansion {
        let generated_syntax = output.generated_syntax;
        self.cache
            .insert_expansion(output.key, generated_syntax.clone());

        CompletedMacroExpansion {
            id: output.id,
            macro_name: output.macro_name,
            elapsed: output.elapsed,
            generated_syntax,
        }
    }
}

impl Default for MacroExpansionRuntime {
    fn default() -> Self {
        Self::new(MacroExpansionPerformancePreference::default())
    }
}

/// A macro call after the caller has resolved the callee definition.
pub struct MacroExpansionRequest<'a> {
    pub def_ref: LocalDefRef,
    pub definition: &'a MacroDefinitionData,
    pub path_text: &'a str,
    pub args: &'a TopSubtree,
    pub call_file_id: FileId,
    pub call_span: Span,
    pub call_edition: RustEdition,
    pub parse_kind: ExpansionParseKind,
}

/// Prepared expansion payload together with compile/expand accounting events.
pub struct PreparedMacroExpansionResult {
    pub expansion: PreparedMacroExpansion,
    pub compile: MacroCompileRecord,
    pub expand: Option<MacroExpandRecord>,
}

/// Either already-expanded syntax, a known failed expansion, or pending runtime-owned work.
pub enum PreparedMacroExpansion {
    Syntax(ExpansionSyntax),
    Failed,
    Pending(PendingMacroExpansion),
}

/// Runtime-owned expansion work that callers may queue and execute later.
pub struct PendingMacroExpansion {
    work: MacroExpansionWork,
}

/// Completed expansion work after runtime cache insertion.
pub struct CompletedMacroExpansion {
    pub id: usize,
    pub macro_name: String,
    pub elapsed: Duration,
    pub generated_syntax: Option<ExpansionSyntax>,
}
