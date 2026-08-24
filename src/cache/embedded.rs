use spf::core::{Character, Font, FontTable, Layout, Pixmap, PixmapTable};

use crate::{
    print::{GenericPrintConfig, RenderSurface, RenderableTexture},
    shape::is_multi_cluster,
    utilities::compact_layout,
    Bitmap, BitmapU8, GlyphId, Trie, Vec,
};

use super::{find_font, generic_update_cache, FontCache, Printer, TextureBuilder};
use crate::color::ColorControl;

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
    fn width(&self) -> u32 {
        self.width as u32
    }
    fn height(&self) -> u32 {
        self.height as u32
    }
    fn advance_x(&self) -> u32 {
        self.advance_x as u32
    }
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

        AbstractCharacterU8 {
            width,
            height,
            advance_x,
            texture,
        }
    }
}

/// Character cache for embedded / `no_std` targets.
///
/// Glyphs are stored flat, indexed directly by [`GlyphId`]; [`Trie`] maps
/// codepoint sequences to that index — the same lookup mechanism as the
/// std backend. Index 0 is always a reserved blank glyph, [`shape`](crate::shape)'s
/// `notdef` fallback. `max_height` is computed once during loading so
/// [`FontCache::max_height`] is O(1).
#[derive(Clone, Default)]
pub struct CharacterCacheU8 {
    pub(crate) glyphs: Vec<AbstractCharacterU8>,
    pub(crate) trie: Trie,
    pub(crate) max_height: u32,
}

impl CharacterCacheU8 {
    pub fn new() -> Self {
        Self::default()
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

        // Reserve index 0 as the notdef fallback before any real glyph.
        self.glyphs.push(AbstractCharacterU8::default());

        let mut trie_entries: Vec<(crate::String, GlyphId, bool)> = Vec::new();

        generic_update_cache(
            font_table,
            font,
            layout,
            &EmbeddedTextureBuilder,
            &mut color_control,
            |code_points, glyph: AbstractCharacterU8| {
                self.track_height(&glyph);
                let id = GlyphId(self.glyphs.len() as u32);
                self.glyphs.push(glyph);
                if !code_points.is_empty() {
                    let is_ligature = is_multi_cluster(code_points);
                    trie_entries.push((code_points.into(), id, is_ligature));
                }
            },
        );

        let entry_refs: Vec<(&str, GlyphId, bool)> = trie_entries
            .iter()
            .map(|(key, id, is_ligature)| (key.as_str(), *id, *is_ligature))
            .collect();
        self.trie = Trie::build(&entry_refs);
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
        let mut abstract_characters =
            Vec::with_capacity(layout.character_tables[0].characters.len());
        for (character, pixmap) in layout.character_tables[0]
            .characters
            .iter()
            .zip(&pixmap_table.pixmaps)
        {
            let mut abstract_character = AbstractCharacterU8 {
                width: pixmap_table.constant_width.or(pixmap.custom_width).unwrap(),
                height: pixmap_table
                    .constant_height
                    .or(pixmap.custom_height)
                    .unwrap(),
                ..Default::default()
            };

            self.track_height(&abstract_character);
            abstract_character.advance_x = character.advance_x.unwrap_or(abstract_character.width);

            let mut bytes = pixmap
                .data
                .iter()
                .map(|b| b.reverse_bits())
                .collect::<Vec<u8>>();
            bytes.shrink_to_fit();

            let texture =
                BitmapU8::from_data(abstract_character.width, abstract_character.height, bytes)
                    .unwrap();
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

        // Single-byte, ASCII-only keys. Key_bytes
        // stays 1:1 aligned with abstract_characters by construction, which
        // GlyphId(i + 1) below relies on.
        let mut key_bytes: Vec<[u8; 1]> = Vec::with_capacity(character_table.characters.len());
        for character in character_table.characters.iter_mut() {
            let code_points = core::mem::take(&mut character.code_points);
            key_bytes.push([code_points.as_bytes()[0]]);
        }
        key_bytes.shrink_to_fit();
        abstract_characters.shrink_to_fit();

        self.glyphs = Vec::with_capacity(abstract_characters.len() + 1);
        self.glyphs.push(AbstractCharacterU8::default());
        self.glyphs.extend(abstract_characters);

        // A multi-byte UTF-8 lead byte can't stand alone as a valid str;
        // such characters are simply unreachable via the trie (they'd fall
        // through to notdef when shaped) rather than widening this path's
        // scope to full codepoint sequences.
        let entries: Vec<(&str, GlyphId, bool)> = key_bytes
            .iter()
            .enumerate()
            .filter_map(|(i, byte)| {
                core::str::from_utf8(byte)
                    .ok()
                    .map(|s| (s, GlyphId((i + 1) as u32), false))
            })
            .collect();
        self.trie = Trie::build(&entries);
    }
}

impl FontCache for CharacterCacheU8 {
    type Key = GlyphId;
    type Glyph = AbstractCharacterU8;
    type Surface = Bitmap;

    fn get(&self, key: &GlyphId) -> Option<&AbstractCharacterU8> {
        self.glyphs.get(key.0 as usize)
    }

    fn max_height(&self) -> u32 {
        self.max_height
    }
}

/// A [`Printer`] pre-configured for the embedded backend.
///
/// Renders text as a monochrome [`Bitmap`]. Fully `no_std` — no heap
/// beyond the glyph data itself.
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
    pub fn from_font_low_memory(layout: Layout, config: GenericPrintConfig) -> Self {
        let mut cache = CharacterCacheU8::new();
        cache.low_memory_zipped_update(layout);
        Self::new(cache, config)
    }

    /// Shape `text` against this printer's font without rendering it.
    /// Maximal munch, respecting
    /// [`self.config.allow_ligatures`](GenericPrintConfig).
    pub fn shape_str(&self, text: &str) -> crate::shape::Shaped {
        crate::shape::shape(
            text,
            &self.cache.trie,
            GlyphId(0),
            self.config.allow_ligatures,
        )
    }

    /// Convenience: shape and render `&str` directly. Shapes against the
    /// cache's [`Trie`] first (maximal munch, respecting
    /// [`self.config.allow_ligatures`](GenericPrintConfig)), then lays out
    /// and renders the resulting glyphs.
    pub fn print_str(&self, text: &str) -> Bitmap {
        let shaped = self.shape_str(text);
        self.print(&shaped.glyphs)
    }
}
