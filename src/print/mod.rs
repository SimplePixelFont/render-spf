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
#[derive(Clone, Debug)]
pub struct GenericPrintConfig {
    /// Extra pixels inserted between each glyph.
    pub letter_spacing: u8,
    /// When `true`, the surface height is set to the font's maximum glyph
    /// height and glyphs are positioned according to `vertical_align`.
    pub vertical_expand: bool,
    pub vertical_align: VerticalAlign,
    /// Whether `print_str`'s shaping pass may match a font's multi-cluster
    /// ligature entries (e.g. "ffi" as one glyph). Non-ligature entries (a
    /// single cluster spanning multiple codepoints, like a base letter plus
    /// combining accent) are never affected by this, since only entries
    /// whose key spans more than one grapheme cluster are ligatures.
    pub allow_ligatures: bool,
    /// Blank pixels added around the whole run, outside the text's own
    /// bounding box — not between glyphs (that's `letter_spacing`). Zero by
    /// default: existing callers see no size change. A processor effect
    /// that reaches beyond a glyph's own pixmap rect (an outline or glow a
    /// few pixels wide) has nowhere to draw without this — the surface is
    /// otherwise cropped exactly to the text, so ink at the outermost edge
    /// has no margin to spread into.
    pub padding_left: u16,
    pub padding_top: u16,
    pub padding_right: u16,
    pub padding_bottom: u16,
}

impl Default for GenericPrintConfig {
    fn default() -> Self {
        Self {
            letter_spacing: 0,
            vertical_expand: false,
            vertical_align: VerticalAlign::default(),
            allow_ligatures: true,
            padding_left: 0,
            padding_top: 0,
            padding_right: 0,
            padding_bottom: 0,
        }
    }
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
///
/// `config`'s `padding_*` fields add blank margin around the run: `width`/
/// `height` grow by `padding_left + padding_right`/`padding_top +
/// padding_bottom`, and every glyph is shifted by `(padding_left,
/// padding_top)`. `vertical_align`'s block-alignment math is computed
/// first, against the unpadded height, exactly as without padding —
/// padding is applied on top as a uniform shift, not folded into the
/// alignment decision itself. An empty `keys` still lays out to `0x0`
/// with no padding — nothing to pad around.
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
        width += if i < last {
            glyph.advance_x()
        } else {
            glyph.width()
        };
        natural_height = natural_height.max(glyph.height());
    }

    let unpadded_height = if config.vertical_expand {
        cache.max_height()
    } else {
        natural_height
    };

    let offset_y: u32 = (if config.vertical_expand {
        match config.vertical_align {
            VerticalAlign::Top => 0,
            VerticalAlign::Middle => unpadded_height.saturating_sub(natural_height) / 2,
            VerticalAlign::Bottom => unpadded_height.saturating_sub(natural_height),
        }
    } else {
        0
    }) + config.padding_top as u32;

    let width = width + config.padding_left as u32 + config.padding_right as u32;
    let height = unpadded_height + config.padding_top as u32 + config.padding_bottom as u32;

    let mut glyphs = Vec::with_capacity(keys.len());
    let mut current_x: u32 = config.padding_left as u32;
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
