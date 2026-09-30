pub enum Result<T, E> {
    Ok(T),
    Err(E),
}

impl<T, E> crate::ops::Try for Result<T, E> {
    type Output = T;
}
