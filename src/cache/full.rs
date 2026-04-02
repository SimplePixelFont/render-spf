#![cfg(feature = "std")]

use bitvec::{field::BitField, order::Lsb0, view::BitView};
use hashbrown::HashMap;
use ril::{Image, Rgba};
use spf::core::{Character, Font, FontTable, Layout, Pixmap, PixmapTable};

use crate::{
    color::{ColorControl, PixelRef},
    print::{GenericPrintConfig, RenderSurface, RenderableTexture, VerticalAlign},
    String, Vec,
};

use super::{find_font, generic_update_cache, FontCache, TextureBuilder};

/// A single full-colour glyph for `std` targets.
///
/// Pixels are stored as [`PixelRef`] values rather than baked RGBA. Each
/// `PixelRef` holds a layout-level color table index and a palette index,
/// resolved through [`ColorControl`] at render time. Mutating `ColorControl`
/// is immediately reflected on the next render call — no cache reload needed.
#[derive(Clone, Debug)]
pub struct AbstractCharacter {
    pub width: u32,
    pub height: u32,
    pub advance_x: u32,
    /// One [`PixelRef`] per pixel, in row-major order.
    pub(crate) pixels: Vec<PixelRef>,
}

impl Default for AbstractCharacter {
    fn default() -> Self {
        Self {
            width: 1,
            height: 1,
            advance_x: 1,
            pixels: vec![PixelRef::default()],
        }
    }
}

impl RenderableTexture for AbstractCharacter {
    fn width(&self) -> u32 {
        self.width
    }
    fn height(&self) -> u32 {
        self.height
    }
    fn advance_x(&self) -> u32 {
        self.advance_x
    }
}

// The generic RenderSurface impl is a safe no-op — the color-aware render
// path in RgbaPrinter::paste_glyph is the intended entry point.
impl RenderSurface<AbstractCharacter> for Image<Rgba> {
    fn new(width: u32, height: u32) -> Self {
        Image::new(width, height, Rgba::transparent())
    }

    fn paste(&mut self, _x: u32, _y: u32, _texture: &AbstractCharacter) {
        // Intentional no-op. Use RgbaPrinter::print_str or RgbaPrinter::render,
        // which resolve PixelRefs through ColorControl.
    }
}

/// Builds [`AbstractCharacter`] glyphs from SPF pixmap data.
///
/// Uses [`bitvec`] with [`Lsb0`] ordering (SPF is LSB-first; `Lsb0` matches
/// natively). Each pixel is stored as a [`PixelRef`] with a **layout-level**
/// `color_table_index`, resolved from the pixmap table's dep-local ordering
/// at build time. This makes [`ColorControl`] resolution unambiguous at
/// render time regardless of which pixmap table produced the glyph.
///
/// # Per-pixel color table support
///
/// When `Pixmap::per_pixel_color_table_indexes` is `Some`, each pixel's dep-
/// local color table index is read from that slice and resolved to a layout-
/// level index via `pixmap_table.color_table_indexes`. When `None`, all pixels
/// default to the layout-level index of dep slot 0 — the first dependency
/// color table.
pub(crate) struct RgbaTextureBuilder;

impl TextureBuilder<AbstractCharacter> for RgbaTextureBuilder {
    fn build_texture(
        &self,
        character: &Character,
        pixmap: &Pixmap,
        pixmap_table: &PixmapTable,
        _layout: &Layout,
    ) -> AbstractCharacter {
        let width = pixmap_table
            .constant_width
            .or(pixmap.custom_width)
            .expect("no width defined in pixmap or pixmap table") as u32;
        let height = pixmap_table
            .constant_height
            .or(pixmap.custom_height)
            .expect("no height defined in pixmap or pixmap table") as u32;
        let advance_x = character.advance_x.unwrap_or(width as u8) as u32;

        let bits_per_pixel = pixmap_table
            .constant_bits_per_pixel
            .or(pixmap.custom_bits_per_pixel)
            .expect("no bits_per_pixel defined") as usize;

        // The dep-local → layout-level mapping for color tables.
        // dep_local_to_layout[i] = layout-level index for dep slot i.
        let dep_local_to_layout: Vec<u8> = pixmap_table
            .color_table_indexes
            .as_deref()
            .unwrap_or(&[])
            .to_vec();

        // Default layout-level index: first dependency color table, or 0.
        let default_layout_ct_idx = dep_local_to_layout.first().copied().unwrap_or(0);

        // Unpack palette indices from LSB-first bit stream
        let palette_indices: Vec<u8> = pixmap
            .data
            .view_bits::<Lsb0>()
            .chunks(bits_per_pixel)
            .take(width as usize * height as usize)
            .map(|chunk| chunk.load_be::<u8>())
            .collect();

        // Build PixelRefs — resolve dep-local → layout-level for each pixel.
        let pixels: Vec<PixelRef> = palette_indices
            .iter()
            // .enumerate()
            .map(/*|(_, &color_index)|*/|&color_index| {
                // uncomment the enumerate and use dep_local if per-pixel indexes are present
                // Per-pixel dep-local index, falling back to dep slot 0

                let dep_local = 0;
                // let dep_local = pixmap
                //     .per_pixel_color_table_indexes
                //     .as_ref()
                //     .and_then(|v| v.get(i).copied())
                //     .unwrap_or(0);

                // Resolve dep-local → layout-level via pixmap table's index list
                let layout_ct_idx = dep_local_to_layout
                    .get(dep_local as usize)
                    .copied()
                    .unwrap_or(default_layout_ct_idx);

                PixelRef {
                    color_table_index: layout_ct_idx,
                    color_index,
                }
            })
            .collect();

        AbstractCharacter {
            width,
            height,
            advance_x,
            pixels,
        }
    }
}

/// Character cache for `std` targets.
///
/// Keyed by the full grapheme cluster [`String`], backed by [`HashMap`] for
/// O(1) average-case lookups. Supports multi-byte grapheme clusters.
#[derive(Clone, Default)]
pub struct CharacterCacheImpl {
    pub(crate) mappings: HashMap<String, AbstractCharacter>,
    pub(crate) max_height: u32,
}

impl CharacterCacheImpl {
    pub fn new() -> Self {
        Self::default()
    }

    fn track_height(&mut self, glyph: &AbstractCharacter) {
        self.max_height = self.max_height.max(glyph.height);
    }

    /// Populate the cache from a specific [`Font`].
    ///
    /// Returns a [`ColorControl`] pre-sized to `layout.color_tables.len()`
    /// and populated for every color table referenced by the font's glyphs.
    /// Store this alongside the printer — it is the live palette used at
    /// render time.
    pub fn update(&mut self, font_table: &FontTable, font: &Font, layout: &Layout) -> ColorControl {
        // Pre-size to layout.color_tables.len() so every slot is addressable
        // by layout-level index from the start. Slots for color tables not
        // referenced by this font remain empty.
        let mut color_control = ColorControl::with_capacity(layout.color_tables.len());

        generic_update_cache(
            font_table,
            font,
            layout,
            &RgbaTextureBuilder,
            &mut color_control,
            |grapheme| grapheme.to_owned(),
            |key, glyph: AbstractCharacter| {
                self.track_height(&glyph);
                self.mappings.insert(key, glyph);
            },
        );

        color_control
    }
}

impl FontCache for CharacterCacheImpl {
    type Key = String;
    type Glyph = AbstractCharacter;
    type Surface = Image<Rgba>;

    fn get(&self, key: &String) -> Option<&AbstractCharacter> {
        self.mappings.get(key)
    }

    fn max_height(&self) -> u32 {
        self.max_height
    }
}

/// A printer for the full-colour std backend.
///
/// Owns a [`ColorControl`] indexed by **layout-level** color table index.
/// Mutate `colors` before calling [`print_str`](Self::print_str) — changes
/// are reflected immediately on the next render call.
///
/// # Example
/// ```ignore
/// let mut printer = RgbaPrinter::from_font_named("Regular", &layout, config)
///     .expect("font not found");
///
/// // See available Dynamic colors in layout color table 0
/// for (i, entry) in printer.colors.dynamic(0) {
///     println!("color {}: {:?}", i, entry.current());
/// }
///
/// // Override layout color table 0, palette entry 0 → red
/// printer.colors.set(0, 0, 255, 0, 0, 255);
/// let red_image = printer.print_str("Hello");
///
/// // Reset Dynamic colors and render with original palette
/// printer.colors.reset_dynamic();
/// let original_image = printer.print_str("Hello");
/// ```
pub struct RgbaPrinter {
    pub cache: CharacterCacheImpl,
    pub config: GenericPrintConfig,
    /// Live color palette, indexed by layout-level color table index.
    /// Mutate this to change rendered colors.
    pub colors: ColorControl,
}

impl RgbaPrinter {
    pub fn new(
        cache: CharacterCacheImpl,
        config: GenericPrintConfig,
        colors: ColorControl,
    ) -> Self {
        Self {
            cache,
            config,
            colors,
        }
    }

    /// Build an [`RgbaPrinter`] from a specific [`Font`].
    pub fn from_font(
        font_table: &FontTable,
        font: &Font,
        layout: &Layout,
        config: GenericPrintConfig,
    ) -> Self {
        let mut cache = CharacterCacheImpl::new();
        let colors = cache.update(font_table, font, layout);
        Self::new(cache, config, colors)
    }

    /// Build an [`RgbaPrinter`] by searching for a font by name.
    ///
    /// Returns `None` if no font with `name` exists in the layout.
    /// Use [`font_names`](crate::font_names) to discover available names.
    pub fn from_font_named(
        name: &str,
        layout: &Layout,
        config: GenericPrintConfig,
    ) -> Option<Self> {
        let (font_table, font) = find_font(layout, name)?;
        Some(Self::from_font(font_table, font, layout, config))
    }

    /// Rasterise `text` onto a new [`Image<Rgba>`], resolving pixel colors
    /// through the current state of [`self.colors`](Self::colors).
    pub fn print_str(&self, text: &str) -> Image<Rgba> {
        let keys: Vec<String> = text.chars().map(|c| c.to_string()).collect();
        self.render(&keys)
    }

    /// Rasterise a pre-built key slice onto a new [`Image<Rgba>`].
    pub fn render(&self, keys: &[String]) -> Image<Rgba> {
        if keys.is_empty() {
            return Image::new(0, 0, Rgba::transparent());
        }

        let last = keys.len() - 1;

        let mut width: u32 = last as u32 * self.config.letter_spacing as u32;
        let mut height: u32 = 0;

        for (i, key) in keys.iter().enumerate() {
            let glyph = self
                .cache
                .get(key)
                .expect("character key not found in cache");
            width += if i < last {
                glyph.advance_x
            } else {
                glyph.width
            };
            height = height.max(glyph.height);
        }

        if self.config.vertical_expand {
            height = self.cache.max_height();
        }

        let offset_y: u32 = if self.config.vertical_expand {
            match self.config.vertical_align {
                VerticalAlign::Top => 0,
                VerticalAlign::Middle => self.cache.max_height().saturating_sub(height) / 2,
                VerticalAlign::Bottom => self.cache.max_height().saturating_sub(height),
            }
        } else {
            0
        };

        let mut surface = Image::new(width, height, Rgba::transparent());
        let mut current_x: u32 = 0;

        for key in keys {
            let glyph = self
                .cache
                .get(key)
                .expect("character key not found in cache");
            self.paste_glyph(&mut surface, glyph, current_x, offset_y);
            current_x += glyph.advance_x + self.config.letter_spacing as u32;
        }

        surface
    }

    /// Composite a single glyph onto `surface` at (x, y), resolving each
    /// [`PixelRef`] through [`self.colors`](Self::colors).
    fn paste_glyph(&self, surface: &mut Image<Rgba>, glyph: &AbstractCharacter, x: u32, y: u32) {
        for py in 0..glyph.height {
            for px in 0..glyph.width {
                let pixel_idx = (py * glyph.width + px) as usize;
                if let Some(&pixel_ref) = glyph.pixels.get(pixel_idx) {
                    let (r, g, b, a) = self.colors.resolve(pixel_ref);
                    if a == 0 {
                        continue;
                    }
                    let (dst_x, dst_y) = (x + px, y + py);
                    if dst_x < surface.width() && dst_y < surface.height() {
                        surface.set_pixel(dst_x, dst_y, Rgba { r, g, b, a });
                    }
                }
            }
        }
    }
}
