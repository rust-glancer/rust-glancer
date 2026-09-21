//! Analysis cursor symbols built from generic indexed source facts.

mod adapter;
mod index;
mod resolver;

pub(crate) use self::adapter::{SourceSymbol, SourceSymbolRole};
pub(crate) use self::index::SourceSymbolIndex;
pub(crate) use self::resolver::SourceSymbolResolver;
