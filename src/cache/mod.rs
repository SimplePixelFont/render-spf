use spf::core::{Character, Font, FontTable, Layout, Pixmap, PixmapTable};

use crate::{
    Vec,
    print::{GenericPrintConfig, RenderableTexture, RenderSurface, generic_print},
};

mod embedded;
pub use embedded::*;

#[cfg(feature = "std")]
mod full;
#[cfg(feature = "std")]
pub use full::*;

pub trait FontCache {
    type Key;
    type Glyph: RenderableTexture;
    type Surface: RenderSurface<Self::Glyph>;

    fn get(&self, key: &Self::Key) -> Option<&Self::Glyph>;
    fn max_height(&self) -> u32;
}

pub struct Printer<C: FontCache> {
    pub cache: C,
    pub config: GenericPrintConfig,
}

impl<C: FontCache> Printer<C> {
    pub fn new(cache: C, config: GenericPrintConfig) -> Self {
        Self { cache, config }
    }

    /// Rasterise a sequence of character keys onto a new surface.
    pub fn print(&self, keys: &[C::Key]) -> C::Surface {
        generic_print(keys, &self.config, &self.cache)
    }
}

/// Returns the names of every [`Font`] in the layout, in table/font order.
///
/// Useful for presenting a list of available fonts to the user before
/// calling [`find_font`] or a `from_font_named` constructor.
///
/// # Example
/// ```ignore
/// for name in font_names(&layout) {
///     println!("{}", name);
/// }
/// ```
pub fn font_names(layout: &Layout) -> Vec<&str> {
    layout
        .font_tables
        .iter()
        .flat_map(|table| table.fonts.iter().map(|f| f.name.as_str()))
        .collect()
}

/// Find the first [`Font`] matching `name` across all font tables, returning
/// both the parent [`FontTable`] and the [`Font`].
///
/// Returns `None` if no font with that name exists in the layout.
pub fn find_font<'a>(layout: &'a Layout, name: &str) -> Option<(&'a FontTable, &'a Font)> {
    layout.font_tables.iter().find_map(|table| {
        table
            .fonts
            .iter()
            .find(|f| f.name == name)
            .map(|font| (table, font))
    })
}

/// Converts raw SPF pixmap data into a concrete glyph type `T`.
pub trait TextureBuilder<T: RenderableTexture> {
    fn build_texture(
        &self,
        character: &Character,
        pixmap: &Pixmap,
        pixmap_table: &PixmapTable,
        layout: &Layout,
    ) -> T;
}

/// Search `tables` for the pixmap at `index`, returning the owning
/// [`PixmapTable`] alongside the [`Pixmap`].
///
/// The table is returned because it carries `constant_width`,
/// `constant_height`, and `constant_bits_per_pixel` which are needed to
/// interpret the pixmap data.
///
/// Searches tables in order, returning the first table that contains a
/// pixmap at `index`. This mirrors the SPF fallback semantics where a
/// character may reference a pixmap by index across multiple dependency
/// tables.
pub(crate) fn resolve_pixmap<'a>(
    index: usize,
    tables: &[&'a PixmapTable],
) -> Option<(&'a PixmapTable, &'a Pixmap)> {
    tables.iter().find_map(|table| {
        (index < table.pixmaps.len()).then(|| (*table, &table.pixmaps[index]))
    })
}

/// Single authoritative cache-population loop used by both backends.
///
/// # Resolution chain
///
/// ```text
/// font.character_table_indexes[i]
///   → font_table.character_table_indexes[i]   (font-local → table-local)
///     → layout.character_tables[i]             (table-local → layout)
///       → dependency pixmap tables
///         → four-way pixmap resolution
///           → build_texture → inserter
/// ```
///
/// ## Four-way pixmap resolution
///
/// SPF characters carry two optional override flags:
///
/// | `use_pixmap_table_index` | `use_pixmap_index` | Resolution |
/// |---|---|---|
/// | true  | true  | explicit table + explicit pixmap index |
/// | true  | false | explicit table + positional index |
/// | false | true  | explicit pixmap index, search all dep tables |
/// | false | false | fully implicit — both by position |
pub fn generic_update_cache<K, T, B>(
    font_table: &FontTable,
    font: &Font,
    layout: &Layout,
    builder: &B,
    key_converter: impl Fn(&str) -> K,
    mut inserter: impl FnMut(K, T),
) where
    T: RenderableTexture,
    B: TextureBuilder<T>,
{
    // Resolve character tables for this font via double indirection:
    //   font.character_table_indexes[i]
    //     → font_table.character_table_indexes[i]  (table-local index)
    //       → layout.character_tables[i]            (layout index)
    let font_table_char_indexes = match font_table.character_table_indexes.as_ref() {
        Some(v) => v,
        None => return, // font table has no character table mappings
    };

    for font_local_idx in &font.character_table_indexes {
        // Step 1: font-local → table-local
        let table_local_idx = match font_table_char_indexes.get(*font_local_idx as usize) {
            Some(i) => *i as usize,
            None => continue,
        };

        // Step 2: table-local → layout
        let character_table = match layout.character_tables.get(table_local_idx) {
            Some(t) => t,
            None => continue,
        };

        // Collect the dependency pixmap tables for this character table
        let dep_pixmap_tables: Vec<&PixmapTable> = character_table
            .pixmap_table_indexes
            .as_ref()
            .map(|indexes| {
                indexes
                    .iter()
                    .filter_map(|i| layout.pixmap_tables.get(*i as usize))
                    .collect()
            })
            .unwrap_or_default();

        for (pos_index, character) in character_table.characters.iter().enumerate() {
            // Four-way pixmap resolution
            let resolved = if character_table.use_pixmap_table_index
                && character_table.use_pixmap_index
            {
                // Both explicit — character names both its table and pixmap
                let pix_idx = character.pixmap_index.unwrap() as usize;
                let tbl_idx = character.pixmap_table_index.unwrap() as usize;
                dep_pixmap_tables
                    .get(tbl_idx)
                    .and_then(|t| (pix_idx < t.pixmaps.len()).then(|| (*t, &t.pixmaps[pix_idx])))
            } else if character_table.use_pixmap_table_index {
                // Explicit table, positional pixmap
                let tbl_idx = character.pixmap_table_index.unwrap() as usize;
                dep_pixmap_tables
                    .get(tbl_idx)
                    .and_then(|t| resolve_pixmap(pos_index, &[t]))
            } else if character_table.use_pixmap_index {
                // Explicit pixmap index, search across all dep tables
                let pix_idx = character.pixmap_index.unwrap() as usize;
                resolve_pixmap(pix_idx, &dep_pixmap_tables)
            } else {
                // Fully implicit — both by position
                resolve_pixmap(pos_index, &dep_pixmap_tables)
            };

            if let Some((pixmap_table, pixmap)) = resolved {
                let texture =
                    builder.build_texture(character, pixmap, pixmap_table, layout);
                let key = key_converter(&character.grapheme_cluster);
                inserter(key, texture);
            }
        }
    }
}