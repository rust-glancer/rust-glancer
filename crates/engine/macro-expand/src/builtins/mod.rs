//! Builtin macro expanders that can be modeled from token trees alone.
//!
//! These helpers deliberately avoid def-map concepts. Callers decide when a builtin has resolved
//! and provide any target-specific environment the builtin needs.

mod cfg_select;

pub use self::cfg_select::expand as expand_cfg_select;
