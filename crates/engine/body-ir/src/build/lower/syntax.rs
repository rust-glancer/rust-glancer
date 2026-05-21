//! Syntax helpers shared by the function-body lowering modules.

use rg_syntax::{
    AstNode as _,
    ast::{self, HasGenericArgs as _, PathSegmentKind},
};

use rg_item_tree::{GenericArg, TypePath, TypeRef};
use rg_parse::{FileId, Span};
use rg_text::Name;

use crate::ir::{
    BodyPath, BodySource, ExprAssignOp, ExprBinaryOp, ExprRangeKind, ExprUnaryOp, LabelData,
    LiteralKind,
    path::{BodyPathSegment, BodyPathSegmentArgs, BodyPathSegmentKind},
};

use super::function::FunctionBodyLowering;

impl LiteralKind {
    pub(super) fn from_ast(literal: &ast::Literal) -> Self {
        match literal.kind() {
            ast::LiteralKind::Bool(_) => Self::Bool,
            ast::LiteralKind::Char(_) => Self::Char,
            ast::LiteralKind::Byte(_) => Self::Int,
            ast::LiteralKind::FloatNumber(_) => Self::Float,
            ast::LiteralKind::IntNumber(_) => Self::Int,
            ast::LiteralKind::String(_)
            | ast::LiteralKind::ByteString(_)
            | ast::LiteralKind::CString(_) => Self::String,
        }
    }
}

impl ExprUnaryOp {
    pub(super) fn from_ast(op: ast::UnaryOp) -> Self {
        match op {
            ast::UnaryOp::Deref => Self::Deref,
            ast::UnaryOp::Not => Self::Not,
            ast::UnaryOp::Neg => Self::Neg,
        }
    }
}

impl ExprBinaryOp {
    pub(super) fn from_ast(op: ast::BinaryOp) -> Option<Self> {
        Some(match op {
            ast::BinaryOp::LogicOp(ast::LogicOp::Or) => Self::LogicOr,
            ast::BinaryOp::LogicOp(ast::LogicOp::And) => Self::LogicAnd,
            ast::BinaryOp::CmpOp(ast::CmpOp::Eq { negated: false }) => Self::Eq,
            ast::BinaryOp::CmpOp(ast::CmpOp::Eq { negated: true }) => Self::NotEq,
            ast::BinaryOp::CmpOp(ast::CmpOp::Ord {
                ordering: ast::Ordering::Less,
                strict: true,
            }) => Self::Less,
            ast::BinaryOp::CmpOp(ast::CmpOp::Ord {
                ordering: ast::Ordering::Less,
                strict: false,
            }) => Self::LessEq,
            ast::BinaryOp::CmpOp(ast::CmpOp::Ord {
                ordering: ast::Ordering::Greater,
                strict: true,
            }) => Self::Greater,
            ast::BinaryOp::CmpOp(ast::CmpOp::Ord {
                ordering: ast::Ordering::Greater,
                strict: false,
            }) => Self::GreaterEq,
            ast::BinaryOp::ArithOp(op) => Self::from_arith_op(op),
            ast::BinaryOp::Assignment { .. } => return None,
        })
    }

    fn from_arith_op(op: ast::ArithOp) -> Self {
        match op {
            ast::ArithOp::Add => Self::Add,
            ast::ArithOp::Mul => Self::Mul,
            ast::ArithOp::Sub => Self::Sub,
            ast::ArithOp::Div => Self::Div,
            ast::ArithOp::Rem => Self::Rem,
            ast::ArithOp::Shl => Self::Shl,
            ast::ArithOp::Shr => Self::Shr,
            ast::ArithOp::BitXor => Self::BitXor,
            ast::ArithOp::BitOr => Self::BitOr,
            ast::ArithOp::BitAnd => Self::BitAnd,
        }
    }
}

impl ExprAssignOp {
    pub(super) fn from_ast(op: ast::BinaryOp) -> Option<Self> {
        match op {
            ast::BinaryOp::Assignment { op } => Some(match op {
                None => Self::Assign,
                Some(ast::ArithOp::Add) => Self::Add,
                Some(ast::ArithOp::Mul) => Self::Mul,
                Some(ast::ArithOp::Sub) => Self::Sub,
                Some(ast::ArithOp::Div) => Self::Div,
                Some(ast::ArithOp::Rem) => Self::Rem,
                Some(ast::ArithOp::Shl) => Self::Shl,
                Some(ast::ArithOp::Shr) => Self::Shr,
                Some(ast::ArithOp::BitXor) => Self::BitXor,
                Some(ast::ArithOp::BitOr) => Self::BitOr,
                Some(ast::ArithOp::BitAnd) => Self::BitAnd,
            }),
            ast::BinaryOp::LogicOp(_) | ast::BinaryOp::ArithOp(_) | ast::BinaryOp::CmpOp(_) => None,
        }
    }
}

impl ExprRangeKind {
    pub(super) fn from_ast(op: ast::RangeOp) -> Self {
        match op {
            ast::RangeOp::Exclusive => Self::Exclusive,
            ast::RangeOp::Inclusive => Self::Inclusive,
        }
    }
}

impl FunctionBodyLowering<'_> {
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
                let ty = type_ref.map(|ty| TypeRef::from_ast(ty, self.line_index, self.interner));
                let trait_ref = trait_ref
                    .and_then(|trait_ref| trait_ref.path())
                    .map(|path| {
                        TypeRef::Path(TypePath::from_ast(path, self.line_index, self.interner))
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
                    .map(|arg| GenericArg::from_ast(arg, self.line_index, self.interner))
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
    use rg_syntax::{AstNode as _, Edition, SourceFile, ast};

    use crate::ir::LiteralKind;

    #[test]
    fn classifies_rust_literal_tokens() {
        let cases = [
            ("true", LiteralKind::Bool, "bool literal"),
            ("'x'", LiteralKind::Char, "char literal"),
            ("b'x'", LiteralKind::Int, "byte literal"),
            ("42", LiteralKind::Int, "integer literal"),
            ("1.5", LiteralKind::Float, "decimal float literal"),
            ("1e10", LiteralKind::Float, "exponent float literal"),
            (
                "1E-10",
                LiteralKind::Float,
                "negative exponent float literal",
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

            assert_eq!(LiteralKind::from_ast(&literal), expected, "{label}");
        }
    }
}
