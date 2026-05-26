//! Source symbol classification over cursor candidates from all indexing layers.

use rg_body_ir::BodyCursorCandidate;
use rg_def_map::DefMapCursorCandidate;
use rg_ir_model::{BodyBindingRef, TargetRef};
use rg_parse::{FileId, Span};
use rg_semantic_ir::SemanticCursorCandidate;

use crate::{
    api::{Analysis, view::declaration::DeclarationView},
    model::{
        DeclarationRef, ExprRef, FunctionBodyRef, LexicalScopeRef, SymbolAt, TypePathScopeRef,
    },
};

/// Why a source symbol exists in the scanned source surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SourceSymbolRole {
    Declaration,
    Reference,
    Structural,
}

/// One source span that can resolve to an analysis symbol.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SourceSymbol {
    symbol: SymbolAt,
    target: TargetRef,
    file_id: FileId,
    span: Span,
    role: SourceSymbolRole,
}

impl SourceSymbol {
    pub(crate) fn symbol(&self) -> &SymbolAt {
        &self.symbol
    }

    pub(crate) fn into_symbol(self) -> SymbolAt {
        self.symbol
    }

    pub(crate) fn target(&self) -> TargetRef {
        self.target
    }

    pub(crate) fn file_id(&self) -> FileId {
        self.file_id
    }

    pub(crate) fn span(&self) -> Span {
        self.span
    }

    pub(crate) fn role(&self) -> SourceSymbolRole {
        self.role
    }

    fn declaration(symbol: SymbolAt, target: TargetRef, file_id: FileId, span: Span) -> Self {
        Self {
            symbol,
            target,
            file_id,
            span,
            role: SourceSymbolRole::Declaration,
        }
    }

    fn reference(symbol: SymbolAt, target: TargetRef, file_id: FileId, span: Span) -> Self {
        Self {
            symbol,
            target,
            file_id,
            span,
            role: SourceSymbolRole::Reference,
        }
    }

    fn structural(symbol: SymbolAt, target: TargetRef, file_id: FileId, span: Span) -> Self {
        Self {
            symbol,
            target,
            file_id,
            span,
            role: SourceSymbolRole::Structural,
        }
    }
}

pub(crate) struct SourceSymbolIndex<'a, 'db> {
    analysis: &'a Analysis<'db>,
}

impl<'a, 'db> SourceSymbolIndex<'a, 'db> {
    pub(crate) fn new(analysis: &'a Analysis<'db>) -> Self {
        Self { analysis }
    }

    pub(crate) fn symbols_at(
        &self,
        target: TargetRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Vec<SourceSymbol>> {
        let mut symbols = Vec::new();

        for candidate in self
            .analysis
            .body_ir
            .cursor_candidates(target, file_id, offset)?
        {
            if let Some(symbol) = self.body_symbol(target, candidate, Some(file_id))? {
                symbols.push(symbol);
            }
        }
        for candidate in self
            .analysis
            .def_map
            .cursor_candidates(target, file_id, offset)?
        {
            symbols.push(Self::def_map_symbol(target, candidate));
        }
        for candidate in self
            .analysis
            .semantic_ir
            .signature_cursor_candidates(target, file_id, offset)?
        {
            if let Some(symbol) = self.semantic_symbol(target, candidate, Some(file_id))? {
                symbols.push(symbol);
            }
        }

        Ok(symbols)
    }

    pub(crate) fn symbols_in_target(
        &self,
        target: TargetRef,
        file_id: Option<FileId>,
    ) -> anyhow::Result<Vec<SourceSymbol>> {
        let mut symbols = Vec::new();

        for candidate in self.analysis.def_map.source_candidates(target, file_id)? {
            symbols.push(Self::def_map_symbol(target, candidate));
        }
        for candidate in self.analysis.body_ir.source_candidates(target, file_id)? {
            if let Some(symbol) = self.body_symbol(target, candidate, file_id)? {
                symbols.push(symbol);
            }
        }
        for candidate in self
            .analysis
            .semantic_ir
            .signature_source_candidates(target, file_id)?
        {
            if let Some(symbol) = self.semantic_symbol(target, candidate, file_id)? {
                symbols.push(symbol);
            }
        }

        Ok(symbols)
    }

    fn def_map_symbol(target: TargetRef, candidate: DefMapCursorCandidate) -> SourceSymbol {
        match candidate {
            DefMapCursorCandidate::Def { def, file_id, span } => {
                let declaration = DeclarationRef::from_def(def);
                SourceSymbol::declaration(
                    SymbolAt::Declaration { declaration, span },
                    target,
                    file_id,
                    span,
                )
            }
            DefMapCursorCandidate::UsePath {
                module,
                path,
                file_id,
                span,
            } => SourceSymbol::reference(
                SymbolAt::UsePath { module, path, span },
                target,
                file_id,
                span,
            ),
        }
    }

    fn semantic_symbol(
        &self,
        target: TargetRef,
        candidate: SemanticCursorCandidate,
        fallback_file_id: Option<FileId>,
    ) -> anyhow::Result<Option<SourceSymbol>> {
        let symbol = match candidate {
            SemanticCursorCandidate::Field { field, span } => {
                let declaration = DeclarationRef::semantic(field.into());
                self.declaration_symbol(
                    SymbolAt::Declaration { declaration, span },
                    declaration,
                    target,
                    span,
                    fallback_file_id,
                )?
            }
            SemanticCursorCandidate::Function { function, span } => {
                let declaration = DeclarationRef::semantic(function.into());
                self.declaration_symbol(
                    SymbolAt::Declaration { declaration, span },
                    declaration,
                    target,
                    span,
                    fallback_file_id,
                )?
            }
            SemanticCursorCandidate::EnumVariant { variant, span } => {
                let declaration = DeclarationRef::semantic(variant.into());
                self.declaration_symbol(
                    SymbolAt::Declaration { declaration, span },
                    declaration,
                    target,
                    span,
                    fallback_file_id,
                )?
            }
            SemanticCursorCandidate::TypePath {
                context,
                path,
                file_id,
                span,
            } => Some(SourceSymbol::reference(
                SymbolAt::TypePath {
                    scope: TypePathScopeRef::signature(context),
                    path,
                    span,
                },
                target,
                file_id,
                span,
            )),
        };

        Ok(symbol)
    }

    fn body_symbol(
        &self,
        target: TargetRef,
        candidate: BodyCursorCandidate,
        fallback_file_id: Option<FileId>,
    ) -> anyhow::Result<Option<SourceSymbol>> {
        let span = candidate.span();
        let symbol = match candidate {
            BodyCursorCandidate::Body { body, .. } => {
                let file_id = match self.analysis.body_ir.body_data(body)? {
                    Some(data) => data.source().file_id,
                    None => {
                        let Some(file_id) = fallback_file_id else {
                            return Ok(None);
                        };
                        file_id
                    }
                };
                Some(SourceSymbol::structural(
                    SymbolAt::FunctionBody {
                        body: FunctionBodyRef::from_body_ir(body),
                    },
                    target,
                    file_id,
                    span,
                ))
            }
            BodyCursorCandidate::Binding { body, binding, .. } => {
                let declaration = DeclarationRef::body_binding(BodyBindingRef { body, binding });
                self.declaration_symbol(
                    SymbolAt::Declaration { declaration, span },
                    declaration,
                    target,
                    span,
                    fallback_file_id,
                )?
            }
            BodyCursorCandidate::Expr { body, expr, .. } => {
                let file_id = match self.analysis.body_ir.body_data(body)? {
                    Some(body_data) => match body_data.expr(expr) {
                        Some(data) => data.source.file_id,
                        None => {
                            let Some(file_id) = fallback_file_id else {
                                return Ok(None);
                            };
                            file_id
                        }
                    },
                    None => {
                        let Some(file_id) = fallback_file_id else {
                            return Ok(None);
                        };
                        file_id
                    }
                };
                Some(SourceSymbol::reference(
                    SymbolAt::Expr {
                        expr: ExprRef::new(body, expr),
                    },
                    target,
                    file_id,
                    span,
                ))
            }
            BodyCursorCandidate::LocalItem { item, .. } => {
                let declaration = DeclarationRef::body_item(item);
                self.declaration_symbol(
                    SymbolAt::Declaration { declaration, span },
                    declaration,
                    target,
                    span,
                    fallback_file_id,
                )?
            }
            BodyCursorCandidate::LocalValueItem { item, .. } => {
                let declaration = DeclarationRef::body_value_item(item);
                self.declaration_symbol(
                    SymbolAt::Declaration { declaration, span },
                    declaration,
                    target,
                    span,
                    fallback_file_id,
                )?
            }
            BodyCursorCandidate::LocalField { field, .. } => {
                let declaration = DeclarationRef::body_field(field);
                self.declaration_symbol(
                    SymbolAt::Declaration { declaration, span },
                    declaration,
                    target,
                    span,
                    fallback_file_id,
                )?
            }
            BodyCursorCandidate::LocalEnumVariant { variant, .. } => {
                let declaration = DeclarationRef::body_enum_variant(variant);
                self.declaration_symbol(
                    SymbolAt::Declaration { declaration, span },
                    declaration,
                    target,
                    span,
                    fallback_file_id,
                )?
            }
            BodyCursorCandidate::LocalFunction { function, .. } => {
                let declaration = DeclarationRef::body_function(function);
                self.declaration_symbol(
                    SymbolAt::Declaration { declaration, span },
                    declaration,
                    target,
                    span,
                    fallback_file_id,
                )?
            }
            BodyCursorCandidate::TypePath {
                body,
                scope,
                path,
                file_id,
                ..
            } => Some(SourceSymbol::reference(
                SymbolAt::TypePath {
                    scope: TypePathScopeRef::body(LexicalScopeRef::new(body, scope)),
                    path,
                    span,
                },
                target,
                file_id,
                span,
            )),
            BodyCursorCandidate::ValuePath {
                body,
                scope,
                path,
                file_id,
                ..
            } => Some(SourceSymbol::reference(
                SymbolAt::ValuePath {
                    scope: LexicalScopeRef::new(body, scope),
                    path,
                    span,
                },
                target,
                file_id,
                span,
            )),
        };

        Ok(symbol)
    }

    fn declaration_symbol(
        &self,
        symbol: SymbolAt,
        declaration: DeclarationRef,
        scan_target: TargetRef,
        span: Span,
        fallback_file_id: Option<FileId>,
    ) -> anyhow::Result<Option<SourceSymbol>> {
        // Declaration candidates from signature/body indexes do not all carry a file id.
        // Prefer the declaration view, and fall back to the cursor file for point lookups.
        let file_id = match DeclarationView::new(self.analysis).declaration(declaration)? {
            Some(declaration) => declaration.file_id(),
            None => {
                let Some(file_id) = fallback_file_id else {
                    return Ok(None);
                };
                file_id
            }
        };

        Ok(Some(SourceSymbol::declaration(
            symbol,
            scan_target,
            file_id,
            span,
        )))
    }
}
