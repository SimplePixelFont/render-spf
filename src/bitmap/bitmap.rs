use super::BitmapU8;
use crate::Vec;
use crate::vec;

/// A loosely-packed 1-bit bitmap canvas.
///
/// Pixels are stored in row-major order, MSB-first. Each row is padded to
/// the nearest byte boundary.
#[derive(Debug, Clone)]
pub struct Bitmap {
    pub width: usize,
    pub height: usize,
    pub data: Vec<u8>,
}

/// Private PasteSource trait
///
/// Eliminates the four near-identical paste loops by abstracting over both
/// Bitmap and BitmapU8 as blit sources. Not part of the public API.
trait PasteSource {
    fn src_width(&self) -> usize;
    fn src_height(&self) -> usize;
    fn src_pixel(&self, x: usize, y: usize) -> Option<bool>;
}

impl PasteSource for Bitmap {
    fn src_width(&self) -> usize { self.width }
    fn src_height(&self) -> usize { self.height }
    fn src_pixel(&self, x: usize, y: usize) -> Option<bool> {
        self.get_pixel(x, y)
    }
}

impl PasteSource for BitmapU8 {
    fn src_width(&self) -> usize { self.width as usize }
    fn src_height(&self) -> usize { self.height as usize }
    fn src_pixel(&self, x: usize, y: usize) -> Option<bool> {
        self.get_pixel(x as u8, y as u8)
    }
}

// ---------------------------------------------------------------------------

impl Bitmap {
    /// Create a new bitmap with all pixels cleared to 0 (transparent).
    pub fn new(width: usize, height: usize) -> Self {
        let bytes_per_row = (width + 7) / 8;
        Self { width, height, data: vec![0u8; bytes_per_row * height] }
    }

    /// Create a [`Bitmap`] from existing loosely-packed data.
    ///
    /// `data` must be row-major with each row padded to the nearest byte
    /// boundary. The MSB of the first byte is the top-left pixel.
    pub fn from_data(width: usize, height: usize, data: Vec<u8>) -> Result<Self, &'static str> {
        let expected = ((width + 7) / 8) * height;
        if data.len() != expected {
            return Err("Data length doesn't match dimensions");
        }
        Ok(Self { width, height, data })
    }

    pub fn width(&self) -> usize { self.width }
    pub fn height(&self) -> usize { self.height }
    pub fn data(&self) -> &[u8] { &self.data }

    #[inline]
    fn bytes_per_row(&self) -> usize {
        (self.width + 7) / 8
    }

    /// Set a pixel at (x, y). Returns `false` if out of bounds.
    pub fn set_pixel(&mut self, x: usize, y: usize, value: bool) -> bool {
        if x >= self.width || y >= self.height {
            return false;
        }
        let byte_index = y * self.bytes_per_row() + x / 8;
        let bit_index = 7 - (x % 8); // MSB-first
        if value {
            self.data[byte_index] |= 1 << bit_index;
        } else {
            self.data[byte_index] &= !(1 << bit_index);
        }
        true
    }

    /// Get a pixel at (x, y). Returns `None` if out of bounds.
    pub fn get_pixel(&self, x: usize, y: usize) -> Option<bool> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let byte_index = y * self.bytes_per_row() + x / 8;
        let bit_index = 7 - (x % 8); // MSB-first
        Some((self.data[byte_index] >> bit_index) & 1 == 1)
    }

    /// Clear all pixels to 0 (transparent).
    pub fn clear(&mut self) {
        self.data.fill(0);
    }

    /// Set all pixels to 1 (opaque).
    ///
    /// Note: padding bits at the end of each row are also set but are never
    /// read by pixel accessors and do not affect rendered output.
    pub fn fill(&mut self) {
        self.data.fill(0xFF);
    }

    // ---------------------------------------------------------------------------
    // Core paste implementation — all four public paste methods delegate here.
    //
    // `transparent`: when true, only "on" (1) source pixels are drawn;
    // "off" (0) source pixels leave the destination unchanged.
    // ---------------------------------------------------------------------------
    fn paste_impl(&mut self, source: &impl PasteSource, x: isize, y: isize, transparent: bool) {
        for src_y in 0..source.src_height() {
            for src_x in 0..source.src_width() {
                let dst_x = x + src_x as isize;
                let dst_y = y + src_y as isize;
                if dst_x < 0
                    || dst_y < 0
                    || dst_x >= self.width as isize
                    || dst_y >= self.height as isize
                {
                    continue;
                }
                match source.src_pixel(src_x, src_y) {
                    Some(true) => {
                        self.set_pixel(dst_x as usize, dst_y as usize, true);
                    }
                    Some(false) if !transparent => {
                        self.set_pixel(dst_x as usize, dst_y as usize, false);
                    }
                    _ => {}
                }
            }
        }
    }

    /// Paste another [`Bitmap`] at (x, y), overwriting destination pixels.
    /// Pixels outside destination bounds are silently clipped.
    pub fn paste(&mut self, source: &Bitmap, x: isize, y: isize) {
        self.paste_impl(source, x, y, false);
    }

    /// Paste a [`BitmapU8`] at (x, y), overwriting destination pixels.
    /// Pixels outside destination bounds are silently clipped.
    pub fn paste_u8(&mut self, source: &BitmapU8, x: isize, y: isize) {
        self.paste_impl(source, x, y, false);
    }

    /// Paste a [`Bitmap`] in transparency mode.
    /// Only "on" (1) source pixels are drawn; "off" (0) pixels leave the
    /// destination unchanged.
    pub fn paste_transparent(&mut self, source: &Bitmap, x: isize, y: isize) {
        self.paste_impl(source, x, y, true);
    }

    /// Paste a [`BitmapU8`] in transparency mode.
    /// Only "on" (1) source pixels are drawn; "off" (0) pixels leave the
    /// destination unchanged.
    pub fn paste_transparent_u8(&mut self, source: &BitmapU8, x: isize, y: isize) {
        self.paste_impl(source, x, y, true);
    }

    /// Scale the bitmap by an integer factor, returning a new [`Bitmap`].
    /// Each pixel is expanded into a `scale_factor × scale_factor` block.
    ///
    /// # Panics
    /// Panics if `scale_factor` is 0.
    pub fn scale(&self, scale_factor: usize) -> Self {
        assert!(scale_factor > 0, "scale factor must be at least 1");

        if scale_factor == 1 {
            return self.clone();
        }

        let mut scaled = Bitmap::new(self.width * scale_factor, self.height * scale_factor);
        for y in 0..self.height {
            for x in 0..self.width {
                if let Some(pixel) = self.get_pixel(x, y) {
                    for dy in 0..scale_factor {
                        for dx in 0..scale_factor {
                            scaled.set_pixel(x * scale_factor + dx, y * scale_factor + dy, pixel);
                        }
                    }
                }
            }
        }
        scaled
    }
}

impl core::fmt::Display for Bitmap {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        for y in 0..self.height {
            for x in 0..self.width {
                if let Some(pixel) = self.get_pixel(x, y) {
                    write!(f, "{}", if pixel { "1" } else { "0" })?;
                }
            }
            if y + 1 != self.height {
                writeln!(f)?;
            }
        }
        Ok(())
    }
}