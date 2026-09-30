#[lang = "Option"]
pub enum Option<T> {
    Some(T),
    None,
}

impl<T> crate::ops::Try for Option<T> {
    type Output = T;
}
