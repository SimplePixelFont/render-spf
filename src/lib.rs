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
pub(crate) use alloc::string::String;
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

// Not std-gated: generic_update_cache requires a &mut ColorControl for both
// backends (the embedded/no_std one discards it immediately — it's
// monochrome — but still needs the type available to satisfy the shared
// signature). color.rs itself has no std-only dependencies.
pub mod color;
pub use color::{ColorControl, ColorEntry, ColorType, PixelRef};

pub mod cache;
pub use cache::{find_font, font_names, FontCache, Printer, EmbeddedPrinter};
#[cfg(feature = "std")]
pub use cache::RgbaPrinter;