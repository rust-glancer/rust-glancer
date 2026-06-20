//! Caches compiled macro definitions and repeated expansion inputs.
//!
//! A macro definition can be called many times, and identical calls can also appear across targets.
//! The cache keeps the expensive macro parser and expander work out of fixed-point collectors while
//! returning small records that callers can fold into their own accounting.

use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::Context as _;

use rg_ir_model::LocalDefRef;
use rg_ir_storage::{MacroDefinitionData, MacroDefinitionPayload};
use rg_macro_expand::{DeclarativeMacro, Edition, ExpansionParseKind, ExpansionSyntax};
use rg_tt::{Span as TtSpan, TopSubtree};

use super::executor::MacroExpansionWork;

/// Per-finalization cache for macro definitions and expanded syntax.
#[derive(Default)]
pub(crate) struct MacroExpansionCache {
    compiled: HashMap<LocalDefRef, Option<Arc<DeclarativeMacro>>>,
    expanded: HashMap<MacroExpansionCacheKey, Option<ExpansionSyntax>>,
}

impl MacroExpansionCache {
    /// Compiles a macro definition once and remembers failures as well as successes.
    pub(crate) fn compile(
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
        let compiled = Self::compile_macro(macro_definition, edition);
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
    pub(crate) fn prepare_expansion(
        &mut self,
        def_ref: LocalDefRef,
        macro_: Arc<DeclarativeMacro>,
        path_text: &str,
        args: &TopSubtree,
        call_site: TtSpan,
        parse_kind: ExpansionParseKind,
    ) -> PreparedMacroExpansionResult {
        let key = MacroExpansionCacheKey {
            def_ref,
            args: args.clone(),
            call_site,
            parse_kind,
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
                parse_kind,
            }),
            record: MacroExpandRecord::Attempt,
        }
    }

    pub(crate) fn insert_expansion(
        &mut self,
        key: MacroExpansionCacheKey,
        syntax: Option<ExpansionSyntax>,
    ) {
        self.expanded.insert(key, syntax);
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
}

/// Compiled macro payload together with the accounting event produced while fetching it.
pub(crate) struct MacroCompileResult {
    pub(crate) macro_: Option<Arc<DeclarativeMacro>>,
    pub(crate) record: MacroCompileRecord,
}

/// Stats event for one macro-definition compile lookup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MacroCompileRecord {
    CacheHit { failed: bool },
    Attempt { elapsed: Duration, failed: bool },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct MacroExpansionCacheKey {
    pub(crate) def_ref: LocalDefRef,
    pub(crate) args: TopSubtree,
    pub(crate) call_site: TtSpan,
    pub(crate) parse_kind: ExpansionParseKind,
}

/// Expansion payload together with the accounting event produced while preparing it.
pub(crate) struct PreparedMacroExpansionResult {
    pub(crate) expansion: PreparedMacroExpansion,
    pub(crate) record: MacroExpandRecord,
}

/// Either already-expanded syntax, a known failed expansion, or work to run in parallel.
pub(crate) enum PreparedMacroExpansion {
    Syntax(ExpansionSyntax),
    Failed,
    Work(MacroExpansionWork),
}

/// Stats event for one macro-call expansion lookup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MacroExpandRecord {
    CacheHit { failed: bool },
    Attempt,
}
