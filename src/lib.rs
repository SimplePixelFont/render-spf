#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(not(feature = "std"))]
extern crate alloc;

#[cfg(feature = "std")]
pub(crate) use std::string::String;
#[cfg(feature = "std")]
pub(crate) use std::vec;
#[cfg(feature = "std")]
pub(crate) use std::vec::Vec;

#[cfg(not(feature = "std"))]
pub(crate) use alloc::vec;
#[cfg(not(feature = "std"))]
pub(crate) use alloc::vec::Vec;

pub mod bitmap;
pub use bitmap::*;

pub mod vecmap;
pub use vecmap::*;

pub mod utilities;
pub use utilities::*;

pub mod print;
pub use print::*;

pub mod cache;
pub use cache::{find_font, font_names, FontCache, Printer, EmbeddedPrinter};
#[cfg(feature = "std")]
pub use cache::{RgbaPrinter};