#![doc = include_str!("../README.md")]

#[cfg(feature = "reqwest")]
pub mod downloader;

#[cfg(feature = "reqwest")]
pub use downloader::*;

pub mod select;

pub mod worker;

pub use worker::*;

pub mod utils;

pub use utils::*;