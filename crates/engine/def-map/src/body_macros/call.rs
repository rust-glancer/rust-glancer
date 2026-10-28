use std::sync::Arc;

use rg_cfg_eval::CfgEvaluator;
use rg_ir_model::{BodySource, CrateRef, FileId, LocalDefRef, ModuleRef, PackageSlot, Span};
use rg_item_tree::BuiltinMacroKind;
use rg_macro_runtime::{DeclarativeMacroDefinition, ExpansionParseKind, MacroExpansionRequest};
use rg_parse::syntax_edition;
use rg_syntax::{TextRange, ast, utils::normalized_syntax_text};
use rg_text::RustEdition;
use rg_tt::{
    TopSubtree,
    syntax_bridge::{ExpansionSpanMap, SpanFactory, syntax_node_to_token_tree_with_span},
};

use super::ExpandedBodyMacro;
use crate::MacroDefinitionView;

/// Source context carried from one body macro invocation to the next.
///
/// Calls parsed from an expansion need both the crate for `$crate` lookup and the parent token
/// map. The map lets a nested call forward original argument locations into its own expansion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BodyMacroCallOrigin {
    /// A macro invocation written in the original source file.
    Source,
    /// A call found in expanded syntax, including calls copied from invocation arguments.
    Generated {
        dollar_crate: CrateRef,
        /// Maps offsets in the parent expansion's text back to original token spans.
        source_map: Arc<ExpansionSpanMap>,
    },
}

impl BodyMacroCallOrigin {
    fn dollar_crate_for_path(&self) -> Option<CrateRef> {
        match self {
            Self::Source => None,
            Self::Generated { dollar_crate, .. } => Some(*dollar_crate),
        }
    }

    fn dollar_crate_for_expansion(&self, caller_target: CrateRef) -> CrateRef {
        match self {
            Self::Source => caller_target,
            Self::Generated { dollar_crate, .. } => *dollar_crate,
        }
    }
}

/// Snapshot of the source context needed to resolve and expand one body macro call.
#[derive(Clone)]
pub struct BodyMacroCallSite<'cfg> {
    module: ModuleRef,
    source: BodySource,
    origin: BodyMacroCallOrigin,
    edition: RustEdition,
    cfg: Option<CfgEvaluator<'cfg>>,
}

impl<'cfg> BodyMacroCallSite<'cfg> {
    /// Creates a call site for macro positions that do not need cfg-sensitive builtin expansion.
    pub fn new(
        module: ModuleRef,
        source: BodySource,
        origin: BodyMacroCallOrigin,
        edition: RustEdition,
    ) -> Self {
        Self {
            module,
            source,
            origin,
            edition,
            cfg: None,
        }
    }

    /// Attaches cfg evaluation for body positions where `cfg_select!` can select generated syntax.
    pub fn with_cfg(self, cfg: CfgEvaluator<'cfg>) -> Self {
        Self {
            cfg: Some(cfg),
            ..self
        }
    }

    pub(super) fn crate_ref(&self) -> Option<CrateRef> {
        self.module.origin.as_crate_ref()
    }

    pub(super) fn module(&self) -> ModuleRef {
        self.module
    }

    pub(super) fn cfg(&self) -> Option<CfgEvaluator<'cfg>> {
        self.cfg
    }

    /// Prepare to visit calls found in this expansion's output.
    ///
    /// Keep the original written call's file and range. The nested syntax has generated offsets,
    /// so its tokens need this expansion's map to recover original argument positions.
    /// Its `$crate` paths belong to the macro definition that produced the output.
    pub(super) fn in_expansion<T>(&self, expanded: &ExpandedBodyMacro<T>) -> Self {
        Self {
            origin: BodyMacroCallOrigin::Generated {
                dollar_crate: expanded.dollar_crate(),
                source_map: expanded.source_map(),
            },
            ..self.clone()
        }
    }

    pub(super) fn source(&self) -> BodySource {
        self.source
    }

    pub(super) fn path_text(&self, call: &ast::MacroCall) -> Option<String> {
        call.path().map(|path| normalized_syntax_text(&path))
    }

    pub(super) fn invocation(
        &self,
        path_text: String,
        call: &ast::MacroCall,
    ) -> Option<BodyMacroInvocation> {
        BodyMacroInvocation::from_ast(
            self.module.origin.origin_crate().package,
            self.source.file_id,
            self.source.span,
            self.edition,
            &self.origin,
            path_text,
            call,
        )
    }

    pub(super) fn dollar_crate_for_path(&self) -> Option<CrateRef> {
        self.origin.dollar_crate_for_path()
    }

    pub(super) fn dollar_crate_for_expansion(&self) -> Option<CrateRef> {
        Some(self.origin.dollar_crate_for_expansion(self.crate_ref()?))
    }
}

/// Body macro call after path lookup has selected the callee.
pub(super) struct ResolvedBodyMacroCall<'a> {
    path_text: String,
    pub(super) callee: BodyMacroCallee<'a>,
}

impl<'a> ResolvedBodyMacroCall<'a> {
    pub(super) fn new(path_text: String, callee: BodyMacroCallee<'a>) -> Self {
        Self { path_text, callee }
    }

    pub(super) fn invocation(
        &self,
        site: &BodyMacroCallSite<'_>,
        call: &ast::MacroCall,
    ) -> Option<BodyMacroInvocation> {
        site.invocation(self.path_text.clone(), call)
    }

    pub(super) fn definition(&self) -> LocalDefRef {
        self.callee.def_ref()
    }
}

/// Macro implementation selected for one body call.
pub(super) enum BodyMacroCallee<'a> {
    Declarative(MacroDefinitionView<'a>),
    Builtin {
        def_ref: LocalDefRef,
        kind: BuiltinMacroKind,
    },
}

impl BodyMacroCallee<'_> {
    pub(super) fn def_ref(&self) -> LocalDefRef {
        match self {
            Self::Declarative(resolved) => resolved.def_ref,
            Self::Builtin { def_ref, .. } => *def_ref,
        }
    }
}

/// Body-specific adapter from parsed macro-call syntax to runtime expansion input.
///
/// Item-position calls are already lowered by item-tree before def-map expansion sees them. Bodies
/// arrive here as `ast::MacroCall`, so this private adapter keeps the AST and token-tree conversion
/// next to the body visibility policy instead of making `rg_macro_runtime` depend on parsed AST.
pub(super) struct BodyMacroInvocation {
    path_text: String,
    args: TopSubtree,
    call_package: PackageSlot,
    call_file_id: FileId,
    call_span: Span,
    call_edition: RustEdition,
}

impl BodyMacroInvocation {
    fn from_ast(
        package: PackageSlot,
        file_id: FileId,
        span: Span,
        edition: RustEdition,
        origin: &BodyMacroCallOrigin,
        path_text: String,
        call: &ast::MacroCall,
    ) -> Option<Self> {
        let args = call.token_tree()?;

        let span_factory = SpanFactory::new(
            u32::try_from(package.0).expect("package slot should fit macro span storage"),
            u32::try_from(file_id.0).expect("file id should fit macro span storage"),
            syntax_edition(edition),
        );
        // Nested calls live in expanded text. Reuse the parent spans so tokens keep their original
        // package, file, and range, whether they came from invocation input or a macro definition.
        let args = syntax_node_to_token_tree_with_span(&args, &mut |range| match origin {
            BodyMacroCallOrigin::Source => span_factory.span_for(range),
            BodyMacroCallOrigin::Generated { source_map, .. } => {
                source_map.span_for_range(range).unwrap_or_else(|| {
                    span_factory.span_for(TextRange::new(span.start.into(), span.end.into()))
                })
            }
        });

        Some(Self {
            path_text,
            args,
            call_package: package,
            call_file_id: file_id,
            call_span: span,
            call_edition: edition,
        })
    }

    pub(super) fn args(&self) -> &TopSubtree {
        &self.args
    }

    pub(super) fn call_package(&self) -> PackageSlot {
        self.call_package
    }

    pub(super) fn expansion_request<'a>(
        &'a self,
        def_ref: LocalDefRef,
        definition: DeclarativeMacroDefinition<'a>,
        parse_kind: ExpansionParseKind,
    ) -> MacroExpansionRequest<'a> {
        MacroExpansionRequest {
            def_ref,
            definition,
            path_text: &self.path_text,
            args: &self.args,
            call_package: self.call_package,
            call_file_id: self.call_file_id,
            call_span: self.call_span,
            call_edition: self.call_edition,
            parse_kind,
        }
    }
}
