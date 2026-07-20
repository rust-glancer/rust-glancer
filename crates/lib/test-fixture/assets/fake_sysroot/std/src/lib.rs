extern crate self as std;
extern crate alloc as alloc_crate;
extern crate core;

pub mod prelude;
pub mod sync;

pub use core::{
    cfg, cfg_select, column, concat, env, file, format_args, format_args_nl, include_bytes,
    include_str, line, module_path, option_env, stringify,
};
pub use core::ops;
pub use alloc_crate::string;
pub use alloc_crate::string::String;
pub use alloc_crate::vec;
pub use alloc_crate::vec::Vec;

#[macro_export]
macro_rules! format {
    ($($args:tt)*) => {
        $crate::__export::format_args!($($args)*)
    };
}

pub mod __export {
    pub use core::format_args;
}
