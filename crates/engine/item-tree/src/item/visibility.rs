use ra_syntax::{AstNode as _, ast};

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    derive_more::Display,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
pub enum VisibilityLevel {
    #[display("private")]
    Private,
    #[display("pub")]
    Public,
    #[display("pub")]
    Crate,
    #[display("pub(super)")]
    Super,
    #[display("pub(self)")]
    Self_,
    #[display("pub(in {_0})")]
    Restricted(String),
    #[display("{_0}")]
    Unknown(String),
}

impl VisibilityLevel {
    pub fn from_ast(visibility: Option<ast::Visibility>) -> Self {
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
