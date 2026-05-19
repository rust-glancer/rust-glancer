//! Minimal symbol support for vendored macro expansion modules.
//!
//! rust-glancer already has project-wide text interning in `rg_text`. Macro
//! expansion only needs a small local vocabulary of static keywords plus cheap
//! owned symbols, so this module keeps that compatibility surface close to the
//! vendored macro code instead of becoming another general text subsystem.

use std::{fmt, hash::Hash};

use smol_str::SmolStr;

#[derive(Clone, PartialEq, Eq, Hash)]
pub enum Symbol {
    Static(&'static str),
    Owned(SmolStr),
}

impl Symbol {
    pub const fn static_(text: &'static str) -> Self {
        Self::Static(text)
    }

    pub fn new(text: &str) -> Self {
        Self::Owned(SmolStr::new(text))
    }

    pub fn integer(value: usize) -> Self {
        match value {
            0 => sym::INTEGER_0,
            1 => sym::INTEGER_1,
            2 => sym::INTEGER_2,
            3 => sym::INTEGER_3,
            4 => sym::INTEGER_4,
            5 => sym::INTEGER_5,
            6 => sym::INTEGER_6,
            7 => sym::INTEGER_7,
            8 => sym::INTEGER_8,
            9 => sym::INTEGER_9,
            10 => sym::INTEGER_10,
            11 => sym::INTEGER_11,
            12 => sym::INTEGER_12,
            13 => sym::INTEGER_13,
            14 => sym::INTEGER_14,
            15 => sym::INTEGER_15,
            value => Self::new(&value.to_string()),
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::Static(text) => text,
            Self::Owned(text) => text.as_str(),
        }
    }
}

impl fmt::Debug for Symbol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.as_str(), f)
    }
}

impl fmt::Display for Symbol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[allow(non_upper_case_globals)]
pub mod sym {
    use super::Symbol;

    pub const attr: Symbol = Symbol::static_("attr");
    pub const concat: Symbol = Symbol::static_("concat");
    pub const const_: Symbol = Symbol::static_("const");
    pub const count: Symbol = Symbol::static_("count");
    pub const crate_: Symbol = Symbol::static_("crate");
    pub const derive: Symbol = Symbol::static_("derive");
    pub const dollar_crate: Symbol = Symbol::static_("$crate");
    pub const false_: Symbol = Symbol::static_("false");
    pub const ignore: Symbol = Symbol::static_("ignore");
    pub const index: Symbol = Symbol::static_("index");
    pub const len: Symbol = Symbol::static_("len");
    pub const let_: Symbol = Symbol::static_("let");
    pub const missing: Symbol = Symbol::static_("<missing>");
    pub const true_: Symbol = Symbol::static_("true");
    pub const unsafe_: Symbol = Symbol::static_("unsafe");
    pub const underscore: Symbol = Symbol::static_("_");

    pub const INTEGER_0: Symbol = Symbol::static_("0");
    pub const INTEGER_1: Symbol = Symbol::static_("1");
    pub const INTEGER_2: Symbol = Symbol::static_("2");
    pub const INTEGER_3: Symbol = Symbol::static_("3");
    pub const INTEGER_4: Symbol = Symbol::static_("4");
    pub const INTEGER_5: Symbol = Symbol::static_("5");
    pub const INTEGER_6: Symbol = Symbol::static_("6");
    pub const INTEGER_7: Symbol = Symbol::static_("7");
    pub const INTEGER_8: Symbol = Symbol::static_("8");
    pub const INTEGER_9: Symbol = Symbol::static_("9");
    pub const INTEGER_10: Symbol = Symbol::static_("10");
    pub const INTEGER_11: Symbol = Symbol::static_("11");
    pub const INTEGER_12: Symbol = Symbol::static_("12");
    pub const INTEGER_13: Symbol = Symbol::static_("13");
    pub const INTEGER_14: Symbol = Symbol::static_("14");
    pub const INTEGER_15: Symbol = Symbol::static_("15");
}
