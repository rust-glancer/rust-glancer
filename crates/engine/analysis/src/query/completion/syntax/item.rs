//! Item, signature-type, and statement-boundary classification.

use rg_ir_model::Path;
use rg_parse::{Span, TextSpan};
use rg_syntax::{
    AstNode as _, SyntaxKind, SyntaxNode, SyntaxToken,
    ast::{self, HasAttrs as _, HasName as _},
};

use crate::query::completion::site::{
    BodyMacroCompletionContext, ItemListCompletionContext, ItemListCompletionKind,
    ItemQualifierContext, ModuleDeclarationCompletionContext, ModuleMacroCompletionContext,
    SyntaxCompletionContext, TraitImplCompletionSyntax, TraitImplMemberKind, TypeCompletionContext,
};

use super::CompletionSyntaxContext;

impl CompletionSyntaxContext<'_> {
    /// Recognize the name slot of an out-of-line `mod name;` declaration.
    pub(super) fn module_declaration_context(&self) -> Option<ModuleDeclarationCompletionContext> {
        if let Some(module) = Self::module_at_marker(&self.marker) {
            // Editing an inline module name must not turn it into a filesystem-module site merely
            // because an added speculative semicolon would form a second valid parse.
            if module.item_list().is_some() {
                return None;
            }
            if module.semicolon_token().is_some() {
                return Self::module_declaration_context_for(module);
            }
        }

        let marker = self.marker_with_suffix(";")?;
        Self::module_declaration_context_for(Self::module_at_marker(&marker)?)
    }

    fn module_at_marker(marker: &SyntaxToken) -> Option<ast::Module> {
        marker
            .parent()?
            .ancestors()
            .find_map(ast::Module::cast)
            .filter(|module| {
                module
                    .name()
                    .is_some_and(|name| name.text() == Self::MARKER)
            })
    }

    fn module_declaration_context_for(
        module: ast::Module,
    ) -> Option<ModuleDeclarationCompletionContext> {
        if module.semicolon_token().is_none() || module.item_list().is_some() {
            return None;
        }

        let has_path_attribute = module
            .attrs()
            .filter(|attribute| attribute.kind().is_outer())
            .any(|attribute| attribute.simple_name().as_deref() == Some("path"));
        Some(ModuleDeclarationCompletionContext::new(has_path_attribute))
    }

    /// Recognize a macro callee only when the call itself is an item or associated item.
    pub(super) fn module_macro_context(&self) -> Option<ModuleMacroCompletionContext> {
        if let Some(context) = self.module_macro_context_at(&self.marker, false) {
            return Some(context);
        }
        if self.prefix().is_empty() {
            return None;
        }

        // Before `!` is typed there is no macro-call node for the ordinary parser to expose. Add
        // only the punctuation needed to ask whether this identifier/path would be an item macro.
        let marker = self.marker_with_suffix("!();")?;
        self.module_macro_context_at(&marker, true)
    }

    /// Recognize the callee path of a macro invocation used inside a body.
    pub(super) fn body_macro_context(&self) -> Option<BodyMacroCompletionContext> {
        let call = self
            .marker
            .parent()?
            .ancestors()
            .find_map(ast::MacroCall::cast)?;
        if !call
            .path()?
            .syntax()
            .text_range()
            .contains_range(self.marker.text_range())
        {
            return None;
        }

        let mut owner = call.syntax().parent();
        while let Some(node) = owner {
            if ast::StmtList::can_cast(node.kind()) || ast::BlockExpr::can_cast(node.kind()) {
                return Some(BodyMacroCompletionContext::new(Self::macro_qualifier(
                    &call,
                )?));
            }
            if ast::SourceFile::can_cast(node.kind())
                || ast::ItemList::can_cast(node.kind())
                || ast::AssocItemList::can_cast(node.kind())
            {
                return None;
            }
            owner = node.parent();
        }
        None
    }

    fn module_macro_context_at(
        &self,
        marker: &SyntaxToken,
        incomplete: bool,
    ) -> Option<ModuleMacroCompletionContext> {
        let call = marker
            .parent()?
            .ancestors()
            .find_map(ast::MacroCall::cast)?;
        let mut owner = call.syntax().parent();
        let item_list_kind = loop {
            let node = owner?;
            if ast::SourceFile::can_cast(node.kind()) {
                break ItemListCompletionKind::SourceFile;
            }
            if ast::ItemList::can_cast(node.kind()) {
                break ItemListCompletionKind::Module;
            }
            if ast::AssocItemList::can_cast(node.kind()) {
                let parent = node.parent()?;
                if ast::Trait::can_cast(parent.kind()) {
                    break ItemListCompletionKind::Trait;
                }
                let impl_ = ast::Impl::cast(parent)?;
                break if impl_.trait_().is_some() {
                    ItemListCompletionKind::TraitImpl
                } else {
                    ItemListCompletionKind::InherentImpl
                };
            }

            // Only parser recovery nodes may sit between an item macro and its owning list. A
            // function, type, expression, or other real AST node means the speculative `!()` was
            // inserted inside that construct rather than forming a module-scope macro item.
            if node.kind() != SyntaxKind::ERROR {
                return None;
            }
            owner = node.parent();
        };

        let qualifier = Self::macro_qualifier(&call)?;
        if incomplete {
            // A bare prefix in a trait impl first belongs to missing-member completion. Once `!`
            // is present the ordinary macro-call parse above takes ownership instead.
            if item_list_kind == ItemListCompletionKind::TraitImpl {
                return None;
            }
            return Some(ModuleMacroCompletionContext::incomplete(
                qualifier,
                ItemListCompletionContext::new(item_list_kind, self.item_qualifiers()),
            ));
        }
        Some(ModuleMacroCompletionContext::new(qualifier))
    }

    /// Return `Some(None)` for an unqualified callee and `Some(Some(path))` for a qualified one.
    fn macro_qualifier(call: &ast::MacroCall) -> Option<Option<Path>> {
        let path = call.path()?;
        let path = Path::from_macro_path_text(&path.syntax().text().to_string(), None)?;
        if path.single_name() == Some(Self::MARKER) {
            return Some(None);
        }
        let (qualifier, name) = path.split_prefix_name()?;
        (name == Self::MARKER).then_some(Some(qualifier))
    }

    /// Decide whether this type slot permits `impl Trait`.
    ///
    /// Function parameter and return slots permit it; nested type syntax and other declaration
    /// owners use the general type vocabulary.
    pub(super) fn type_completion_context(type_node: &SyntaxNode) -> TypeCompletionContext {
        let mut declaration_slot = false;
        for ancestor in type_node.ancestors().skip(1) {
            if ast::Param::can_cast(ancestor.kind()) || ast::RetType::can_cast(ancestor.kind()) {
                declaration_slot = true;
            }
            if declaration_slot && ast::Fn::can_cast(ancestor.kind()) {
                return TypeCompletionContext::ImplTraitAllowed;
            }
            if ast::BlockExpr::can_cast(ancestor.kind())
                || ast::ItemList::can_cast(ancestor.kind())
                || ast::AssocItemList::can_cast(ancestor.kind())
                || ast::SourceFile::can_cast(ancestor.kind())
            {
                break;
            }
        }

        TypeCompletionContext::General
    }

    /// Attach already-written qualifiers to one item-list owner.
    pub(super) fn item_list_context(
        &self,
        kind: ItemListCompletionKind,
    ) -> SyntaxCompletionContext {
        SyntaxCompletionContext::ItemList(ItemListCompletionContext::new(
            kind,
            self.item_qualifiers(),
        ))
    }

    /// Recover the semantic owner and edit policy for a missing-member prefix.
    ///
    /// Parser recovery already creates an associated item for `fn re$0`, `type Ou$0`, and
    /// `const LI$0`. Retaining that item's introducer lets candidate lookup select one declaration
    /// family and makes the generated scaffold replace the introducer together with the partial
    /// name. A bare `re$0` remains deliberately permissive and replaces only its identifier.
    pub(crate) fn trait_impl_completion_syntax(&self) -> Option<TraitImplCompletionSyntax> {
        let owner_start = self.marker.parent()?.ancestors().find_map(|node| {
            let impl_ = ast::Impl::cast(node)?;
            impl_
                .trait_()
                .is_some()
                .then(|| u32::from(impl_.syntax().text_range().start()))
        })?;

        let prefix_span = self.prefix.span();
        let mut member_kind = None;
        let mut replace_start = prefix_span.text.start;

        // Use the recovered associated item's own tokens rather than scanning arbitrary previous
        // source words. This prevents a keyword from an earlier complete item from becoming part
        // of the replacement span.
        for node in self.marker.parent()?.ancestors() {
            if let Some(function) = ast::Fn::cast(node.clone()) {
                let mut start = u32::from(function.fn_token()?.text_range().start());
                for token in [
                    function.async_token(),
                    function.const_token(),
                    function.default_token(),
                    function.gen_token(),
                    function.safe_token(),
                    function.unsafe_token(),
                ]
                .into_iter()
                .flatten()
                {
                    start = start.min(u32::from(token.text_range().start()));
                }
                if let Some(abi) = function.abi() {
                    start = start.min(u32::from(abi.syntax().text_range().start()));
                }
                member_kind = Some(TraitImplMemberKind::Function);
                replace_start = start;
                break;
            }
            if let Some(alias) = ast::TypeAlias::cast(node.clone()) {
                let mut start = u32::from(alias.type_token()?.text_range().start());
                if let Some(token) = alias.default_token() {
                    start = start.min(u32::from(token.text_range().start()));
                }
                member_kind = Some(TraitImplMemberKind::TypeAlias);
                replace_start = start;
                break;
            }
            if let Some(konst) = ast::Const::cast(node.clone()) {
                let mut start = u32::from(konst.const_token()?.text_range().start());
                if let Some(token) = konst.default_token() {
                    start = start.min(u32::from(token.text_range().start()));
                }
                member_kind = Some(TraitImplMemberKind::Const);
                replace_start = start;
                break;
            }
            if ast::AssocItemList::can_cast(node.kind()) {
                break;
            }
        }

        Some(TraitImplCompletionSyntax::new(
            owner_start,
            member_kind,
            Span {
                text: TextSpan {
                    start: replace_start,
                    end: prefix_span.text.end,
                },
            },
            match member_kind {
                Some(_) => Some(
                    self.source_text(Span {
                        text: TextSpan {
                            start: replace_start,
                            end: prefix_span.text.start,
                        },
                    })?
                    .to_string(),
                ),
                None => None,
            },
        ))
    }

    /// Read only the current item's leading tokens so recovered AST wrappers do not matter.
    fn item_qualifiers(&self) -> ItemQualifierContext {
        let mut qualifiers = ItemQualifierContext::default();
        let mut token = self.marker.prev_token();
        while let Some(previous) = token {
            token = previous.prev_token();
            if previous.kind().is_trivia() {
                continue;
            }

            match previous.kind() {
                SyntaxKind::PUB_KW => qualifiers.has_visibility = true,
                SyntaxKind::UNSAFE_KW => qualifiers.has_unsafe = true,
                SyntaxKind::ASYNC_KW => qualifiers.has_async = true,
                SyntaxKind::EXTERN_KW => qualifiers.has_extern = true,
                SyntaxKind::CONST_KW => qualifiers.has_const = true,

                // These tokens can belong to `pub(in path)`, `pub(crate)`, or an extern ABI.
                SyntaxKind::L_PAREN
                | SyntaxKind::R_PAREN
                | SyntaxKind::IN_KW
                | SyntaxKind::CRATE_KW
                | SyntaxKind::SELF_KW
                | SyntaxKind::SUPER_KW
                | SyntaxKind::IDENT
                | SyntaxKind::COLON2
                | SyntaxKind::STRING => {}

                // An item boundary or attribute prevents qualifiers from an earlier item from
                // leaking into this one.
                SyntaxKind::SEMICOLON
                | SyntaxKind::L_CURLY
                | SyntaxKind::R_CURLY
                | SyntaxKind::R_BRACK => break,
                _ => break,
            }
        }
        qualifiers
    }

    /// Tell a new statement position from an identifier being typed inside an expression.
    pub(super) fn at_statement_boundary(&self) -> bool {
        self.previous_non_trivia_token().is_none_or(|token| {
            matches!(
                token.kind(),
                SyntaxKind::L_CURLY | SyntaxKind::R_CURLY | SyntaxKind::SEMICOLON
            )
        })
    }
}
