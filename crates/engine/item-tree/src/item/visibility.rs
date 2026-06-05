use rg_ir_model::items::VisibilityLevel;
use rg_syntax::{AstNode as _, ast};

use super::FromAst;

impl FromAst for VisibilityLevel {
    type AstNode = Option<ast::Visibility>;
    type Context<'a> = ();

    fn from_ast(visibility: &Self::AstNode, _ctx: Self::Context<'_>) -> Self {
        let Some(visibility) = visibility else {
            return Self::Private;
        };

        let Some(inner) = visibility.visibility_inner() else {
            return Self::Public;
        };

        let Some(path) = inner.path() else {
            return Self::Unknown(visibility.syntax().text().to_string());
        };
        let path_text = path.syntax().text().to_string();

        if inner.in_token().is_some() {
            return Self::Restricted(path_text);
        }

        match path_text.as_str() {
            "crate" => Self::Crate,
            "super" => Self::Super,
            "self" => Self::Self_,
            _ => Self::Unknown(visibility.syntax().text().to_string()),
        }
    }
}
