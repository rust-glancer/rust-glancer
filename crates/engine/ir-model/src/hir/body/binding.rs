use wincode::{SchemaRead, SchemaWrite};

use rg_parse::Span;
use rg_text::Name;

use crate::{Mutability, ScopeId, items::TypeRef};
use rg_std::{MemorySize, Shrink};

use super::BodySource;

/// One local binding introduced by a parameter or `let`.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct BindingData {
    pub source: BodySource,
    pub name_span: Option<Span>,
    pub scope: ScopeId,
    pub kind: BindingKind,
    pub name: Option<Name>,
    pub annotation: Option<TypeRef>,
}

/// Local binding category.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    derive_more::Display,
    SchemaRead,
    SchemaWrite,
    MemorySize,
    Shrink,
)]
#[memsize(leaf)]
#[shrink(leaf)]
pub enum BindingKind {
    /// `param` in `fn f(param: Type)`.
    #[display("param")]
    Param,
    /// `self`, `&self`, or another receiver parameter.
    #[display("self_param")]
    SelfParam(BodySelfParamKind),
    /// `let name = value`.
    #[display("let")]
    Let,
}

/// Receiver form written by a function's self parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, SchemaRead, SchemaWrite, Shrink)]
#[shrink(leaf)]
pub enum BodySelfParamKind {
    Value,
    Reference { mutability: Mutability },
    Explicit,
}
