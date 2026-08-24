//! End-to-end check that live `ColorControl` edits actually show up in `RgbaPrinter::render()` output across
//! repeated calls on the same printer -- not just on the first render.
#![cfg(feature = "std")] // RgbaPrinter/ril are std-only

use render_spf::*;
use spf::core::*;

/// A single 1x1 glyph "A", bits_per_pixel=1, its one pixel set to palette
/// index 1 (bit = 1) in a 2-entry color table (red at 0, green at 1).
fn build_test_layout() -> Layout {
    let mut pixmap = Pixmap::default();
    pixmap.custom_width = Some(1);
    pixmap.custom_height = Some(1);
    pixmap.data = vec![0b0000_0001]; // SPF pixmap data is Lsb0 -- first pixel is bit 0

    let mut character = Character::default();
    character.advance_x = Some(1);
    character.code_points = "A".to_string();

    let mut red = Color::default();
    red.red = 255;
    let mut green = Color::default();
    green.green = 255;

    let mut color_table = ColorTable::default();
    color_table.colors = vec![red, green];

    let mut pixmap_table = PixmapTable::default();
    pixmap_table.constant_bits_per_pixel = Some(1);
    pixmap_table.color_table_indexes = Some(vec![0]);
    pixmap_table.pixmaps = vec![pixmap];

    let mut character_table = CharacterTable::default();
    character_table.pixmap_table_indexes = Some(vec![0]);
    character_table.characters = vec![character];

    let mut font = Font::default();
    font.name = "Test".to_string();
    font.linked_character_table_indexes = vec![0];

    let mut font_table = FontTable::default();
    font_table.character_table_indexes = Some(vec![0]);
    font_table.fonts = vec![font];

    let mut layout = Layout::default();
    layout.character_tables = vec![character_table];
    layout.color_tables = vec![color_table];
    layout.pixmap_tables = vec![pixmap_table];
    layout.font_tables = vec![font_table];
    layout
}

fn pixel_at(image: &ril::Image<ril::Rgba>, x: u32, y: u32) -> (u8, u8, u8, u8) {
    let p = image
        .pixels()
        .nth(y as usize)
        .unwrap()
        .get(x as usize)
        .unwrap();
    (p.r, p.g, p.b, p.a)
}

#[test]
fn live_color_edits_are_reflected_across_repeated_renders() {
    let layout = build_test_layout();
    let config = GenericPrintConfig {
        letter_spacing: 1,
        vertical_expand: false,
        vertical_align: VerticalAlign::Top,
        allow_ligatures: true,
        padding_left: 0,
        padding_top: 0,
        padding_right: 0,
        padding_bottom: 0,
    };
    let mut printer = RgbaPrinter::from_font_named("Test", &layout, config).unwrap();
    let keys = printer.shape_str("A").glyphs;

    // Original: palette index 1 is green.
    let image = printer.render(&keys);
    assert_eq!(pixel_at(&image, 0, 0), (0, 255, 0, 255));

    // Live edit, then a second render on the same printer must pick it up.
    printer.colors.set(0, 1, 0, 0, 255, 255);
    let image = printer.render(&keys);
    assert_eq!(pixel_at(&image, 0, 0), (0, 0, 255, 255));

    // A third render with no further mutation stays stable.
    let image = printer.render(&keys);
    assert_eq!(pixel_at(&image, 0, 0), (0, 0, 255, 255));

    // Reset reverts to the original SPF-defined color.
    printer.colors.reset(0, 1);
    let image = printer.render(&keys);
    assert_eq!(pixel_at(&image, 0, 0), (0, 255, 0, 255));
}
