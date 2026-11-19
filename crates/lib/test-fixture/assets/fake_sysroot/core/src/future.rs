#[lang = "future_trait"]
pub trait Future {
    #[lang = "future_output"]
    type Output;
}

#[rustc_diagnostic_item = "IntoFuture"]
pub trait IntoFuture {
    type Output;
    type IntoFuture: Future<Output = Self::Output>;

    fn into_future(self) -> Self::IntoFuture;
}

impl<F: Future> IntoFuture for F {
    type Output = F::Output;
    type IntoFuture = F;

    fn into_future(self) -> F { self }
}

pub struct Ready<T>(T);

impl<T> Future for Ready<T> {
    type Output = T;
}

pub fn ready<T>(value: T) -> Ready<T> { Ready(value) }
