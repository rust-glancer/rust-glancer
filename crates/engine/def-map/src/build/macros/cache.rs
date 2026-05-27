//! Caches compiled macro definitions and repeated expansion inputs.
//!
//! A macro definition can be called many times, and identical calls can also appear across targets.
//! The cache keeps the expensive macro parser and expander work out of the def-map fixed-point loop
//! while returning small records that the caller can fold into finalization stats.

use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::Context as _;

use rg_ir_model::LocalDefRef;
use rg_macro_expand::{DeclarativeMacro, Edition, ExpansionSyntax};
use rg_tt::{Span as TtSpan, TopSubtree};

use crate::{MacroDefinitionData, MacroDefinitionPayload};

use super::expand::MacroExpansionWork;

/// Per-finalization cache for macro definitions and expanded syntax.
#[derive(Default)]
pub(crate) struct MacroExpansionCache {
    compiled: HashMap<LocalDefRef, Option<Arc<DeclarativeMacro>>>,
    expanded: HashMap<MacroExpansionCacheKey, Option<ExpansionSyntax>>,
}

impl MacroExpansionCache {
    /// Compiles a macro definition once and remembers failures as well as successes.
    pub(super) fn compile(
        &mut self,
        def_ref: LocalDefRef,
        macro_definition: &MacroDefinitionData,
        edition: Edition,
    ) -> MacroCompileResult {
        if self.compiled.contains_key(&def_ref) {
            let macro_ = self.compiled.get(&def_ref).and_then(Clone::clone);
            let failed = self
                .compiled
                .get(&def_ref)
                .is_some_and(|compiled| compiled.is_none());
            return MacroCompileResult {
                macro_,
                record: MacroCompileRecord::CacheHit { failed },
            };
        }

        let started_at = Instant::now();
        let compiled = compile_macro(macro_definition, edition);
        let elapsed = started_at.elapsed();

        match compiled {
            Ok(compiled) => {
                let compiled = Arc::new(compiled);
                self.compiled.insert(def_ref, Some(Arc::clone(&compiled)));
                MacroCompileResult {
                    macro_: Some(compiled),
                    record: MacroCompileRecord::Attempt {
                        elapsed,
                        failed: false,
                    },
                }
            }
            Err(_) => {
                self.compiled.insert(def_ref, None);
                MacroCompileResult {
                    macro_: None,
                    record: MacroCompileRecord::Attempt {
                        elapsed,
                        failed: true,
                    },
                }
            }
        }
    }

    /// Returns a cached expansion result or packages new expansion work for the worker pool.
    pub(super) fn prepare_expansion(
        &mut self,
        def_ref: LocalDefRef,
        macro_: Arc<DeclarativeMacro>,
        path_text: &str,
        args: &TopSubtree,
        call_site: TtSpan,
    ) -> PreparedMacroExpansionResult {
        let key = MacroExpansionCacheKey {
            def_ref,
            args: args.clone(),
            call_site,
        };

        if let Some(expanded) = self.expanded.get(&key) {
            let expansion = match expanded {
                Some(syntax) => PreparedMacroExpansion::Syntax(syntax.clone()),
                None => PreparedMacroExpansion::Failed,
            };
            return PreparedMacroExpansionResult {
                expansion,
                record: MacroExpandRecord::CacheHit {
                    failed: expanded.is_none(),
                },
            };
        }

        PreparedMacroExpansionResult {
            expansion: PreparedMacroExpansion::Work(MacroExpansionWork {
                key,
                macro_name: path_text.to_string(),
                macro_,
                args: args.clone(),
                call_site,
            }),
            record: MacroExpandRecord::Attempt,
        }
    }

    pub(super) fn insert_expansion(
        &mut self,
        key: MacroExpansionCacheKey,
        syntax: Option<ExpansionSyntax>,
    ) {
        self.expanded.insert(key, syntax);
    }
}

/// Compiled macro payload together with the accounting event produced while fetching it.
pub(super) struct MacroCompileResult {
    pub(super) macro_: Option<Arc<DeclarativeMacro>>,
    pub(super) record: MacroCompileRecord,
}

/// Stats event for one macro-definition compile lookup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MacroCompileRecord {
    CacheHit { failed: bool },
    Attempt { elapsed: Duration, failed: bool },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct MacroExpansionCacheKey {
    pub(super) def_ref: LocalDefRef,
    pub(super) args: TopSubtree,
    pub(super) call_site: TtSpan,
}

/// Expansion payload together with the accounting event produced while preparing it.
pub(super) struct PreparedMacroExpansionResult {
    pub(super) expansion: PreparedMacroExpansion,
    pub(super) record: MacroExpandRecord,
}

/// Either already-expanded syntax, a known failed expansion, or work to run in parallel.
pub(super) enum PreparedMacroExpansion {
    Syntax(ExpansionSyntax),
    Failed,
    Work(MacroExpansionWork),
}

/// Stats event for one macro-call expansion lookup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MacroExpandRecord {
    CacheHit { failed: bool },
    Attempt,
}

fn compile_macro(
    macro_definition: &MacroDefinitionData,
    edition: Edition,
) -> anyhow::Result<DeclarativeMacro> {
    match &macro_definition.payload {
        MacroDefinitionPayload::MacroRules { body } => {
            let body = body
                .as_ref()
                .context("while attempting to fetch macro_rules body")?;
            DeclarativeMacro::from_macro_rules_tokens(body, edition)
        }
        MacroDefinitionPayload::MacroDef { args, body } => {
            let body = body
                .as_ref()
                .context("while attempting to fetch macro body")?;
            DeclarativeMacro::from_macro_def_tokens(args.as_ref(), body, edition)
        }
    }
}
