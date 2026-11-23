//! Caches compiled macro definitions and repeated expansion inputs.
//!
//! A macro definition can be called many times, and identical calls can also appear across targets.
//! The cache keeps the expensive macro parser and expander work out of fixed-point collectors while
//! returning small records that callers can fold into their own accounting.
//!
//! Expansion caching stays transient: saved declarations retain the lowered result, while
//! intermediate token streams would duplicate much of that data.

use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

use rg_ir_model::LocalDefRef;
use rg_macro_expand::{DeclarativeMacro, ExpansionParseKind, ExpansionSyntax};
use rg_tt::{Span as TtSpan, TopSubtree};

use super::{DeclarativeMacroDefinition, executor::MacroExpansionWork};

/// Per-build cache for compiled macro definitions and expanded syntax.
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
        definition: DeclarativeMacroDefinition<'_>,
    ) -> CachedMacroCompileResult {
        if let Some(compiled) = self.compiled.get(&def_ref) {
            return CachedMacroCompileResult {
                macro_: compiled.clone(),
                record: MacroCompileRecord::CacheHit {
                    failed: compiled.is_none(),
                },
            };
        }

        let started_at = Instant::now();
        let compiled = match definition {
            DeclarativeMacroDefinition::MacroRules { body, .. } => {
                body.map(DeclarativeMacro::from_macro_rules_tokens)
            }
            DeclarativeMacroDefinition::MacroDef { args, body, .. } => {
                body.map(|body| DeclarativeMacro::from_macro_def_tokens(args, body))
            }
        };
        let elapsed = started_at.elapsed();

        match compiled {
            Some(mut compiled) => {
                let origin = definition.dollar_crate();
                compiled.bind_dollar_crate(
                    u32::try_from(origin.package.0)
                        .expect("macro definition package fits its span"),
                    u32::try_from(origin.crate_id.0)
                        .expect("macro definition target fits its span"),
                );
                let compiled = Arc::new(compiled);
                self.compiled.insert(def_ref, Some(Arc::clone(&compiled)));
                CachedMacroCompileResult {
                    macro_: Some(compiled),
                    record: MacroCompileRecord::Attempt {
                        elapsed,
                        failed: false,
                    },
                }
            }
            None => {
                self.compiled.insert(def_ref, None);
                CachedMacroCompileResult {
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
    ) -> CachedMacroExpansionPreparation {
        let key = MacroExpansionCacheKey {
            def_ref,
            args: args.clone(),
            call_site,
            parse_kind,
        };

        if let Some(expanded) = self.expanded.get(&key) {
            let expansion = match expanded {
                Some(syntax) => CachedPreparedMacroExpansion::Syntax(syntax.clone()),
                None => CachedPreparedMacroExpansion::Failed,
            };
            return CachedMacroExpansionPreparation {
                expansion,
                record: MacroExpandRecord::CacheHit {
                    failed: expanded.is_none(),
                },
            };
        }

        CachedMacroExpansionPreparation {
            expansion: CachedPreparedMacroExpansion::Work(Box::new(MacroExpansionWork {
                key,
                macro_name: path_text.to_string(),
                macro_,
                args: args.clone(),
                call_site,
                parse_kind,
            })),
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
}

/// Compiled macro payload together with the accounting event produced while fetching it.
pub(crate) struct CachedMacroCompileResult {
    pub(crate) macro_: Option<Arc<DeclarativeMacro>>,
    pub(crate) record: MacroCompileRecord,
}

/// Stats event for one macro-definition compile lookup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MacroCompileRecord {
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
pub(crate) struct CachedMacroExpansionPreparation {
    pub(crate) expansion: CachedPreparedMacroExpansion,
    pub(crate) record: MacroExpandRecord,
}

/// Either already-expanded syntax, a known failed expansion, or work to run in parallel.
pub(crate) enum CachedPreparedMacroExpansion {
    Syntax(ExpansionSyntax),
    Failed,
    Work(Box<MacroExpansionWork>),
}

/// Stats event for one macro-call expansion lookup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MacroExpandRecord {
    CacheHit { failed: bool },
    Attempt,
}
