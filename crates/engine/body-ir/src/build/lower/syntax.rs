//! Syntax helpers shared by the function-body lowering modules.

use rg_ir_model::{FileId, PrimitiveTy, Span, UnsignedIntTy};
use rg_item_tree::{FromAst as _, GenericArg, TypePath, TypeRef};
use rg_syntax::{
    AstNode as _, SyntaxNode,
    ast::{self, HasGenericArgs as _, PathSegmentKind},
};
use rg_text::Name;

use super::body::BodyLowering;
use crate::body::{
    BodyPath, BodyPathSegment, BodyPathSegmentArgs, BodyPathSegmentKind, BodySource, LabelData,
    LiteralKind,
};

impl BodyLowering<'_> {
    pub(super) fn literal_kind_from_ast(literal: &ast::Literal) -> LiteralKind {
        match literal.kind() {
            ast::LiteralKind::Bool(_) => LiteralKind::Bool,
            ast::LiteralKind::Char(_) => LiteralKind::Char,
            ast::LiteralKind::Byte(_) => LiteralKind::Int {
                primitive_ty: Some(PrimitiveTy::UnsignedInt(UnsignedIntTy::U8)),
            },
            ast::LiteralKind::FloatNumber(number) => LiteralKind::Float {
                primitive_ty: number
                    .suffix()
                    .and_then(|suffix| PrimitiveTy::from_float_suffix(Some(suffix))),
            },
            ast::LiteralKind::IntNumber(number) => LiteralKind::Int {
                primitive_ty: number
                    .suffix()
                    .and_then(|suffix| PrimitiveTy::from_integer_suffix(Some(suffix))),
            },
            ast::LiteralKind::String(_)
            | ast::LiteralKind::ByteString(_)
            | ast::LiteralKind::CString(_) => LiteralKind::String,
        }
    }

    pub(super) fn lower_body_path(&mut self, path: ast::Path) -> Option<BodyPath> {
        let location = self.source_location(path.syntax().text_range());
        let absolute = path
            .first_segment()
            .is_some_and(|segment| segment.coloncolon_token().is_some());
        let mut segments = Vec::new();
        self.collect_path_segments(&path, &mut segments)?;

        if segments.is_empty() {
            return None;
        }

        Some(BodyPath::new(location, absolute, segments))
    }

    pub(super) fn lower_label(&mut self, label: Option<ast::Label>) -> Option<LabelData> {
        self.lower_lifetime_label(label.and_then(|label| label.lifetime()))
    }

    pub(super) fn lower_type_ref(&mut self, ty: ast::Type) -> TypeRef {
        // Body lowering only expands macro calls that occupy the whole type position. Nested type
        // syntax stays delegated to item-tree's TypeRef lowering so we do not duplicate that
        // recursive grammar here.
        if let ast::Type::MacroType(macro_ty) = ty.clone()
            && let Some(expanded) = self.expand_macro_type(&macro_ty)
        {
            return expanded;
        }

        self.with_sources(|sources| TypeRef::from_ast(&ty, sources))
    }

    /// Turbofish arguments need the same source mapping as body type annotations.
    pub(super) fn lower_generic_arg(&mut self, arg: &ast::GenericArg) -> GenericArg {
        self.with_sources(|sources| GenericArg::from_ast(arg, sources))
    }

    fn expand_macro_type(&mut self, ty: &ast::MacroType) -> Option<TypeRef> {
        let call_source = self.source(ty.syntax());
        let call = ty.macro_call()?;
        let _expansion_scope = self.macro_expansion.expansion_scope()?;
        let module = self.macro_resolution_module();
        let origin = self.macro_call_origin();

        // Type expansion is best-effort like expression and pattern expansion. A failed expansion
        // keeps the original `MacroType` as an unknown TypeRef so body lowering remains buildable.
        let outcome = self
            .macro_expansion
            .expand_type_call(module, call_source, origin.clone(), &call)
            .ok()
            .flatten()?;
        self.record_source_macro_call(call_source, &call, origin, outcome.definition);
        let expanded = outcome.expansion?;

        Some(
            self.with_expanded_macro(call_source, expanded, |this, syntax| {
                this.lower_type_ref(syntax)
            }),
        )
    }

    pub(super) fn lower_lifetime_label(
        &mut self,
        lifetime: Option<ast::Lifetime>,
    ) -> Option<LabelData> {
        let lifetime = lifetime?;
        Some(LabelData {
            name: Name::new(lifetime.text()),
            location: self.source_location(lifetime.syntax().text_range()),
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
        let (kind, location) = match segment.kind()? {
            PathSegmentKind::Name(name_ref) => {
                let location = self.source_location(name_ref.syntax().text_range());
                let name = Name::new(name_ref.text());
                // `$crate` is meaningful only for macro-generated syntax, where the temporary
                // lowering context knows which crate defined the macro.
                let kind = if name.as_str() == "$crate" {
                    BodyPathSegmentKind::DollarCrate(self.dollar_crate()?)
                } else {
                    BodyPathSegmentKind::Name(name)
                };
                (kind, location)
            }
            // `Self` needs to stay distinguishable in the rich path, but its DefMap projection
            // follows DefMap's type-path convention and behaves like a normal type name.
            PathSegmentKind::SelfTypeKw => {
                let token = segment.self_type_token()?;
                (
                    BodyPathSegmentKind::SelfType,
                    self.source_location(token.text_range()),
                )
            }
            PathSegmentKind::SelfKw => {
                let token = segment.self_token()?;
                (
                    BodyPathSegmentKind::SelfKw,
                    self.source_location(token.text_range()),
                )
            }
            PathSegmentKind::SuperKw => {
                let token = segment.super_token()?;
                (
                    BodyPathSegmentKind::SuperKw,
                    self.source_location(token.text_range()),
                )
            }
            PathSegmentKind::CrateKw => {
                let token = segment.crate_token()?;
                (
                    BodyPathSegmentKind::CrateKw,
                    self.source_location(token.text_range()),
                )
            }
            PathSegmentKind::Type {
                type_ref,
                trait_ref,
            } => {
                let ty = type_ref.map(|ty| self.lower_type_ref(ty));
                let trait_ref = trait_ref
                    .and_then(|trait_ref| trait_ref.path())
                    .map(|path| {
                        self.with_sources(|sources| {
                            TypeRef::Path(TypePath::from_ast(&path, sources))
                        })
                    });
                let location = segment
                    .type_anchor()
                    .map(|anchor| self.source_location(anchor.syntax().text_range()))
                    .unwrap_or_else(|| self.source_location(segment.syntax().text_range()));

                (BodyPathSegmentKind::TypeAnchor { ty, trait_ref }, location)
            }
        };

        let args = self.lower_path_segment_args(&segment);
        Some(BodyPathSegment::new(kind, location, args))
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
                    .map(|arg| self.lower_generic_arg(&arg))
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
}

pub(super) fn source_for(file_id: FileId, syntax: &SyntaxNode) -> BodySource {
    BodySource::written(file_id, Span::from_text_range(syntax.text_range()))
}

#[cfg(test)]
mod tests {
    use rg_ir_model::{FloatTy, PrimitiveTy, UnsignedIntTy};
    use rg_syntax::{AstNode as _, Edition, SourceFile, ast};

    use super::BodyLowering;
    use crate::body::LiteralKind;

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
                LiteralKind::Int { primitive_ty: None },
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
                LiteralKind::Float { primitive_ty: None },
                "decimal float literal",
            ),
            (
                "1e10",
                LiteralKind::Float { primitive_ty: None },
                "exponent float literal",
            ),
            (
                "1E-10",
                LiteralKind::Float { primitive_ty: None },
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
