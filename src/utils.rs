use std::fmt::Display;

use log::warn;
use tower::{ServiceBuilder, layer::util::Stack, filter::FilterLayer};

/// Identity function. If it receives an Err variant, outputs it via log::warn!,
/// therefore the error must be `Display`.
pub fn print_error<T, E>(value: Result<T, E>) -> Result<T, E>
where E: Display {
    if let Err(ref why) = value {
        warn!("{}", why);
    }
    
    value
}

pub trait ServiceBuilderUtilsExt<T, E>
where
    E: Display {
    type Output;

    /// Output all Err variant items via log::warn! and discard them.
    /// Converts output type of the rest from Result<T, E> to T.
    fn print_and_drop_request_error(self) -> Self::Output;
}

impl<L, T, E> ServiceBuilderUtilsExt<T, E> for ServiceBuilder<L>
where
    E: Display {
    type Output = ServiceBuilder<Stack<FilterLayer<fn(Result<T, E>) -> Result<T, E>>, L>>;
    
    fn print_and_drop_request_error(self) -> Self::Output {
        self.filter(print_error)
    }
}