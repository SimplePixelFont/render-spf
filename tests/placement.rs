//! Tests for `render_spf::layout` (Phase 2.1's extracted Placement pass)
//! and its consumers. Uses a synthetic in-memory `Layout` (spf::core's
//! structs are `#[non_exhaustive]` but derive `Default` with public
//! fields, so `Foo { field, ..Default::default() }` works from outside
//! the defining crate) instead of a bundled `.spf` fixture -- no file I/O,
//! full control over glyph dimensions for edge cases.
#![cfg(feature = "std")] // RgbaPrinter/ril are std-only

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
        allow_ligatures: true,
        padding_left: 0,
        padding_top: 0,
        padding_right: 0,
        padding_bottom: 0,
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
    let keys = printer.shape_str("AB").glyphs;
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
        let keys = printer.shape_str("AB").glyphs;
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
    let keys = printer.shape_str("AB").glyphs;
    let placement = render_spf::layout(&keys, &printer.config, &printer.cache);
    assert_eq!(placement.height, 10); // cache.max_height(), from "C"
    for glyph in &placement.glyphs {
        assert_eq!(glyph.dst_y, 0);
    }
}

// ---------------------------------------------------------------------
// vertical_align Middle/Bottom (Phase 2.2 fix): offset_y is computed from
// the run's own natural height, not from the (possibly taller) surface
// height, so a short run within a taller cached font is actually shifted.
// ---------------------------------------------------------------------

#[test]
fn vertical_align_middle_offsets_by_half_the_height_difference() {
    let layout = build_test_layout();
    // "A","B" only: natural height 8, surface height (max_height) 10.
    let printer =
        RgbaPrinter::from_font_named("Test", &layout, config(true, VerticalAlign::Middle))
            .unwrap();
    let keys = printer.shape_str("AB").glyphs;
    let placement = render_spf::layout(&keys, &printer.config, &printer.cache);
    assert_eq!(placement.height, 10);
    for glyph in &placement.glyphs {
        assert_eq!(glyph.dst_y, 1); // (10 - 8) / 2
    }
}

#[test]
fn vertical_align_bottom_offsets_by_the_full_height_difference() {
    let layout = build_test_layout();
    // "A","B" only: natural height 8, surface height (max_height) 10.
    let printer =
        RgbaPrinter::from_font_named("Test", &layout, config(true, VerticalAlign::Bottom))
            .unwrap();
    let keys = printer.shape_str("AB").glyphs;
    let placement = render_spf::layout(&keys, &printer.config, &printer.cache);
    assert_eq!(placement.height, 10);
    for glyph in &placement.glyphs {
        assert_eq!(glyph.dst_y, 2); // 10 - 8
    }
}

#[test]
fn vertical_align_middle_bottom_agree_with_top_when_run_reaches_max_height() {
    let layout = build_test_layout();
    // Including "C" (height 10) makes natural_height == cache.max_height(),
    // so every alignment should agree: offset_y = 0.
    for align in [VerticalAlign::Top, VerticalAlign::Middle, VerticalAlign::Bottom] {
        let printer =
            RgbaPrinter::from_font_named("Test", &layout, config(true, align)).unwrap();
        let keys = printer.shape_str("AC").glyphs;
        let placement = render_spf::layout(&keys, &printer.config, &printer.cache);
        assert_eq!(placement.height, 10);
        for glyph in &placement.glyphs {
            assert_eq!(glyph.dst_y, 0);
        }
    }
}

// ---------------------------------------------------------------------
// Padding: blank margin around the whole run, outside the text's own
// bounding box -- distinct from letter_spacing (between glyphs).
// ---------------------------------------------------------------------

#[test]
fn padding_grows_the_surface_and_shifts_every_glyph() {
    let layout = build_test_layout();
    let mut cfg = config(false, VerticalAlign::Top);
    cfg.padding_left = 3;
    cfg.padding_top = 2;
    cfg.padding_right = 5;
    cfg.padding_bottom = 4;
    let printer = RgbaPrinter::from_font_named("Test", &layout, cfg).unwrap();
    let keys = printer.shape_str("AB").glyphs;
    let placement = render_spf::layout(&keys, &printer.config, &printer.cache);

    // Unpadded: width 11 (6 + 1 + 4), height 8 (natural, vertical_expand off).
    assert_eq!(placement.width, 11 + 3 + 5);
    assert_eq!(placement.height, 8 + 2 + 4);

    // Every glyph shifts by (padding_left, padding_top) -- letter_spacing
    // math between glyphs is untouched.
    assert_eq!(placement.glyphs[0].dst_x, 3);
    assert_eq!(placement.glyphs[0].dst_y, 2);
    assert_eq!(placement.glyphs[1].dst_x, 3 + 6 + 1);
    assert_eq!(placement.glyphs[1].dst_y, 2);
}

#[test]
fn padding_top_stacks_on_top_of_vertical_align_offset() {
    let layout = build_test_layout();
    // "A","B" only: natural height 8, surface height (max_height) 10 --
    // same fixture as vertical_align_middle_offsets_by_half_the_height_difference.
    let mut cfg = config(true, VerticalAlign::Middle);
    cfg.padding_top = 5;
    cfg.padding_bottom = 1;
    let printer = RgbaPrinter::from_font_named("Test", &layout, cfg).unwrap();
    let keys = printer.shape_str("AB").glyphs;
    let placement = render_spf::layout(&keys, &printer.config, &printer.cache);

    assert_eq!(placement.height, 10 + 5 + 1);
    for glyph in &placement.glyphs {
        // (10 - 8) / 2 == 1 alignment offset, plus padding_top.
        assert_eq!(glyph.dst_y, 1 + 5);
    }
}

#[test]
fn zero_padding_is_unobservable() {
    let layout = build_test_layout();
    let printer =
        RgbaPrinter::from_font_named("Test", &layout, config(true, VerticalAlign::Middle))
            .unwrap();
    let keys = printer.shape_str("AB").glyphs;
    let placement = render_spf::layout(&keys, &printer.config, &printer.cache);
    assert_eq!(placement.height, 10);
    assert_eq!(placement.glyphs[0].dst_x, 0);
    assert_eq!(placement.glyphs[0].dst_y, 1);
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

    let rgba_keys = rgba.shape_str("ABC").glyphs;
    let embedded_keys = embedded.shape_str("ABC").glyphs;

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
    let mut printer =
        RgbaPrinter::from_font_named("Test", &layout_data, config(true, VerticalAlign::Top))
            .unwrap();
    let keys = printer.shape_str("AB").glyphs;
    let placement = render_spf::layout(&keys, &printer.config, &printer.cache);
    let image = printer.render(&keys);

    assert_eq!(image.width(), placement.width);
    assert_eq!(image.height(), placement.height);
}
