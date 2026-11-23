//! File-local AST lowering. Source discovery interprets module and include paths separately.

use std::{cell::RefCell, sync::Arc};

use rg_arena::Arena;
use rg_ir_model::{FileId, PackageSlot, Span};
use rg_macro_expand::CfgSelect;
use rg_parse::syntax_edition;
use rg_syntax::{
    AstNode as _, Parse, TextRange, TextSize,
    ast::{self, HasModuleItem as _},
};
use rg_text::RustEdition;
use rg_tt::{
    Span as TtSpan, TokenOwner,
    syntax_bridge::{ExpansionSpanMap, SpanFactory},
};

use crate::{
    BuiltinMacroItem, CfgSelectArmItem, DeclarationSourceStore, Documentation, ExpandedItemBodies,
    FileTree, IncludePathExpression, InnerDocs, ItemKind, ItemLowering, ItemMacroSpans, ItemNode,
    ItemTreeId, LoweredItem, MaybeFromAst, RecordSource,
};

/// Build declaration meaning and its source table in the same traversal.
///
/// Item nodes keep the IDs returned by their recorder. The separate table keeps file positions,
/// including positions of input tokens retained by macro definitions and calls. Lowering a
/// `cfg_select!` arm uses the same recorder with a mapper back to that arm's original file text.
pub(super) struct FileLowering<'a> {
    edition: RustEdition,
    expansion: Option<&'a FileExpansionSource>,
}

impl FileLowering<'_> {
    pub(super) fn lower(
        package: PackageSlot,
        file: FileId,
        edition: RustEdition,
        syntax: &Parse<ast::SourceFile>,
    ) -> (
        FileTree,
        DeclarationSourceStore<Span>,
        Vec<ExpandedItemBodies>,
    ) {
        let package = u32::try_from(package.0).expect("package slot should fit macro span storage");
        let mut builder = FileTreeBuilder::new(package, file);
        let source_file = syntax.tree();
        let docs =
            <Documentation as MaybeFromAst<InnerDocs>>::maybe_from_ast(&source_file, InnerDocs);
        let top_level = Self {
            edition,
            expansion: None,
        }
        .collect_items(&mut builder, source_file.items());
        // TODO: Track source-sensitive attributes when attribute macro expansion is supported.
        (
            FileTree {
                file,
                docs,
                top_level,
                items: builder.items,
                source_sensitive: builder.source_sensitive
                    || Self::has_declaration_parse_errors(syntax),
            },
            builder.sources,
            builder.cfg_select_bodies,
        )
    }

    /// Recovery inside a closed function body or terminated initializer belongs to body queries.
    /// An incomplete declaration can change how neighboring syntax is parsed, so keep its text
    /// dependency.
    fn has_declaration_parse_errors(syntax: &Parse<ast::SourceFile>) -> bool {
        // Additional syntax validation does not change the recovered tree.
        let parse_errors = syntax.parse_errors();
        if parse_errors.is_empty() {
            return false;
        }

        let bodies: Vec<_> = syntax
            .tree()
            .syntax()
            .descendants()
            .filter_map(|node| {
                let body = match ast::Item::cast(node)? {
                    ast::Item::Fn(function) => {
                        let body = function.body()?;
                        let statements = body.stmt_list()?;
                        statements.l_curly_token()?;
                        statements.r_curly_token()?;
                        ast::Expr::BlockExpr(body)
                    }
                    // The semicolon closes an initializer independently of the expression form.
                    // Errors in the annotation remain outside this range and still affect headers.
                    ast::Item::Const(item) => {
                        item.semicolon_token()?;
                        item.body()?
                    }
                    ast::Item::Static(item) => {
                        item.semicolon_token()?;
                        item.body()?
                    }
                    _ => return None,
                };
                Some(body.syntax().text_range())
            })
            .collect();
        parse_errors
            .iter()
            .any(|error| !bodies.iter().any(|body| body.contains_range(error.range())))
    }

    /// Tokens entering macro expansion share the declaration's occurrence table. For a parsed
    /// `cfg_select!` arm, reuse the input origin instead of allocating an origin in temporary text.
    fn tt_span_for_range(
        &self,
        builder: &FileTreeBuilder,
        range: TextRange,
        edition: rg_tt::Edition,
        source: &mut dyn RecordSource,
    ) -> TtSpan {
        if let Some(span) = self
            .expansion
            .and_then(|source| source.tokens.span_for_range(range))
        {
            return span;
        }

        let source = source.record(range);
        SpanFactory::new(
            builder.package,
            builder.current_file_id.as_raw(),
            TokenOwner::Declaration(source.owner),
            edition,
        )
        .span_for(source.local)
    }

    fn collect_items(
        &self,
        builder: &mut FileTreeBuilder,
        items: impl Iterator<Item = ast::Item>,
    ) -> Vec<ItemTreeId> {
        items.map(|item| self.lower_item(builder, item)).collect()
    }

    fn lower_item(&self, builder: &mut FileTreeBuilder, item: ast::Item) -> ItemTreeId {
        let mut map = |range: TextRange, _role| match self.expansion {
            Some(expansion) => expansion.span(range),
            None => Span::from_text_range(range),
        };
        let mut recorder = builder.sources.start_owner(&mut map);
        let children =
            ItemLowering::lower_children(&item, |child| Some(self.lower_item(builder, child)));
        let macro_edition = syntax_edition(self.edition);
        let LoweredItem { node, body_range } = ItemLowering {
            file_id: builder.current_file_id,
            dollar_crate: None,
            macro_spans: Some(ItemMacroSpans {
                token: &|range, source| {
                    self.tt_span_for_range(builder, range, macro_edition, source)
                },
                call_site: None,
            }),
        }
        .lower(&item, children, &mut recorder)
        .expect("file lowering retains every item kind");
        builder.sources.finish_owner(recorder);
        let id = builder.items.alloc(node);

        if let (Some(expansion), Some(range)) = (self.expansion, body_range) {
            expansion
                .body_items
                .borrow_mut()
                .push((id, Span::from_text_range(range)));
        }
        match item {
            // Keep an exact-text dependency because the node omits the assembly payload.
            ast::Item::AsmExpr(_) => builder.source_sensitive = true,
            ast::Item::MacroCall(call) => {
                // The call's input table is complete before cfg_select arms refer back to it.
                let builtin = self.lower_builtin_macro(builder, id, &call);
                if let ItemKind::MacroCall(call) = &mut builder.items[id].kind {
                    call.builtin = builtin;
                }
            }
            _ => {}
        }
        id
    }

    /// Retains enough syntax for DefMap to interpret source-like builtins after name resolution.
    fn lower_builtin_macro(
        &self,
        builder: &mut FileTreeBuilder,
        id: ItemTreeId,
        item: &ast::MacroCall,
    ) -> Option<BuiltinMacroItem> {
        if let Some(path) = IncludePathExpression::from_macro_call(item, self.edition) {
            return Some(BuiltinMacroItem::Include { path });
        }

        if item.path()?.segment()?.name_ref()?.text() != "cfg_select" {
            return None;
        }
        let ItemKind::MacroCall(call) = &builder.items[id].kind else {
            return None;
        };
        let input = call.input.as_ref()?;
        let select = CfgSelect::parse(input.args.as_ref()?)?;
        let call_site = input.call_site;
        let arms = select
            .arms()
            .iter()
            .map(|arm| {
                // A target that doesn't select this arm can still use the other arms.
                let Some((file, span_map)) = arm.parse_items() else {
                    return CfgSelectArmItem::lowering_failed(arm.predicate.clone());
                };
                let positions = span_map
                    .entries()
                    .map(|(end, token)| builder.sources.token_span(token).map(|span| (end, span)))
                    .collect::<Option<Vec<_>>>();
                let Some(positions) = positions else {
                    return CfgSelectArmItem::lowering_failed(arm.predicate.clone());
                };
                let invocation = *builder
                    .sources
                    .get(builder.items[id].location)
                    .expect("call sources were finished before its arms");
                let expansion = FileExpansionSource {
                    tokens: span_map,
                    positions,
                    invocation,
                    body_items: RefCell::default(),
                };
                let lowering = FileLowering {
                    edition: self.edition,
                    expansion: Some(&expansion),
                };
                let items = lowering.collect_items(builder, file.items());
                // Keep this arm's tokens with the ranges recorded in its parse. A nested
                // `cfg_select!` saves its own arm separately because it has a different parse.
                let body_items = expansion.body_items.into_inner();
                if !body_items.is_empty() {
                    builder.cfg_select_bodies.push(ExpandedItemBodies {
                        tokens: Arc::new(arm.payload.clone()),
                        call_site,
                        dollar_crate: None,
                        items: body_items,
                    });
                }
                CfgSelectArmItem::lowered(arm.predicate.clone(), items)
            })
            .collect();
        Some(BuiltinMacroItem::CfgSelect { arms })
    }
}

/// A cfg_select arm is parsed from tokens, but every retained occurrence belongs to the
/// original file. Keep that interpretation immutable for the whole arm, including its children.
/// `tokens` preserves input-token identities for nested macro calls; `positions` maps the parsed
/// arm's byte offsets to physical file spans for declaration lowering.
struct FileExpansionSource {
    tokens: ExpansionSpanMap,
    positions: Vec<(TextSize, Span)>,
    invocation: Span,
    body_items: RefCell<Vec<(ItemTreeId, Span)>>,
}

impl FileExpansionSource {
    fn span(&self, range: TextRange) -> Span {
        let first = self
            .positions
            .partition_point(|(end, _)| *end <= range.start());
        let last = self
            .positions
            .partition_point(|(end, _)| *end < range.end());
        match (self.positions.get(first), self.positions.get(last)) {
            (Some((_, first)), Some((_, last))) => first.cover(*last),
            _ => self.invocation,
        }
    }
}

/// File-local item arena under construction.
struct FileTreeBuilder {
    package: u32,
    current_file_id: FileId,
    items: Arena<ItemTreeId, ItemNode>,
    sources: DeclarationSourceStore<Span>,
    source_sensitive: bool,
    cfg_select_bodies: Vec<ExpandedItemBodies>,
}

impl FileTreeBuilder {
    fn new(package: u32, current_file_id: FileId) -> Self {
        Self {
            package,
            current_file_id,
            items: Arena::new(),
            sources: DeclarationSourceStore::default(),
            source_sensitive: false,
            cfg_select_bodies: Vec::new(),
        }
    }
}
