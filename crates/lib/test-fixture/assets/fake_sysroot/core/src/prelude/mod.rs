pub mod rust_2024 {
    pub use crate::{
        cfg, cfg_select, column, concat, env, file, format_args, format_args_nl, include_bytes,
        include_str, line, module_path, option_env, stringify,
    };
    pub use crate::{Fn, FnMut, FnOnce};
    pub use crate::iter::{IntoIterator, Iterator};
    pub use crate::option::Option;
    pub use crate::result::Result;
}
