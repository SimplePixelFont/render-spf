use spf::core::{Character, Font, FontTable, Layout, Pixmap, PixmapTable};

use crate::{
    Vec,
    color::{ColorControl, ColorEntry, ColorType},
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

    pub fn print(&self, keys: &[C::Key]) -> C::Surface {
        generic_print(keys, &self.config, &self.cache)
    }
}

/// Returns the names of every [`Font`] in the layout, in table/font order.
///
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

/// Find the first [`Font`] matching `name` across all font tables.
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
///
/// Color table population is handled separately by `generic_update_cache`
/// via [`populate_color_control`] — builders only need to produce the glyph.
pub trait TextureBuilder<T: RenderableTexture> {
    fn build_texture(
        &self,
        character: &Character,
        pixmap: &Pixmap,
        pixmap_table: &PixmapTable,
        layout: &Layout,
    ) -> T;
}

/// Search `tables` for the pixmap at `index`.
/// First table with a valid entry at `index` wins.
pub(crate) fn resolve_pixmap<'a>(
    index: usize,
    tables: &[&'a PixmapTable],
) -> Option<(&'a PixmapTable, &'a Pixmap)> {
    tables.iter().find_map(|table| {
        (index < table.pixmaps.len()).then(|| (*table, &table.pixmaps[index]))
    })
}

/// Fill [`ColorControl`] slots for each color table referenced by `pixmap_table`.
///
/// Slots are keyed by **layout-level** color table index — the same index
/// used in `layout.color_tables` — so resolution is an unambiguous direct
/// lookup regardless of which pixmap table a glyph came from.
///
/// A slot is only written once: if it is already populated (non-empty),
/// it is left unchanged. This is safe because layout-level indexes are
/// globally unique, so two pixmap tables referencing the same color table
/// will write identical data to the same slot.
pub(crate) fn populate_color_control(
    color_control: &mut ColorControl,
    pixmap_table: &PixmapTable,
    layout: &Layout,
) {
    let color_table_indexes = match pixmap_table.color_table_indexes.as_ref() {
        Some(v) => v,
        None => return,
    };

    for &layout_ct_idx in color_table_indexes {
        let slot = layout_ct_idx as usize;

        // Already populated — two pixmap tables referencing the same layout
        // color table write identical data, so first write wins.
        if color_control.tables.get(slot).map(|t| !t.is_empty()).unwrap_or(false) {
            continue;
        }

        if let Some(color_table) = layout.color_tables.get(slot) {
            let entries: Vec<ColorEntry> = color_table
                .colors
                .iter()
                .map(|color| {
                    let alpha = color_table
                        .constant_alpha
                        .or(color.custom_alpha)
                        .unwrap_or(255);
                    let color_type = match color.color_type {
                        Some(spf::core::ColorType::Absolute) => ColorType::Absolute,
                        _ => ColorType::Dynamic,
                    };
                    ColorEntry::new(color_type, color.r, color.g, color.b, alpha)
                })
                .collect();

            if let Some(slot_ref) = color_control.tables.get_mut(slot) {
                *slot_ref = entries;
            }
        }
    }
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
///         → four-way pixmap resolution → Pixmap
///           → populate_color_control (by layout-level index)
///           → build_texture → glyph
///             → inserter
/// ```
///
/// `color_control` is pre-sized to `layout.color_tables.len()` by the
/// caller so every slot is addressable by layout-level index from the start.
///
/// ## Four-way pixmap resolution
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
    color_control: &mut ColorControl,
    key_converter: impl Fn(&str) -> K,
    mut inserter: impl FnMut(K, T),
) where
    T: RenderableTexture,
    B: TextureBuilder<T>,
{
    let font_table_char_indexes = match font_table.character_table_indexes.as_ref() {
        Some(v) => v,
        None => return,
    };

    for font_local_idx in &font.character_table_indexes {
        // Double indirection: font-local → table-local → layout
        let table_local_idx = match font_table_char_indexes.get(*font_local_idx as usize) {
            Some(i) => *i as usize,
            None => continue,
        };
        let character_table = match layout.character_tables.get(table_local_idx) {
            Some(t) => t,
            None => continue,
        };

        // Collect dependency pixmap tables for this character table
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
                let pix_idx = character.pixmap_index.unwrap() as usize;
                let tbl_idx = character.pixmap_table_index.unwrap() as usize;
                dep_pixmap_tables.get(tbl_idx).and_then(|t| {
                    (pix_idx < t.pixmaps.len()).then(|| (*t, &t.pixmaps[pix_idx]))
                })
            } else if character_table.use_pixmap_table_index {
                let tbl_idx = character.pixmap_table_index.unwrap() as usize;
                dep_pixmap_tables
                    .get(tbl_idx)
                    .and_then(|t| resolve_pixmap(pos_index, &[t]))
            } else if character_table.use_pixmap_index {
                let pix_idx = character.pixmap_index.unwrap() as usize;
                resolve_pixmap(pix_idx, &dep_pixmap_tables)
            } else {
                resolve_pixmap(pos_index, &dep_pixmap_tables)
            };

            if let Some((pixmap_table, pixmap)) = resolved {
                populate_color_control(color_control, pixmap_table, layout);

                let texture = builder.build_texture(character, pixmap, pixmap_table, layout);
                let key = key_converter(&character.grapheme_cluster);
                inserter(key, texture);
            }
        }
    }
}