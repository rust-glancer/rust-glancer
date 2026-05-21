//! Builtin macro helpers that can be modeled from token trees alone.
//!
//! These helpers deliberately avoid def-map concepts. Callers decide when a builtin has resolved,
//! when target cfg is available, and how the resulting token streams should be lowered.

mod cfg_select;

pub use self::cfg_select::{CfgSelect, CfgSelectArm};
