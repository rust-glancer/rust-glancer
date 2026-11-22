use std::sync::Arc;

use rg_cfg_eval::CfgEvaluator;
use rg_ir_model::{BodySource, CrateRef, FileId, LocalDefRef, ModuleRef, PackageSlot, Span};
use rg_item_tree::BuiltinMacroKind;
use rg_macro_runtime::{DeclarativeMacroDefinition, ExpansionParseKind, MacroExpansionRequest};
use rg_parse::syntax_edition;
use rg_syntax::{AstNode as _, SyntaxNode, TextRange, ast, utils::normalized_syntax_text};
use rg_text::RustEdition;
use rg_tt::{
    Span as TtSpan, TokenOrigins, TokenOwner, TopSubtree,
    syntax_bridge::{ExpansionSpanMap, SpanFactory, syntax_node_to_token_tree_with_span},
};

use super::{BodyMacroInputSources, ExpandedBodyMacro, expanded::BodyMacroInputMap};
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
        input_sources: BodyMacroInputSources,
        call_site: TtSpan,
    },
}

impl BodyMacroCallOrigin {
    fn dollar_crate_for_path(&self, call: &ast::MacroCall) -> Option<CrateRef> {
        match self {
            Self::Source => None,
            Self::Generated {
                dollar_crate,
                source_map,
                ..
            } => call
                .path()
                .and_then(|path| path.first_segment())
                .and_then(|segment| segment.name_ref())
                .filter(|name| name.text() == "$crate")
                .and_then(|name| name.syntax().first_token())
                .and_then(|token| {
                    ExpandedBodyMacro::<SyntaxNode>::mapped_dollar_crate(
                        source_map,
                        token.text_range(),
                    )
                })
                .or(Some(*dollar_crate)),
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
            origin: expanded.call_origin(),
            ..self.clone()
        }
    }

    pub(super) fn source(&self) -> BodySource {
        self.source
    }

    pub(super) fn path_text(&self, call: &ast::MacroCall) -> Option<String> {
        call.path().map(|path| normalized_syntax_text(&path))
    }

    /// A call such as `assert!(ok, "{user}")` can capture names under Rust 2021 panic rules.
    /// Require a modern caller and a modern macro path in generated code. Some standard
    /// wrappers use the caller's edition for panic behavior.
    pub(super) fn assert_formats_single_message(&self, call: &ast::MacroCall) -> bool {
        if !syntax_edition(self.edition).at_least_2021() {
            return false;
        }
        match &self.origin {
            BodyMacroCallOrigin::Source => true,
            BodyMacroCallOrigin::Generated { source_map, .. } => call
                .path()
                .and_then(|path| path.syntax().first_token())
                .and_then(|token| source_map.span_for_range(token.text_range()))
                .is_some_and(|span| span.ctx.edition().at_least_2021()),
        }
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

    pub(super) fn dollar_crate_for_path(&self, call: &ast::MacroCall) -> Option<CrateRef> {
        self.origin.dollar_crate_for_path(call)
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
    call_site: TtSpan,
    input_sources: BodyMacroInputSources,
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
            u32::try_from(package.0).expect("package slot fits macro origins"),
            file_id.as_raw(),
            TokenOwner::Body,
            syntax_edition(edition),
        );
        let mut sources = BodyMacroInputMap::default();
        let call_site = sources.record(
            span_factory,
            TextRange::new(span.start.into(), span.end.into()),
            None,
        );
        let args = match origin {
            BodyMacroCallOrigin::Source => {
                syntax_node_to_token_tree_with_span(&args, &mut |range| {
                    sources.record(span_factory, range, None)
                })
            }
            BodyMacroCallOrigin::Generated {
                source_map,
                input_sources,
                call_site: parent_call_site,
                ..
            } => syntax_node_to_token_tree_with_span(&args, &mut |range| {
                let mut token = source_map
                    .span_for_range(range)
                    .unwrap_or(*parent_call_site);
                if token.anchor == parent_call_site.anchor
                    && let Some(source) = input_sources.span(token)
                    && source != span
                    && span.contains_span(source)
                {
                    // If `outer!(x)` emits `inner!(x, x)`, give each copy its own input ID
                    // even though both point to the same written x. Preserve its hygiene.
                    let source = TextRange::new(source.start.into(), source.end.into());
                    let forwarded = sources.record(span_factory, source, Some(range));
                    return TtSpan {
                        ctx: token.ctx,
                        ..forwarded
                    };
                }
                if token.anchor == call_site.anchor {
                    // An unmapped token must never address the new table using an old ID.
                    token.origins = TokenOrigins::synthetic(call_site.origins.first());
                }
                token
            }),
        };
        let input_sources = BodyMacroInputSources::Body(Arc::new(sources));

        Some(Self {
            path_text,
            args,
            call_package: package,
            call_site,
            input_sources,
        })
    }

    pub(super) fn call_site(&self) -> TtSpan {
        self.call_site
    }

    pub(super) fn args(&self) -> &TopSubtree {
        &self.args
    }

    pub(super) fn input_sources(&self) -> BodyMacroInputSources {
        self.input_sources.clone()
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
            call_site: self.call_site,
            parse_kind,
        }
    }
}
