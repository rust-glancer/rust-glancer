// Builtin bodies are placeholders: rust-glancer dispatches these definitions through the
// `rustc_builtin_macro` marker after ordinary macro name resolution has selected them.
#[rustc_builtin_macro]
#[macro_export]
macro_rules! cfg {
    ($($args:tt)*) => {{ /* compiler built-in */ }};
}

#[rustc_builtin_macro]
#[macro_export]
macro_rules! cfg_select {
    ($($args:tt)*) => {{ /* compiler built-in */ }};
}

#[rustc_builtin_macro]
#[macro_export]
macro_rules! column {
    ($($args:tt)*) => {{ /* compiler built-in */ }};
}

#[rustc_builtin_macro]
#[macro_export]
macro_rules! concat {
    ($($args:tt)*) => {{ /* compiler built-in */ }};
}

#[rustc_builtin_macro]
#[macro_export]
macro_rules! env {
    ($($args:tt)*) => {{ /* compiler built-in */ }};
}

#[rustc_builtin_macro]
#[macro_export]
macro_rules! file {
    ($($args:tt)*) => {{ /* compiler built-in */ }};
}

#[rustc_builtin_macro]
#[macro_export]
macro_rules! format_args {
    ($($args:tt)*) => {{ /* compiler built-in */ }};
}

#[rustc_builtin_macro]
#[macro_export]
macro_rules! format_args_nl {
    ($($args:tt)*) => {{ /* compiler built-in */ }};
}

#[rustc_builtin_macro]
#[macro_export]
macro_rules! include_bytes {
    ($($args:tt)*) => {{ /* compiler built-in */ }};
}

#[rustc_builtin_macro]
#[macro_export]
macro_rules! include_str {
    ($($args:tt)*) => {{ /* compiler built-in */ }};
}

#[rustc_builtin_macro]
#[macro_export]
macro_rules! line {
    ($($args:tt)*) => {{ /* compiler built-in */ }};
}

#[rustc_builtin_macro]
#[macro_export]
macro_rules! module_path {
    ($($args:tt)*) => {{ /* compiler built-in */ }};
}

#[rustc_builtin_macro]
#[macro_export]
macro_rules! option_env {
    ($($args:tt)*) => {{ /* compiler built-in */ }};
}

#[rustc_builtin_macro]
#[macro_export]
macro_rules! stringify {
    ($($args:tt)*) => {{ /* compiler built-in */ }};
}
