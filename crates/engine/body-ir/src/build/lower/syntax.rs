//! Syntax helpers shared by the function-body lowering modules.

use rg_syntax::{
    AstNode as _,
    ast::{self, HasGenericArgs as _, PathSegmentKind},
};

use rg_ir_model::{
    BodyPath, BodyPathSegment, BodyPathSegmentArgs, BodyPathSegmentKind,
    items::{GenericArg, PrimitiveTy, TypePath, TypeRef, UnsignedIntTy},
};
use rg_item_tree::FromAst as _;
use rg_parse::{FileId, Span};
use rg_text::Name;

use crate::ir::{BodySource, LabelData, LiteralKind};

use super::body::BodyLowering;

impl BodyLowering<'_> {
    pub(super) fn literal_kind_from_ast(literal: &ast::Literal) -> LiteralKind {
        match literal.kind() {
            ast::LiteralKind::Bool(_) => LiteralKind::Bool,
            ast::LiteralKind::Char(_) => LiteralKind::Char,
            ast::LiteralKind::Byte(_) => LiteralKind::Int {
                primitive_ty: Some(PrimitiveTy::UnsignedInt(UnsignedIntTy::U8)),
            },
            ast::LiteralKind::FloatNumber(number) => LiteralKind::Float {
                primitive_ty: PrimitiveTy::from_float_suffix(number.suffix()),
            },
            ast::LiteralKind::IntNumber(number) => LiteralKind::Int {
                primitive_ty: PrimitiveTy::from_integer_suffix(number.suffix()),
            },
            ast::LiteralKind::String(_)
            | ast::LiteralKind::ByteString(_)
            | ast::LiteralKind::CString(_) => LiteralKind::String,
        }
    }
}

impl BodyLowering<'_> {
    pub(super) fn lower_body_path(&mut self, path: ast::Path) -> Option<BodyPath> {
        let source_span = Span::from_text_range(path.syntax().text_range());
        let absolute = path
            .first_segment()
            .is_some_and(|segment| segment.coloncolon_token().is_some());
        let mut segments = Vec::new();
        self.collect_path_segments(&path, &mut segments)?;

        if segments.is_empty() {
            return None;
        }

        Some(BodyPath::new(source_span, absolute, segments))
    }

    pub(super) fn intern_ast_name(&mut self, name: ast::Name) -> Name {
        self.intern_name_text(name.text())
    }

    pub(super) fn intern_ast_name_ref(&mut self, name_ref: ast::NameRef) -> Name {
        self.intern_name_text(name_ref.text())
    }

    pub(super) fn intern_ast_name_or_name_ref(&mut self, name: ast::NameOrNameRef) -> Name {
        self.intern_name_text(name.text())
    }

    pub(super) fn intern_ast_lifetime(&mut self, lifetime: ast::Lifetime) -> Name {
        self.intern_name_text(lifetime.text())
    }

    pub(super) fn lower_label(&mut self, label: Option<ast::Label>) -> Option<LabelData> {
        self.lower_lifetime_label(label.and_then(|label| label.lifetime()))
    }

    pub(super) fn lower_lifetime_label(
        &mut self,
        lifetime: Option<ast::Lifetime>,
    ) -> Option<LabelData> {
        let lifetime = lifetime?;
        Some(LabelData {
            name: self.intern_ast_lifetime(lifetime.clone()),
            span: self.source(lifetime.syntax()).span,
        })
    }

    fn collect_path_segments(
        &mut self,
        path: &ast::Path,
        segments: &mut Vec<BodyPathSegment>,
    ) -> Option<()> {
        if let Some(qualifier) = path.qualifier() {
            self.collect_path_segments(&qualifier, segments)?;
        }

        if let Some(segment) = path.segment() {
            let segment = self.lower_path_segment(segment)?;
            segments.push(segment);
        }

        Some(())
    }

    fn lower_path_segment(&mut self, segment: ast::PathSegment) -> Option<BodyPathSegment> {
        let (kind, span) = match segment.kind()? {
            PathSegmentKind::Name(name_ref) => {
                let span = self.source(name_ref.syntax()).span;
                (
                    BodyPathSegmentKind::Name(self.intern_ast_name_ref(name_ref)),
                    span,
                )
            }
            // `Self` needs to stay distinguishable in the rich path, but its DefMap projection
            // follows DefMap's type-path convention and behaves like a normal type name.
            PathSegmentKind::SelfTypeKw => {
                let token = segment.self_type_token()?;
                (
                    BodyPathSegmentKind::SelfType,
                    Span::from_text_range(token.text_range()),
                )
            }
            PathSegmentKind::SelfKw => {
                let token = segment.self_token()?;
                (
                    BodyPathSegmentKind::SelfKw,
                    Span::from_text_range(token.text_range()),
                )
            }
            PathSegmentKind::SuperKw => {
                let token = segment.super_token()?;
                (
                    BodyPathSegmentKind::SuperKw,
                    Span::from_text_range(token.text_range()),
                )
            }
            PathSegmentKind::CrateKw => {
                let token = segment.crate_token()?;
                (
                    BodyPathSegmentKind::CrateKw,
                    Span::from_text_range(token.text_range()),
                )
            }
            PathSegmentKind::Type {
                type_ref,
                trait_ref,
            } => {
                let ty = type_ref
                    .map(|ty| TypeRef::from_ast(&ty, (self.line_index, &mut *self.interner)));
                let trait_ref = trait_ref
                    .and_then(|trait_ref| trait_ref.path())
                    .map(|path| {
                        TypeRef::Path(TypePath::from_ast(
                            &path,
                            (self.line_index, &mut *self.interner),
                        ))
                    });
                let span = segment
                    .type_anchor()
                    .map(|anchor| self.source(anchor.syntax()).span)
                    .unwrap_or_else(|| self.source(segment.syntax()).span);

                (BodyPathSegmentKind::TypeAnchor { ty, trait_ref }, span)
            }
        };

        let args = self.lower_path_segment_args(&segment);
        Some(BodyPathSegment::new(kind, span, args))
    }

    fn lower_path_segment_args(
        &mut self,
        segment: &ast::PathSegment,
    ) -> Option<BodyPathSegmentArgs> {
        if let Some(args) = segment.generic_arg_list() {
            return Some(BodyPathSegmentArgs::Angle {
                colon_colon: args.coloncolon_token().is_some(),
                args: args
                    .generic_args()
                    .map(|arg| GenericArg::from_ast(&arg, (self.line_index, &mut *self.interner)))
                    .collect(),
            });
        }

        let parenthesized_args = segment.parenthesized_arg_list()?;
        let mut text = parenthesized_args.syntax().text().to_string();
        if let Some(ret_ty) = segment.ret_type() {
            text.push_str(&ret_ty.syntax().text().to_string());
        }

        Some(BodyPathSegmentArgs::Parenthesized(text))
    }

    fn intern_name_text(&mut self, text: impl AsRef<str>) -> Name {
        let text = text.as_ref();
        self.interner
            .intern(text.strip_prefix("r#").unwrap_or(text))
    }
}

pub(super) fn source_for(file_id: FileId, syntax: &rg_syntax::SyntaxNode) -> BodySource {
    BodySource {
        file_id,
        span: Span::from_text_range(syntax.text_range()),
    }
}

#[cfg(test)]
mod tests {
    use rg_ir_model::items::{FloatTy, PrimitiveTy, SignedIntTy, UnsignedIntTy};
    use rg_syntax::{AstNode as _, Edition, SourceFile, ast};

    use crate::ir::LiteralKind;

    use super::BodyLowering;

    #[test]
    fn classifies_rust_literal_tokens() {
        let cases = [
            ("true", LiteralKind::Bool, "bool literal"),
            ("'x'", LiteralKind::Char, "char literal"),
            (
                "b'x'",
                LiteralKind::Int {
                    primitive_ty: Some(PrimitiveTy::UnsignedInt(UnsignedIntTy::U8)),
                },
                "byte literal",
            ),
            (
                "42",
                LiteralKind::Int {
                    primitive_ty: Some(PrimitiveTy::SignedInt(SignedIntTy::I32)),
                },
                "integer literal",
            ),
            (
                "42usize",
                LiteralKind::Int {
                    primitive_ty: Some(PrimitiveTy::UnsignedInt(UnsignedIntTy::Usize)),
                },
                "suffixed integer literal",
            ),
            (
                "1.5",
                LiteralKind::Float {
                    primitive_ty: Some(PrimitiveTy::Float(FloatTy::F64)),
                },
                "decimal float literal",
            ),
            (
                "1e10",
                LiteralKind::Float {
                    primitive_ty: Some(PrimitiveTy::Float(FloatTy::F64)),
                },
                "exponent float literal",
            ),
            (
                "1E-10",
                LiteralKind::Float {
                    primitive_ty: Some(PrimitiveTy::Float(FloatTy::F64)),
                },
                "negative exponent float literal",
            ),
            (
                "1E-10f32",
                LiteralKind::Float {
                    primitive_ty: Some(PrimitiveTy::Float(FloatTy::F32)),
                },
                "suffixed float literal",
            ),
            (r#""text""#, LiteralKind::String, "string literal"),
            (r##"r#"text"#"##, LiteralKind::String, "raw string literal"),
            (r#"b"text""#, LiteralKind::String, "byte string literal"),
            (
                r##"br#"text"#"##,
                LiteralKind::String,
                "raw byte string literal",
            ),
        ];

        for (expr, expected, label) in cases {
            let source = format!("fn main() {{ let _ = {expr}; }}");
            let file = SourceFile::parse(&source, Edition::CURRENT)
                .ok()
                .expect("literal fixture should parse");
            let literal = file
                .syntax()
                .descendants()
                .find_map(ast::Literal::cast)
                .expect("fixture should contain a literal expression");

            assert_eq!(
                BodyLowering::literal_kind_from_ast(&literal),
                expected,
                "{label}"
            );
        }
    }
}
