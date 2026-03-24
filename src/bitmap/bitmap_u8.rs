use crate::Vec;
use crate::vec;

/// A tightly-packed 1-bit bitmap for per-glyph storage on embedded targets.
///
/// Unlike [`Bitmap`](super::Bitmap), bits flow continuously across row
/// boundaries with no per-row byte padding, keeping storage compact. 
/// Dimensions are `u8` (max 255×255), enough for all SPF glyphs.
#[derive(Debug, Clone, PartialEq)]
pub struct BitmapU8 {
    pub(crate) width: u8,
    pub(crate) height: u8,
    pub(crate) data: Vec<u8>,
}

impl BitmapU8 {
    /// Create a new tightly-packed bitmap with all pixels cleared to 0.
    pub fn new(width: u8, height: u8) -> Self {
        let total_bits = width as usize * height as usize;
        let length = (total_bits + 7) / 8;
        Self { width, height, data: vec![0u8; length] }
    }

    /// Create a [`BitmapU8`] from existing tightly-packed data.
    ///
    /// Returns an error if `data` is shorter than required for the given
    /// dimensions.
    pub fn from_data(width: u8, height: u8, data: Vec<u8>) -> Result<Self, &'static str> {
        let expected = (width as usize * height as usize + 7) / 8;
        if data.len() < expected {
            return Err("Data too short for dimensions");
        }
        Ok(Self { width, height, data })
    }

    /// Get the pixel at (x, y). Returns `None` if out of bounds.
    pub fn get_pixel(&self, x: u8, y: u8) -> Option<bool> {
        if x >= self.width || y >= self.height {
            return None;
        }
        // After the bounds check above, byte_index is guaranteed in-range.
        let global_bit = y as usize * self.width as usize + x as usize;
        let byte_index = global_bit / 8;
        let bit_index = 7 - (global_bit % 8); // MSB-first
        Some((self.data[byte_index] >> bit_index) & 1 == 1)
    }

    /// Set the pixel at (x, y). Returns `false` if out of bounds.
    pub fn set_pixel(&mut self, x: u8, y: u8, value: bool) -> bool {
        if x >= self.width || y >= self.height {
            return false;
        }
        // After the bounds check above, byte_index is guaranteed in-range.
        let global_bit = y as usize * self.width as usize + x as usize;
        let byte_index = global_bit / 8;
        let bit_index = 7 - (global_bit % 8); // MSB-first
        if value {
            self.data[byte_index] |= 1 << bit_index;
        } else {
            self.data[byte_index] &= !(1 << bit_index);
        }
        true
    }

    pub fn width(&self) -> u8 { self.width }
    pub fn height(&self) -> u8 { self.height }
    pub fn data(&self) -> &[u8] { &self.data }

    /// Clear all pixels to 0.
    pub fn clear(&mut self) { self.data.fill(0); }

    /// Set all pixels to 1.
    pub fn fill(&mut self) { self.data.fill(0xFF); }
}