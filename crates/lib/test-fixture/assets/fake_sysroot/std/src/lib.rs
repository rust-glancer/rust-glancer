extern crate self as std;
extern crate alloc;
extern crate core;

pub mod prelude;

pub use core::{
    cfg, cfg_select, column, concat, env, file, format_args, format_args_nl, include_bytes,
    include_str, line, module_path, option_env, stringify,
};
pub use core::ops;
pub use alloc::string;
pub use alloc::string::String;
pub use alloc::vec;
pub use alloc::vec::Vec;

#[macro_export]
macro_rules! format {
    ($($args:tt)*) => {
        $crate::__export::format_args!($($args)*)
    };
}

pub mod __export {
    pub use core::format_args;
}
