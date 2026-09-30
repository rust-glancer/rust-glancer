#[lang = "format_arguments"]
pub struct Arguments;

pub struct Formatter<'a> { marker: &'a () }
pub struct Error;
pub type Result = crate::result::Result<(), Error>;
pub trait Debug {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> Result;
}
pub mod macros {
    #[rustc_builtin_macro]
    pub macro Debug($item:item) {}
}
pub use macros::Debug;
