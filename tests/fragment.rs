//! Tests for `RgbaPrinter::render_with_fragments`/`print_str_with_fragments`
//! (Phase 6's fragment model, replacing Phase 4's `RasterMetadata`). Uses
//! synthetic in-memory `Layout`s (see `tests/placement.rs` for why
//! `Default::default()` + field assignment is used instead of a struct
//! literal).
#![cfg(feature = "std")] // RgbaPrinter/ril are std-only

use render_spf::*;
use spf::core::*;

/// Packs `values` (each `< 2^bits_per_pixel`) into an SPF pixmap byte
/// stream -- the exact inverse of the crate's own unpacking
/// (`view_bits::<Lsb0>().chunks(bits_per_pixel).load_be()`), verified by
/// round-trip against that call directly (bitvec's Lsb0 `chunks` +
/// `load_be` combination places each chunk's *lowest* bit index as the
/// value's LSB, not its MSB -- despite the "be" in the name). This is the
/// general-purpose way to hand-author multi-value pixmap fixtures without
/// error-prone manual bit arithmetic.
fn pack_bits(values: &[u8], bits_per_pixel: usize) -> Vec<u8> {
    let mut bits: Vec<bool> = Vec::with_capacity(values.len() * bits_per_pixel);
    for &v in values {
        for bit in 0..bits_per_pixel {
            bits.push((v >> bit) & 1 != 0);
        }
    }
    let mut bytes = vec![0u8; bits.len().div_ceil(8)];
    for (i, &b) in bits.iter().enumerate() {
        if b {
            bytes[i / 8] |= 1 << (i % 8);
        }
    }
    bytes
}

/// Two overlapping glyphs sharing one color table (index 0: transparent
/// white per SPF convention, index 1: opaque red, index 2: opaque green):
/// - "A": 2w x 1h, advance_x 1 (overlap-capable), pixels `[red, red]`.
/// - "B": 2w x 1h, advance_x 2, pixels `[transparent-white, green]`.
///
/// Printing "AB" with letter_spacing 0 places A at x=[0,2) and B at
/// x=[1,3), so column x=1 is A's *second* (red, opaque) pixel underneath
/// B's *first* (transparent) pixel -- the overlap case the fragment model
/// exists for: both sources survive as separate fragments instead of one
/// silently overwriting the other.
fn build_overlap_layout() -> Layout {
    let mut white = Color::default();
    white.red = 255;
    white.green = 255;
    white.blue = 255;
    white.custom_alpha = Some(0);

    let mut red = Color::default();
    red.red = 255;

    let mut green = Color::default();
    green.green = 255;

    let mut color_table = ColorTable::default();
    color_table.colors = vec![white, red, green];

    let mut pixmap_a = Pixmap::default();
    pixmap_a.custom_width = Some(2);
    pixmap_a.custom_height = Some(1);
    pixmap_a.data = pack_bits(&[1, 1], 2);

    let mut pixmap_b = Pixmap::default();
    pixmap_b.custom_width = Some(2);
    pixmap_b.custom_height = Some(1);
    pixmap_b.data = pack_bits(&[0, 2], 2);

    let mut character_a = Character::default();
    character_a.advance_x = Some(1);
    character_a.code_points = "A".to_string();

    let mut character_b = Character::default();
    character_b.advance_x = Some(2);
    character_b.code_points = "B".to_string();

    let mut pixmap_table = PixmapTable::default();
    pixmap_table.constant_bits_per_pixel = Some(2);
    pixmap_table.color_table_indexes = Some(vec![0]);
    pixmap_table.pixmaps = vec![pixmap_a, pixmap_b];

    let mut character_table = CharacterTable::default();
    character_table.pixmap_table_indexes = Some(vec![0]);
    character_table.characters = vec![character_a, character_b];

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

fn config(letter_spacing: u8) -> GenericPrintConfig {
    GenericPrintConfig {
        letter_spacing,
        vertical_expand: false,
        vertical_align: VerticalAlign::Top,
        allow_ligatures: true,
    }
}

#[test]
fn overlap_column_carries_both_glyphs_as_separate_fragments() {
    let layout = build_overlap_layout();
    let mut printer = RgbaPrinter::from_font_named("Test", &layout, config(0)).unwrap();
    let output = printer.print_str_with_fragments("AB", &FragmentConfig::default());

    assert_eq!(output.surface.width(), 3);
    assert_eq!(output.surface.height(), 1);
    let f = &output.fragments;

    // x=0: A only (red, opaque). One fragment.
    assert!(!f.is_multichar(0));
    let start = f.frag_offsets[0] as usize;
    assert_eq!(f.frag_offsets[1] - f.frag_offsets[0], 1);
    assert_eq!(f.frag_rgba[start], [255, 0, 0, 255]);
    assert_eq!(f.frag_char_index[start], 0);
    assert_eq!(f.frag_char_x[start], 0);
    assert_eq!(f.frag_char_y[start], 0);
    assert_eq!(f.frag_flags[start] & FragmentFlags::INKED, FragmentFlags::INKED);
    assert_eq!(f.composite(0), [255, 0, 0, 255]);

    // x=1: overlap column. Both A's opaque red and B's transparent-white
    // survive as fragments, bottom-most (paint order) first.
    assert!(f.is_multichar(1));
    let start = f.frag_offsets[1] as usize;
    let end = f.frag_offsets[2] as usize;
    assert_eq!(end - start, 2);

    // Fragment 0: A's second pixel, red, opaque, inked.
    assert_eq!(f.frag_char_index[start], 0);
    assert_eq!(f.frag_char_x[start], 1);
    assert_eq!(f.frag_char_y[start], 0);
    assert_eq!(f.frag_rgba[start], [255, 0, 0, 255]);
    assert_eq!(f.frag_flags[start] & FragmentFlags::INKED, FragmentFlags::INKED);

    // Fragment 1: B's first pixel, transparent white, rect-only.
    assert_eq!(f.frag_char_index[start + 1], 1);
    assert_eq!(f.frag_char_x[start + 1], 0);
    assert_eq!(f.frag_char_y[start + 1], 0);
    assert_eq!(f.frag_rgba[start + 1], [255, 255, 255, 0]);
    assert_eq!(f.frag_flags[start + 1] & FragmentFlags::INKED, 0);

    // Composited (source-over, back-to-front): B's transparent hole leaves
    // A's red visible underneath -- matches the plain display surface.
    assert_eq!(f.composite(1), [255, 0, 0, 255]);
    let p1 = output.surface.pixels().next().unwrap().get(1).unwrap();
    assert_eq!((p1.r, p1.g, p1.b, p1.a), (255, 0, 0, 255));

    // x=2: B only (green, opaque). One fragment.
    assert!(!f.is_multichar(2));
    let start = f.frag_offsets[2] as usize;
    assert_eq!(f.frag_char_index[start], 1);
    assert_eq!(f.frag_char_x[start], 1);
    assert_eq!(f.frag_char_y[start], 0);
    assert_eq!(f.frag_rgba[start], [0, 255, 0, 255]);
    assert_eq!(f.composite(2), [0, 255, 0, 255]);
}

#[test]
fn rect_only_suppression_drops_the_transparent_fragment_but_keeps_composite() {
    let layout = build_overlap_layout();
    let mut printer = RgbaPrinter::from_font_named("Test", &layout, config(0)).unwrap();
    let output = printer.print_str_with_fragments(
        "AB",
        &FragmentConfig {
            include_rect_only: false,
        },
    );
    let f = &output.fragments;

    // The overlap column now carries only A's inked fragment -- B's
    // transparent rect-only pixel was suppressed.
    assert!(!f.is_multichar(1));
    let start = f.frag_offsets[1] as usize;
    assert_eq!(f.frag_offsets[2] - f.frag_offsets[1], 1);
    assert_eq!(f.frag_char_index[start], 0);
    assert_eq!(f.composite(1), [255, 0, 0, 255]);
}

/// A single 1x1 glyph "A", opaque red, advance_x 1. Printing "AA" with
/// letter_spacing 1 leaves a genuine background column between the two
/// glyphs -- neither glyph's paste loop ever touches it.
fn build_background_layout() -> Layout {
    let mut red = Color::default();
    red.red = 255;

    let mut color_table = ColorTable::default();
    color_table.colors = vec![red];

    let mut pixmap = Pixmap::default();
    pixmap.custom_width = Some(1);
    pixmap.custom_height = Some(1);
    pixmap.data = vec![0b0000_0000]; // single-entry table: index 0 = red

    let mut character = Character::default();
    character.advance_x = Some(1);
    character.code_points = "A".to_string();

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

#[test]
fn letter_spacing_gap_has_no_fragments_and_composites_transparent() {
    let layout = build_background_layout();
    let mut printer = RgbaPrinter::from_font_named("Test", &layout, config(1)).unwrap();
    let output = printer.print_str_with_fragments("AA", &FragmentConfig::default());
    let f = &output.fragments;

    assert_eq!(output.surface.width(), 3); // A(1) + gap(1) + A(1)

    assert_eq!(f.frag_offsets[1] - f.frag_offsets[0], 1);
    let start = f.frag_offsets[0] as usize;
    assert_eq!(f.frag_char_index[start], 0);
    assert_eq!(f.frag_rgba[start], [255, 0, 0, 255]);

    // The letter_spacing column: no glyph's pixmap rect covers it, so it
    // has an empty fragment range and composites to fully transparent.
    assert_eq!(f.frag_offsets[1], f.frag_offsets[2]);
    assert_eq!(f.composite(1), [0, 0, 0, 0]);

    assert_eq!(f.frag_offsets[3] - f.frag_offsets[2], 1);
    let start = f.frag_offsets[2] as usize;
    assert_eq!(f.frag_char_index[start], 1);
    assert_eq!(f.frag_rgba[start], [255, 0, 0, 255]);
}

#[test]
fn render_with_fragments_composited_surface_agrees_with_render() {
    let layout = build_overlap_layout();
    let mut printer = RgbaPrinter::from_font_named("Test", &layout, config(0)).unwrap();

    let keys = printer.shape_str("AB").glyphs;
    let plain = printer.render(&keys);
    let fused = printer.render_with_fragments(&keys, &FragmentConfig::default());

    assert_eq!(plain.width(), fused.surface.width());
    assert_eq!(plain.height(), fused.surface.height());
    for y in 0..plain.height() {
        for x in 0..plain.width() {
            let a = plain.pixels().nth(y as usize).unwrap().get(x as usize).unwrap();
            let b = fused
                .surface
                .pixels()
                .nth(y as usize)
                .unwrap()
                .get(x as usize)
                .unwrap();
            assert_eq!((a.r, a.g, a.b, a.a), (b.r, b.g, b.b, b.a));
        }
    }
}
