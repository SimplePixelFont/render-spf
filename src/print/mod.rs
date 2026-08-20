use crate::cache::FontCache;
use crate::Vec;

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

/// A single glyph's computed position within a [`Placement`], produced by
/// [`layout`]. `width`/`height`/`advance_x` are copied from the glyph at
/// layout time, so a `Placement` fully describes a run without requiring a
/// second cache lookup just to know a glyph's extent.
#[derive(Clone, Debug)]
pub struct PlacedGlyph<K> {
    /// The cache key this placement was computed from — look this back up
    /// via [`FontCache::get`] to retrieve the actual glyph to paint.
    pub key: K,
    pub dst_x: u32,
    pub dst_y: u32,
    pub width: u32,
    pub height: u32,
    pub advance_x: u32,
}

/// The fully-computed layout of a run of glyphs, before rasterisation.
///
/// Produced by [`layout`], consumed by [`generic_print`] and by
/// [`RgbaPrinter::render`](crate::cache::RgbaPrinter::render) — the seam
/// that decouples "where does each glyph go" (this struct) from "how is a
/// glyph's texture painted" (each backend's own paste step). A color-aware
/// backend that can't use the generic [`RenderSurface::paste`] no-op still
/// gets the layout math for free instead of re-deriving it.
///
/// `K` is generic over [`FontCache::Key`] rather than a fixed glyph-id type,
/// since no shaping stage exists yet to produce one — today `K` is `String`
/// (std backend) or `u8` (embedded backend).
#[derive(Clone, Debug)]
pub struct Placement<K> {
    pub width: u32,
    pub height: u32,
    pub glyphs: Vec<PlacedGlyph<K>>,
}

/// Core layout pass: computes glyph positions without touching a surface.
///
/// Shared by every backend so the width/height/vertical-alignment math
/// exists in exactly one place.
///
/// `offset_y` is computed from this run's own tallest glyph
/// (`natural_height`), not from the (possibly taller) surface height —
/// `vertical_expand` only changes how tall the *surface* is, not what "no
/// offset" means. The previous implementation computed `offset_y` from
/// `height` after `height` had already been reassigned to
/// `cache.max_height()`, which made `Middle`/`Bottom` always yield 0.
///
/// Note this computes one `offset_y` for the whole run — glyphs of
/// differing heights within the run share a top offset (block alignment)
/// rather than being individually centred. SPF has no baseline concept,
/// so this is the deliberate choice, not an oversight.
pub fn layout<C>(keys: &[C::Key], config: &GenericPrintConfig, cache: &C) -> Placement<C::Key>
where
    C: FontCache,
{
    if keys.is_empty() {
        return Placement {
            width: 0,
            height: 0,
            glyphs: Vec::new(),
        };
    }

    let last = keys.len() - 1;
    let mut width: u32 = last as u32 * config.letter_spacing as u32;
    let mut natural_height: u32 = 0;

    for (i, key) in keys.iter().enumerate() {
        let glyph = cache.get(key).expect("character key not found in cache");
        width += if i < last { glyph.advance_x() } else { glyph.width() };
        natural_height = natural_height.max(glyph.height());
    }

    let height = if config.vertical_expand {
        cache.max_height()
    } else {
        natural_height
    };

    let offset_y: u32 = if config.vertical_expand {
        match config.vertical_align {
            VerticalAlign::Top => 0,
            VerticalAlign::Middle => height.saturating_sub(natural_height) / 2,
            VerticalAlign::Bottom => height.saturating_sub(natural_height),
        }
    } else {
        0
    };

    let mut glyphs = Vec::with_capacity(keys.len());
    let mut current_x: u32 = 0;
    for key in keys {
        let glyph = cache.get(key).expect("character key not found in cache");
        glyphs.push(PlacedGlyph {
            key: key.clone(),
            dst_x: current_x,
            dst_y: offset_y,
            width: glyph.width(),
            height: glyph.height(),
            advance_x: glyph.advance_x(),
        });
        current_x += glyph.advance_x() + config.letter_spacing as u32;
    }

    Placement {
        width,
        height,
        glyphs,
    }
}

/// Core rasterisation loop. For ergonomics use [`Printer::print`](crate::cache::Printer::print).
pub fn generic_print<C>(keys: &[C::Key], config: &GenericPrintConfig, cache: &C) -> C::Surface
where
    C: FontCache,
{
    let placement = layout(keys, config, cache);
    let mut surface = C::Surface::new(placement.width, placement.height);

    for placed in &placement.glyphs {
        let glyph = cache
            .get(&placed.key)
            .expect("character key not found in cache");
        surface.paste(placed.dst_x, placed.dst_y, glyph);
    }

    surface
}