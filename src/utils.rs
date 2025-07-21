use std::fmt::Display;

use log::warn;

pub fn print_errors<T, E>(value: Result<T, E>) -> Result<T, E>
where E: Display {
    if let Err(ref why) = value {
        warn!("{}", why);
    }
    
    value
}