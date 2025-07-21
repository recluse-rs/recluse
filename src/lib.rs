#![doc = include_str!("../README.md")]

pub mod worker;

pub use worker::*;

#[cfg(feature = "reqwest")]
pub mod downloader;

#[cfg(feature = "reqwest")]
pub use downloader::*;