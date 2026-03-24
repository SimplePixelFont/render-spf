use bitvec::{field::BitField, order::Lsb0, view::BitView};
use hashbrown::HashMap;
use ril::{Image, Rgba};
use spf::core::{Character, Font, FontTable, Layout, Pixmap, PixmapTable};

use crate::{
    String, Vec,
    print::{GenericPrintConfig, RenderableTexture, RenderSurface},
};

use super::{FontCache, Printer, TextureBuilder, find_font, generic_update_cache};

/// A single full-color glyph for `std` targets.
///
/// The texture is a [`ril::Image<Rgba>`], supporting variable bits-per-pixel
/// palettes defined in the SPF colour table.
#[derive(Clone)]
pub struct AbstractCharacter {
    pub texture: Image<Rgba>,
    pub advance_x: u32,
}

impl Default for AbstractCharacter {
    fn default() -> Self {
        Self {
            texture: Image::new(1, 1, Rgba::transparent()),
            advance_x: 1,
        }
    }
}

impl RenderableTexture for AbstractCharacter {
    fn width(&self) -> u32 { self.texture.width() }
    fn height(&self) -> u32 { self.texture.height() }
    fn advance_x(&self) -> u32 { self.advance_x }
}

/// A full-color [`ril::Image<Rgba>`] acts as the rendering surface for the
/// std backend.
impl RenderSurface<AbstractCharacter> for Image<Rgba> {
    fn new(width: u32, height: u32) -> Self {
        Image::new(width, height, Rgba::transparent())
    }

    fn paste(&mut self, x: u32, y: u32, texture: &AbstractCharacter) {
        for py in 0..texture.texture.height() {
            for px in 0..texture.texture.width() {
                let pixel = texture.texture.pixel(px, py);
                if pixel.a == 0 { continue; }
                let (dst_x, dst_y) = (x + px, y + py);
                if dst_x < self.width() && dst_y < self.height() {
                    self.set_pixel(dst_x, dst_y, *pixel);
                }
            }
        }
    }
}

/// Builds [`AbstractCharacter`] glyphs from SPF pixmap data with full RGBA
/// colour support.
///
/// Uses [`bitvec`] with [`Lsb0`] ordering to unpack variable bits-per-pixel
/// values. SPF stores bits LSB-first and `Lsb0` matches natively — no manual
/// byte reversal needed (contrast with [`EmbeddedTextureBuilder`]).
/// Palette indices are resolved via the layout's colour table.
pub(crate) struct RgbaTextureBuilder;

impl TextureBuilder<AbstractCharacter> for RgbaTextureBuilder {
    fn build_texture(
        &self,
        character: &Character,
        pixmap: &Pixmap,
        pixmap_table: &PixmapTable,
        layout: &Layout,
    ) -> AbstractCharacter {
        let width = pixmap_table
            .constant_width
            .or(pixmap.custom_width)
            .expect("no width defined in pixmap or pixmap table");
        let height = pixmap_table
            .constant_height
            .or(pixmap.custom_height)
            .expect("no height defined in pixmap or pixmap table");

        // advance_x stored as u32 to match ril's native dimension type
        let advance_x = character.advance_x.unwrap_or(width) as u32;

        let color_table = &layout.color_tables[
            pixmap_table.color_table_indexes.as_ref().unwrap()[0] as usize
        ];
        let bits_per_pixel = pixmap_table
            .constant_bits_per_pixel
            .or(pixmap.custom_bits_per_pixel)
            .expect("no bits_per_pixel defined") as usize;

        // Lsb0 matches SPF's LSB-first bit storage — no reversal needed
        let pixels: Vec<Rgba> = pixmap
            .data
            .view_bits::<Lsb0>()
            .chunks(bits_per_pixel)
            .take(width as usize * height as usize)
            .map(|chunk| {
                let index = chunk.load_be::<u8>() as usize;
                let color = &color_table.colors[index];
                Rgba {
                    r: color.r,
                    g: color.g,
                    b: color.b,
                    a: color_table.constant_alpha.or(color.custom_alpha).unwrap(),
                }
            })
            .collect();

        AbstractCharacter {
            texture: Image::from_pixels(width as u32, pixels),
            advance_x,
        }
    }
}

/// Character cache for `std` targets.
///
/// Keyed by the full grapheme cluster [`String`], backed by [`HashMap`] for
/// O(1) average-case lookups. Supports multi-byte grapheme clusters (emoji,
/// combining characters) that a `u8` key cannot represent.
///
/// `max_height` is computed once during loading so [`FontCache::max_height`]
/// is O(1).
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
        self.max_height = self.max_height.max(glyph.height());
    }

    /// Populate the cache from a specific [`Font`] in the layout.
    ///
    /// `font_table` is the parent [`FontTable`] that contains `font`.
    pub fn update(&mut self, font_table: &FontTable, font: &Font, layout: &Layout) {
        generic_update_cache(
            font_table,
            font,
            layout,
            &RgbaTextureBuilder,
            |grapheme| grapheme.to_owned(),
            |key, glyph: AbstractCharacter| {
                self.track_height(&glyph);
                self.mappings.insert(key, glyph);
            },
        );
    }
}

impl FontCache for CharacterCacheImpl {
    type Key = String;
    type Glyph = AbstractCharacter;
    type Surface = Image<Rgba>;

    fn get(&self, key: &String) -> Option<&AbstractCharacter> {
        self.mappings.get(key)
    }

    fn max_height(&self) -> u32 { self.max_height }
}

/// A [`Printer`] pre-configured for the full-colour std backend.
///
/// Renders text onto a [`ril::Image<Rgba>`], keyed by grapheme cluster
/// [`String`]. Supports multi-byte Unicode characters.
///
/// # Example
/// ```ignore
/// // Discover available fonts
/// for name in font_names(&layout) {
///     println!("{}", name);
/// }
///
/// // Build a printer for a named font
/// let printer = RgbaPrinter::from_font_named("Regular", &layout, config)
///     .expect("font not found");
///
/// let image = printer.print_str("Hello, 世界");
/// image.save_inferred("output.png").unwrap();
/// ```
pub type RgbaPrinter = Printer<CharacterCacheImpl>;

impl RgbaPrinter {
    /// Build an [`RgbaPrinter`] from a specific [`Font`].
    ///
    /// `font_table` is the parent [`FontTable`] that contains `font`.
    pub fn from_font(
        font_table: &FontTable,
        font: &Font,
        layout: &Layout,
        config: GenericPrintConfig,
    ) -> Self {
        let mut cache = CharacterCacheImpl::new();
        cache.update(font_table, font, layout);
        Self::new(cache, config)
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

    /// Convenience: render a `&str` by splitting on char boundaries.
    ///
    /// Note: allocates one [`String`] per character plus an outer [`Vec`]
    /// on each call. For hot paths, prefer building the key slice manually
    /// and calling [`Printer::print`] directly.
    pub fn print_str(&self, text: &str) -> Image<Rgba> {
        let keys: Vec<String> = text.chars().map(|c| c.to_string()).collect();
        self.print(&keys)
    }
}