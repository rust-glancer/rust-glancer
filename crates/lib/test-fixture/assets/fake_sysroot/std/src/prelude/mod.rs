pub mod rust_2024 {
    pub use alloc::string::String;
    pub use alloc::vec::Vec;
    pub use crate::format;
    pub use core::{
        cfg, cfg_select, column, concat, env, file, format_args, format_args_nl, include_bytes,
        include_str, line, module_path, option_env, stringify,
    };
    pub use core::{Fn, FnMut, FnOnce};
    pub use core::iter::{IntoIterator, Iterator};
    pub use core::option::Option;
    pub use core::result::Result;
}
