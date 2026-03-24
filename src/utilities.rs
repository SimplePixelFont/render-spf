use spf::core::Layout;

/// Calls `shrink_to_fit` on [`Vec`]s and multiple fields inside a [`Layout`], reclaiming
/// allocator overhead before the layout is processed. Used by
/// [`crate::cache::CharacterCacheU8::low_memory_zipped_update`].
pub fn compact_layout(layout: &mut Layout) {
    for table in &mut layout.character_tables {
        for character in &mut table.characters {
            character.grapheme_cluster.shrink_to_fit();
        }
    }

    for table in &mut layout.pixmap_tables {
        for pixmap in &mut table.pixmaps {
            pixmap.data.shrink_to_fit();
        }
    }

    for table in &mut layout.color_tables {
        table.colors.shrink_to_fit();
    }

    for table in &mut layout.font_tables {
        for font in &mut table.fonts {
            font.name.shrink_to_fit();
            font.author.shrink_to_fit();
            font.character_table_indexes.shrink_to_fit();
        }
    }

    layout.character_tables.shrink_to_fit();
    layout.pixmap_tables.shrink_to_fit();
    layout.color_tables.shrink_to_fit();
    layout.font_tables.shrink_to_fit();
}
