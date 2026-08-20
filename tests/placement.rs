//! Tests for `render_spf::layout` (Phase 2.1's extracted Placement pass)
//! and its consumers. Uses a synthetic in-memory `Layout` (spf::core's
//! structs are `#[non_exhaustive]` but derive `Default` with public
//! fields, so `Foo { field, ..Default::default() }` works from outside
//! the defining crate) instead of a bundled `.spf` fixture -- no file I/O,
//! full control over glyph dimensions for edge cases.

use render_spf::*;
use spf::core::*;

/// Three glyphs with deliberately different heights/widths/advances:
/// - "A": 5w x 8h, advance 6 (advance > width)
/// - "B": 4w x 5h, advance 4 (advance == width)
/// - "C": 3w x 10h, advance 2 (advance < width -- overlap-capable)
///
/// `cache.max_height()` across the whole font is 10 (from "C"), which is
/// taller than any run that excludes "C" -- the exact condition needed to
/// observe the vertical_align bug (offset_y collapses to 0 whenever
/// `natural_height == max_height`, which is only true when "C" is in the
/// run).
// spf::core's types are #[non_exhaustive] with public fields: struct-literal
// construction (even with ..Default::default()) is blocked from outside the
// crate, but Default::default() + field assignment on the resulting
// instance is not -- non_exhaustive only restricts the literal-construction
// syntax, not mutation of an already-constructed value.
fn build_test_layout() -> Layout {
    let glyphs: [(&str, u8, u8, u8); 3] = [("A", 5, 8, 6), ("B", 4, 5, 4), ("C", 3, 10, 2)];

    let pixmaps: Vec<Pixmap> = glyphs
        .iter()
        .map(|(_, w, h, _)| {
            let num_bytes = (*w as usize * *h as usize).div_ceil(8);
            let mut pixmap = Pixmap::default();
            pixmap.custom_width = Some(*w);
            pixmap.custom_height = Some(*h);
            pixmap.data = vec![0u8; num_bytes];
            pixmap
        })
        .collect();

    let characters: Vec<Character> = glyphs
        .iter()
        .map(|(name, _, _, adv)| {
            let mut character = Character::default();
            character.advance_x = Some(*adv);
            character.code_points = name.to_string();
            character
        })
        .collect();

    let mut color = Color::default();
    color.red = 255;
    color.green = 0;
    color.blue = 0;

    let mut color_table = ColorTable::default();
    color_table.colors = vec![color];

    let mut pixmap_table = PixmapTable::default();
    pixmap_table.constant_bits_per_pixel = Some(1);
    pixmap_table.color_table_indexes = Some(vec![0]);
    pixmap_table.pixmaps = pixmaps;

    let mut character_table = CharacterTable::default();
    character_table.pixmap_table_indexes = Some(vec![0]);
    character_table.characters = characters;

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

fn config(vertical_expand: bool, vertical_align: VerticalAlign) -> GenericPrintConfig {
    GenericPrintConfig {
        letter_spacing: 1,
        vertical_expand,
        vertical_align,
    }
}

// ---------------------------------------------------------------------
// Width / advance math, and vertical_expand=false (bug-unaffected paths)
// ---------------------------------------------------------------------

#[test]
fn empty_keys_yield_empty_placement() {
    let layout = build_test_layout();
    let printer = RgbaPrinter::from_font_named("Test", &layout, config(true, VerticalAlign::Middle))
        .unwrap();
    let placement = render_spf::layout(&[], &printer.config, &printer.cache);
    assert_eq!(placement.width, 0);
    assert_eq!(placement.height, 0);
    assert!(placement.glyphs.is_empty());
}

#[test]
fn width_and_advance_math() {
    let layout = build_test_layout();
    let printer = RgbaPrinter::from_font_named("Test", &layout, config(false, VerticalAlign::Top))
        .unwrap();
    let keys = vec!["A".to_string(), "B".to_string()];
    let placement = render_spf::layout(&keys, &printer.config, &printer.cache);

    // last glyph contributes its own width, not advance_x: 6 (A's advance)
    // + 1 (letter_spacing) + 4 (B's width, since B is last) = 11.
    assert_eq!(placement.width, 6 + 1 + 4);
    assert_eq!(placement.glyphs.len(), 2);
    assert_eq!(placement.glyphs[0].dst_x, 0);
    assert_eq!(placement.glyphs[0].advance_x, 6);
    assert_eq!(placement.glyphs[1].dst_x, 6 + 1);
}

#[test]
fn vertical_expand_false_uses_natural_height_and_zero_offset() {
    let layout = build_test_layout();
    // "A" (h=8) and "B" (h=5): natural height is 8, unaffected by C's
    // taller cached glyph since vertical_expand is off.
    for align in [VerticalAlign::Top, VerticalAlign::Middle, VerticalAlign::Bottom] {
        let printer =
            RgbaPrinter::from_font_named("Test", &layout, config(false, align)).unwrap();
        let keys = vec!["A".to_string(), "B".to_string()];
        let placement = render_spf::layout(&keys, &printer.config, &printer.cache);
        assert_eq!(placement.height, 8);
        for glyph in &placement.glyphs {
            assert_eq!(glyph.dst_y, 0);
        }
    }
}

#[test]
fn vertical_expand_true_top_align_is_always_zero_offset() {
    let layout = build_test_layout();
    let printer =
        RgbaPrinter::from_font_named("Test", &layout, config(true, VerticalAlign::Top)).unwrap();
    let keys = vec!["A".to_string(), "B".to_string()];
    let placement = render_spf::layout(&keys, &printer.config, &printer.cache);
    assert_eq!(placement.height, 10); // cache.max_height(), from "C"
    for glyph in &placement.glyphs {
        assert_eq!(glyph.dst_y, 0);
    }
}

// ---------------------------------------------------------------------
// Known bug (documented, not yet fixed): Middle/Bottom + vertical_expand
// always yield offset_y = 0, even when the run's natural height is
// shorter than the surface (cache.max_height()). Fixed in Phase 2.2 --
// this test is replaced there with the corrected expectation.
// ---------------------------------------------------------------------

#[test]
fn known_bug_vertical_align_middle_bottom_collapse_to_zero_offset() {
    let layout = build_test_layout();
    // "A","B" only: natural height 8, surface height (max_height) 10.
    // Correct Middle offset would be (10-8)/2=1, Bottom would be 10-8=2.
    // The current implementation yields 0 for both.
    for align in [VerticalAlign::Middle, VerticalAlign::Bottom] {
        let printer =
            RgbaPrinter::from_font_named("Test", &layout, config(true, align)).unwrap();
        let keys = vec!["A".to_string(), "B".to_string()];
        let placement = render_spf::layout(&keys, &printer.config, &printer.cache);
        assert_eq!(placement.height, 10);
        for glyph in &placement.glyphs {
            assert_eq!(glyph.dst_y, 0, "bug: offset_y should be wrong (0) pre-Phase-2.2");
        }
    }
}

// ---------------------------------------------------------------------
// Placement/layout math is identical across backends (std vs embedded) --
// both now go through the same shared `layout` function.
// ---------------------------------------------------------------------

#[test]
fn std_and_embedded_backends_agree_on_placement() {
    let layout_data = build_test_layout();
    let rgba = RgbaPrinter::from_font_named("Test", &layout_data, config(true, VerticalAlign::Top))
        .unwrap();
    let embedded =
        EmbeddedPrinter::from_font_named("Test", &layout_data, config(true, VerticalAlign::Top))
            .unwrap();

    let rgba_keys = vec!["A".to_string(), "B".to_string(), "C".to_string()];
    let embedded_keys: Vec<u8> = vec![b'A', b'B', b'C'];

    let rgba_placement = render_spf::layout(&rgba_keys, &rgba.config, &rgba.cache);
    let embedded_placement = render_spf::layout(&embedded_keys, &embedded.config, &embedded.cache);

    assert_eq!(rgba_placement.width, embedded_placement.width);
    assert_eq!(rgba_placement.height, embedded_placement.height);
    for (a, b) in rgba_placement
        .glyphs
        .iter()
        .zip(embedded_placement.glyphs.iter())
    {
        assert_eq!(a.dst_x, b.dst_x);
        assert_eq!(a.dst_y, b.dst_y);
        assert_eq!(a.width, b.width);
        assert_eq!(a.height, b.height);
        assert_eq!(a.advance_x, b.advance_x);
    }
}

// ---------------------------------------------------------------------
// End-to-end sanity: RgbaPrinter::render produces a surface matching the
// Placement it was computed from.
// ---------------------------------------------------------------------

#[test]
fn render_surface_matches_placement_dimensions() {
    let layout_data = build_test_layout();
    let printer =
        RgbaPrinter::from_font_named("Test", &layout_data, config(true, VerticalAlign::Top))
            .unwrap();
    let keys = vec!["A".to_string(), "B".to_string()];
    let placement = render_spf::layout(&keys, &printer.config, &printer.cache);
    let image = printer.render(&keys);

    assert_eq!(image.width(), placement.width);
    assert_eq!(image.height(), placement.height);
}
