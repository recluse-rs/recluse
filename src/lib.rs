#![doc = include_str!("../README.md")]

#[cfg(feature = "reqwest")]
pub mod downloader;

#[cfg(feature = "reqwest")]
pub use downloader::*;

#[cfg(feature = "limit")]
pub mod limit;

#[cfg(feature = "limit")]
pub use limit::*;

pub mod select;

pub mod worker;

pub use worker::*;

pub mod utils;

pub use utils::*;