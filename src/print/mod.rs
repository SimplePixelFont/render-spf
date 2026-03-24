use crate::cache::FontCache;

/// A single character glyph that can be rendered onto a [`RenderSurface`].
///
/// All dimension methods return `u32` regardless of internal storage type.
/// Embedded glyph types store dimensions as `u8` internally but widen to
/// `u32` at the trait boundary, keeping the rendering pipeline uniform.
pub trait RenderableTexture {
    fn width(&self) -> u32;
    fn height(&self) -> u32;

    /// Horizontal cursor advance after rendering this glyph.
    /// May differ from `width()` for glyphs with side-bearings.
    fn advance_x(&self) -> u32;
}

/// A canvas that glyphs are composited onto.
///
/// Coordinates are always `u32`. The surface is the output of rendering
/// and has no embedded storage constraint.
pub trait RenderSurface<T: RenderableTexture> {
    /// Create a blank surface of the given pixel dimensions.
    fn new(width: u32, height: u32) -> Self;

    /// Composite a glyph at pixel position (x, y).
    fn paste(&mut self, x: u32, y: u32, texture: &T);
}

/// Controls vertical positioning when [`GenericPrintConfig::vertical_expand`]
/// is enabled.
#[derive(Clone, Default, Debug)]
pub enum VerticalAlign {
    #[default]
    Top,
    Middle,
    Bottom,
}

/// Configuration for a [`Printer::print`](crate::cache::Printer::print) call.
#[derive(Clone, Default, Debug)]
pub struct GenericPrintConfig {
    /// Extra pixels inserted between each glyph.
    pub letter_spacing: u8,
    /// When `true`, the surface height is set to the font's maximum glyph
    /// height and glyphs are positioned according to `vertical_align`.
    pub vertical_expand: bool,
    pub vertical_align: VerticalAlign,
}

/// Core rasterisation loop. the public For ergonomics use [`Printer::print`](crate::cache::Printer::print).
pub fn generic_print<C>(
    keys: &[C::Key],
    config: &GenericPrintConfig,
    cache: &C,
) -> C::Surface
where
    C: FontCache,
{
    if keys.is_empty() {
        return C::Surface::new(0, 0);
    }

    let last = keys.len() - 1;
    let mut width: u32 = last as u32 * config.letter_spacing as u32;
    let mut height: u32 = 0;

    for (i, key) in keys.iter().enumerate() {
        let glyph = cache.get(key).expect("character key not found in cache");
        width += if i < last { glyph.advance_x() } else { glyph.width() };
        height = height.max(glyph.height());
    }

    if config.vertical_expand {
        height = cache.max_height();
    }

    let offset_y: u32 = if config.vertical_expand {
        match config.vertical_align {
            VerticalAlign::Top => 0,
            VerticalAlign::Middle => cache.max_height().saturating_sub(height) / 2,
            VerticalAlign::Bottom => cache.max_height().saturating_sub(height),
        }
    } else {
        0
    };

    let mut surface = C::Surface::new(width, height);
    let mut current_x: u32 = 0;

    for key in keys {
        let glyph = cache.get(key).expect("character key not found in cache");
        surface.paste(current_x, offset_y, glyph);
        current_x += glyph.advance_x() + config.letter_spacing as u32;
    }

    surface
}