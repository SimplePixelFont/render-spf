use spf::core::{Character, Font, FontTable, Layout, Pixmap, PixmapTable};

use crate::{
    Vec,
    Bitmap, BitmapU8, VecMap,
    print::{GenericPrintConfig, RenderableTexture, RenderSurface},
    utilities::compact_layout,
};
 
use crate::color::ColorControl;
use super::{
    FontCache, Printer, TextureBuilder,
    find_font, generic_update_cache,
};

/// A single glyph for embedded / `no_std` targets.
///
/// Dimensions are stored as `u8` to minimise per-glyph overhead. The texture
/// is a tightly-packed 1-bit [`BitmapU8`], keeping storage proportional to
/// pixel count rather than byte-aligned row width.
#[derive(Clone, Debug)]
pub struct AbstractCharacterU8 {
    pub width: u8,
    pub height: u8,
    pub advance_x: u8,
    pub texture: BitmapU8,
}

impl Default for AbstractCharacterU8 {
    fn default() -> Self {
        // All fields consistent with the 1×1 texture.
        Self {
            width: 1,
            height: 1,
            advance_x: 1,
            texture: BitmapU8::new(1, 1),
        }
    }
}

impl RenderableTexture for AbstractCharacterU8 {
    // Stored as u8, widened to u32 at the trait boundary.
    fn width(&self) -> u32 { self.width as u32 }
    fn height(&self) -> u32 { self.height as u32 }
    fn advance_x(&self) -> u32 { self.advance_x as u32 }
}

/// [`Bitmap`] is the rendering surface for the embedded backend.
///
/// Coordinates from [`generic_print`](crate::print::generic_print) arrive as
/// `u32` and are cast to `isize` for [`Bitmap::paste_u8`], which clips
/// out-of-bounds pixels internally.
impl RenderSurface<AbstractCharacterU8> for Bitmap {
    fn new(width: u32, height: u32) -> Self {
        Bitmap::new(width as usize, height as usize)
    }

    fn paste(&mut self, x: u32, y: u32, texture: &AbstractCharacterU8) {
        self.paste_u8(&texture.texture, x as isize, y as isize);
    }
}

/// Builds [`AbstractCharacterU8`] glyphs from raw SPF pixmap data.
///
/// SPF stores bits **LSB-first**; [`BitmapU8`] expects **MSB-first**.
/// [`u8::reverse_bits`] bridges this with no extra dependencies, keeping
/// the embedded path fully `no_std`.
pub(crate) struct EmbeddedTextureBuilder;

impl TextureBuilder<AbstractCharacterU8> for EmbeddedTextureBuilder {
    fn build_texture(
        &self,
        character: &Character,
        pixmap: &Pixmap,
        pixmap_table: &PixmapTable,
        _layout: &Layout,
    ) -> AbstractCharacterU8 {
        let width = pixmap_table
            .constant_width
            .or(pixmap.custom_width)
            .expect("no width defined in pixmap or pixmap table");
        let height = pixmap_table
            .constant_height
            .or(pixmap.custom_height)
            .expect("no height defined in pixmap or pixmap table");
        let advance_x = character.advance_x.unwrap_or(width);

        // SPF is LSB-first; reverse each byte to produce MSB-first BitmapU8 data
        let bytes: Vec<u8> = pixmap.data.iter().map(|b| b.reverse_bits()).collect();

        let texture = BitmapU8::from_data(width, height, bytes)
            .expect("pixmap data length does not match declared dimensions");

        AbstractCharacterU8 { width, height, advance_x, texture }
    }
}

/// Character cache for embedded / `no_std` targets.
///
/// Keyed by the first byte of each grapheme cluster (`u8`), backed by a
/// [`VecMap`] for allocation-minimal O(n) lookups. `max_height` is computed
/// once during loading so [`FontCache::max_height`] is O(1).
#[derive(Clone, Default)]
pub struct CharacterCacheU8 {
    pub(crate) mappings: VecMap<u8, AbstractCharacterU8>,
    pub(crate) max_height: u32,
}

impl CharacterCacheU8 {
    pub const fn new() -> Self {
        Self {
            mappings: VecMap::new(),
            max_height: 0,
        }
    }

    pub fn track_height(&mut self, glyph: &AbstractCharacterU8) {
        self.max_height = self.max_height.max(glyph.height as u32);
    }

    /// Populate the cache from a specific [`Font`] in the layout.
    ///
    /// `font_table` is the parent [`FontTable`] that owns `font` — needed
    /// for the double-indirection resolution of character table indexes.
    pub fn update(&mut self, font_table: &FontTable, font: &Font, layout: &Layout) {
        // The embedded backend is monochrome — ColorControl is constructed
        // to satisfy generic_update_cache's signature but immediately dropped.
        let mut color_control = ColorControl::with_capacity(layout.color_tables.len());
        generic_update_cache(
            font_table,
            font,
            layout,
            &EmbeddedTextureBuilder,
            &mut color_control,
            |grapheme| *grapheme.as_bytes().first().unwrap_or(&0),
            |key, glyph: AbstractCharacterU8| {
                self.track_height(&glyph);
                self.mappings.insert(key, glyph);
            },
        );
    }

    /// Memory-optimised update path for severely constrained targets.
    ///
    /// Drops and shrinks layout internals eagerly as they are consumed to
    /// minimise peak heap usage. The source [`Layout`] is taken by value
    /// and freed in full before this method returns.
    ///
    /// This method will update the cache with characters and pixmaps only
    /// from the first table in the layout's `character_tables` and 
    /// `pixmap_tables`, zipping them together.
    ///
    /// Use instead of [`update`](Self::update) when heap is tight.
    pub fn low_memory_zipped_update(&mut self, mut layout: Layout) {
        layout.font_tables.clear();
        compact_layout(&mut layout);

        let pixmap_table = &layout.pixmap_tables[0];
        let mut abstract_characters = Vec::with_capacity(layout.character_tables[0].characters.len());
        for (character, pixmap) in 
            layout.character_tables[0].characters.iter().zip(&pixmap_table.pixmaps) {
            let mut abstract_character = AbstractCharacterU8::default();

            abstract_character.width = pixmap_table.constant_width.or(pixmap.custom_width).unwrap();
            abstract_character.height = pixmap_table.constant_height.or(pixmap.custom_height).unwrap();
            self.track_height(&abstract_character);
            abstract_character.advance_x = character
                .advance_x
                .or(Some(abstract_character.width))
                .unwrap();

            let mut bytes = pixmap.data.iter().map(|b| b.reverse_bits()).collect::<Vec<u8>>();
            bytes.shrink_to_fit();
            
            let texture = BitmapU8::from_data(abstract_character.width.into(), abstract_character.height.into(), bytes).unwrap();
            abstract_character.texture = texture;

            abstract_characters.push(abstract_character);
        }
        layout.color_tables.clear();
        layout.pixmap_tables.clear();
        layout.color_tables.shrink_to_fit();
        layout.pixmap_tables.shrink_to_fit();

        let mut character_table = core::mem::take(&mut layout.character_tables[0]);
        layout.character_tables.clear();
        layout.character_tables.shrink_to_fit();
        drop(layout);

        let mut characters = vec![];
        for character in character_table.characters.iter_mut() {
            let character = core::mem::take(&mut character.grapheme_cluster);
            characters.push(character.as_bytes()[0]);
        }

        characters.shrink_to_fit();
        abstract_characters.shrink_to_fit();

        self.mappings = VecMap { keys: characters, values: abstract_characters };
    }
}

impl FontCache for CharacterCacheU8 {
    type Key = u8;
    type Glyph = AbstractCharacterU8;
    type Surface = Bitmap;

    fn get(&self, key: &u8) -> Option<&AbstractCharacterU8> {
        self.mappings.get(key)
    }

    fn max_height(&self) -> u32 { self.max_height }
}

/// A [`Printer`] pre-configured for the embedded backend.
///
/// Renders text as a monochrome [`Bitmap`], keyed by ASCII byte (`u8`).
/// Fully `no_std` — no heap beyond the glyph data itself.
///
/// # Example
/// ```ignore
/// // Discover available fonts
/// for name in font_names(&layout) {
///     println!("{}", name);
/// }
///
/// // Build a printer for a named font
/// let printer = EmbeddedPrinter::from_font_named("Regular", &layout, config)
///     .expect("font not found");
///
/// let bitmap = printer.print_str("Hello!");
/// send_to_display(bitmap.data());
/// ```
pub type EmbeddedPrinter = Printer<CharacterCacheU8>;

impl EmbeddedPrinter {
    /// Build an [`EmbeddedPrinter`] from a specific [`Font`].
    ///
    /// `font_table` is the parent [`FontTable`] that contains `font`.
    pub fn from_font(
        font_table: &FontTable,
        font: &Font,
        layout: &Layout,
        config: GenericPrintConfig,
    ) -> Self {
        let mut cache = CharacterCacheU8::new();
        cache.update(font_table, font, layout);
        Self::new(cache, config)
    }

    /// Build an [`EmbeddedPrinter`] by searching for a font by name.
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

    /// Build an [`EmbeddedPrinter`] consuming the layout to minimise peak
    /// heap usage. Selects the font by table/font index.
    ///
    /// Use instead of [`from_font`](Self::from_font) when heap is tight.
    pub fn from_font_low_memory(
        layout: Layout,
        config: GenericPrintConfig,
    ) -> Self {
        let mut cache = CharacterCacheU8::new();
        cache.low_memory_zipped_update(layout);
        Self::new(cache, config)
    }

    /// Convenience: render a `&str` directly without converting to `&[u8]`.
    pub fn print_str(&self, text: &str) -> Bitmap {
        self.print(text.as_bytes())
    }
}