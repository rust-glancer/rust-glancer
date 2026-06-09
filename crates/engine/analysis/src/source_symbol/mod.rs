//! Analysis cursor symbols built from generic indexed source facts.

mod adapter;
mod index;
mod resolver;

pub(crate) use adapter::{SourceSymbol, SourceSymbolRole};
pub(crate) use index::SourceSymbolIndex;
pub(crate) use resolver::SourceSymbolResolver;
