use rg_cfg_eval::CfgEvaluator;
use rg_ir_model::{BodySource, LocalDefRef, ModuleRef, TargetRef, items::BuiltinMacroKind};
use rg_ir_storage::{MacroDefinitionData, MacroDefinitionView};
use rg_macro_runtime::{ExpansionParseKind, MacroExpansionRequest, macro_edition};
use rg_parse::{FileId, Span};
use rg_syntax::{ast, utils::normalized_syntax_text};
use rg_tt::TopSubtree;
use rg_tt::syntax_bridge::{SpanFactory, syntax_node_to_token_tree_with_span};
use rg_workspace::RustEdition;

/// Tells body macro lookup whether the call came from user-written syntax or generated syntax.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyMacroCallOrigin {
    /// A macro invocation written in the original source file.
    Source,
    /// A macro invocation produced by expanding syntax from a macro definition target.
    Generated { dollar_crate_target: TargetRef },
}

impl BodyMacroCallOrigin {
    fn dollar_crate_target_for_path(self) -> Option<TargetRef> {
        match self {
            Self::Source => None,
            Self::Generated {
                dollar_crate_target,
            } => Some(dollar_crate_target),
        }
    }

    fn dollar_crate_target_for_expansion(self, caller_target: TargetRef) -> TargetRef {
        match self {
            Self::Source => caller_target,
            Self::Generated {
                dollar_crate_target,
            } => dollar_crate_target,
        }
    }
}

/// Snapshot of the source context needed to resolve and expand one body macro call.
#[derive(Clone, Copy)]
pub struct BodyMacroCallSite<'cfg> {
    target: TargetRef,
    module: ModuleRef,
    source: BodySource,
    origin: BodyMacroCallOrigin,
    edition: RustEdition,
    cfg: Option<CfgEvaluator<'cfg>>,
}

impl<'cfg> BodyMacroCallSite<'cfg> {
    /// Creates a call site for macro positions that do not need cfg-sensitive builtin expansion.
    pub fn new(
        target: TargetRef,
        module: ModuleRef,
        source: BodySource,
        origin: BodyMacroCallOrigin,
        edition: RustEdition,
    ) -> Self {
        Self {
            target,
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

    pub(super) fn target(self) -> TargetRef {
        self.target
    }

    pub(super) fn module(self) -> ModuleRef {
        self.module
    }

    pub(super) fn cfg(self) -> Option<CfgEvaluator<'cfg>> {
        self.cfg
    }

    pub(super) fn invocation(self, call: &ast::MacroCall) -> Option<BodyMacroInvocation> {
        BodyMacroInvocation::from_ast(self.source.file_id, self.source.span, self.edition, call)
    }

    pub(super) fn dollar_crate_target_for_path(self) -> Option<TargetRef> {
        self.origin.dollar_crate_target_for_path()
    }

    pub(super) fn dollar_crate_target_for_expansion(self) -> TargetRef {
        self.origin.dollar_crate_target_for_expansion(self.target)
    }
}

/// Body macro call after parsing and normalizing macro lookup.
pub(super) struct ResolvedBodyMacroCall<'a> {
    pub(super) invocation: BodyMacroInvocation,
    pub(super) callee: BodyMacroCallee<'a>,
}

impl<'a> ResolvedBodyMacroCall<'a> {
    pub(super) fn new(invocation: BodyMacroInvocation, callee: BodyMacroCallee<'a>) -> Self {
        Self { invocation, callee }
    }
}

/// Macro implementation selected for one body call.
pub(super) enum BodyMacroCallee<'a> {
    Declarative(MacroDefinitionView<'a>),
    Builtin(BuiltinMacroKind),
}

/// Body-specific adapter from parsed macro-call syntax to runtime expansion input.
///
/// Item-position calls are already lowered by item-tree before def-map expansion sees them. Bodies
/// arrive here as `ast::MacroCall`, so this private adapter keeps the AST and token-tree conversion
/// next to the body visibility policy instead of making `rg_macro_runtime` depend on parsed AST.
pub(super) struct BodyMacroInvocation {
    path_text: String,
    args: TopSubtree,
    call_file_id: FileId,
    call_span: Span,
    call_edition: RustEdition,
}

impl BodyMacroInvocation {
    fn from_ast(
        file_id: FileId,
        span: Span,
        edition: RustEdition,
        call: &ast::MacroCall,
    ) -> Option<Self> {
        let path_text = call.path().map(|path| normalized_syntax_text(&path))?;
        let args = call.token_tree()?;

        let span_factory = SpanFactory::new(
            u32::try_from(file_id.0).expect("file id should fit macro span storage"),
            macro_edition(edition),
        );
        let args =
            syntax_node_to_token_tree_with_span(&args, &mut |range| span_factory.span_for(range));

        Some(Self {
            path_text,
            args,
            call_file_id: file_id,
            call_span: span,
            call_edition: edition,
        })
    }

    pub(super) fn path_text(&self) -> &str {
        &self.path_text
    }

    pub(super) fn args(&self) -> &TopSubtree {
        &self.args
    }

    pub(super) fn expansion_request<'a>(
        &'a self,
        def_ref: LocalDefRef,
        definition: &'a MacroDefinitionData,
        parse_kind: ExpansionParseKind,
    ) -> MacroExpansionRequest<'a> {
        MacroExpansionRequest {
            def_ref,
            definition,
            path_text: &self.path_text,
            args: &self.args,
            call_file_id: self.call_file_id,
            call_span: self.call_span,
            call_edition: self.call_edition,
            parse_kind,
        }
    }
}
