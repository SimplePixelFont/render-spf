#![cfg(feature = "std")]

use ril::{Image, Rgba};

use crate::{
    cache::{CharacterCacheImpl, FontCache},
    color::ColorControl,
    print::Placement,
    GlyphId, Vec,
};

/// Bit flags for [`FragmentSet::frag_flags`].
pub struct FragmentFlags;

impl FragmentFlags {
    /// Set when this fragment's alpha is non-zero. Clear for a "rect-only"
    /// fragment — a pixel inside a glyph's pixmap rect that the font
    /// authored as transparent (SPF's `(255, 255, 255, 0)` convention).
    pub const INKED: u8 = 0b0000_0001;
}

/// Tunables for [`RgbaPrinter::render_with_fragments`](super::RgbaPrinter::render_with_fragments).
#[derive(Clone, Debug)]
pub struct FragmentConfig {
    /// When `true` (the default), every pixel in a glyph's pixmap rect
    /// emits a fragment regardless of alpha — matching Phase 4's
    /// `RasterMetadata`, which always carried the full rect so processors
    /// could see the transparent-white convention. When `false`, pixels
    /// with `a == 0` emit no fragment at all: a cost knob for consumers who
    /// only care about ink, since `F` scales with total rect area times
    /// layer count when rect-only fragments are included.
    pub include_rect_only: bool,
}

impl Default for FragmentConfig {
    fn default() -> Self {
        Self {
            include_rect_only: true,
        }
    }
}

/// Per-fragment records plus the pixel-indexed offsets into them, produced
/// by [`RgbaPrinter::render_with_fragments`](super::RgbaPrinter::render_with_fragments).
///
/// Replaces Phase 4's `RasterMetadata` single-owner-per-pixel model.
/// Overlapping glyphs, and future character-pixmap layers, both produce
/// more than one fragment for the same pixel instead of one silently
/// overwriting the other — nothing is discarded, so nothing needs
/// adjudicating. See `spf-engine-plan.md` 6.1.
///
/// # Layout
///
/// Struct-of-arrays, `F` fragments long (`frag_*` fields) plus a
/// `frag_offsets` array `P + 1` long (`P = width * height`):
/// `frag_offsets[p]..frag_offsets[p + 1]` indexes every `frag_*` array for
/// pixel `p`'s fragments. This is the flat/typed-array shape the wasm ABI
/// exposes directly, same as `RasterMetadata` before it.
///
/// # Ordering
///
/// Index `0` within a pixel's range is **bottom-most** (painted first).
/// Fragments are produced in paint order (glyph shaping order) within each
/// pixel — see [`RgbaPrinter::render_with_fragments`](super::RgbaPrinter::render_with_fragments)'s
/// counting-sort description.
///
/// # `frag_layer`
///
/// Always `0` today — SPF has no character-pixmap-layer support yet. The
/// field is reserved so the record shape doesn't change (a breaking change
/// for every consumer) once layers land; see `spf-engine-plan.md` 6.5.
#[derive(Clone, Debug, Default)]
pub struct FragmentSet {
    pub width: u32,
    pub height: u32,

    /// Resolved pre-composite color for this fragment.
    pub frag_rgba: Vec<[u8; 4]>,
    /// Index into the placement's glyph run — same indexing as
    /// [`Shaped`](crate::shape::Shaped)'s `cp_offsets`.
    pub frag_char_index: Vec<u32>,
    /// Always `0` — see the struct-level docs.
    pub frag_layer: Vec<u8>,
    /// Column within the owning glyph's pixmap rect. Signed: layers may
    /// overflow the main pixmap once they exist (`spf-engine-plan.md` 6.5).
    pub frag_char_x: Vec<i16>,
    /// Row within the owning glyph's pixmap rect. Signed, same reason.
    pub frag_char_y: Vec<i16>,
    /// [`FragmentFlags`] bits for this fragment.
    pub frag_flags: Vec<u8>,

    /// `frag_offsets[p]..frag_offsets[p + 1]` indexes the `frag_*` arrays
    /// above for pixel `p`. Length is always `width * height + 1`. An empty
    /// range (`frag_offsets[p] == frag_offsets[p + 1]`) means pixel `p` is
    /// background — no glyph claims it — and composites to fully
    /// transparent.
    pub frag_offsets: Vec<u32>,
}

impl FragmentSet {
    /// Total number of fragments across every pixel (`F`).
    pub fn fragment_count(&self) -> usize {
        self.frag_rgba.len()
    }

    /// `true` when pixel `p` has more than one fragment. Derived, not
    /// stored — `spf-engine-plan.md` 6.2 notes "multiple layers of one
    /// char" and "multiple different chars" share this same condition;
    /// only comparing `frag_char_index` values across the range
    /// distinguishes them.
    pub fn is_multichar(&self, p: usize) -> bool {
        let start = self.frag_offsets[p];
        let end = self.frag_offsets[p + 1];
        end - start > 1
    }

    /// Composite pixel `p`'s fragments source-over, back-to-front (index
    /// `0` first), into a single RGBA value. The "simple path" of
    /// `spf-engine-plan.md` 6.4 — for users who just want default
    /// compositing and don't want to write the offset-walking loop
    /// themselves.
    pub fn composite(&self, p: usize) -> [u8; 4] {
        composite_from(&self.frag_offsets, &self.frag_rgba, p)
    }
}

/// Composite pixel `p`'s fragments source-over, back-to-front, from raw
/// `offsets`/`frag_rgba` slices rather than a [`FragmentSet`] — mirrors the
/// `compositeFrom(offsets, frag_rgba, p)` JS helper from
/// `spf-engine-plan.md` 6.4, for callers who have their own copies of these
/// arrays (e.g. views over wasm memory) rather than an owned `FragmentSet`.
pub fn composite_from(offsets: &[u32], frag_rgba: &[[u8; 4]], p: usize) -> [u8; 4] {
    let start = offsets[p] as usize;
    let end = offsets[p + 1] as usize;
    let mut acc = (0u8, 0u8, 0u8, 0u8);
    for &[r, g, b, a] in &frag_rgba[start..end] {
        acc = source_over(acc, (r, g, b, a));
    }
    [acc.0, acc.1, acc.2, acc.3]
}

/// Standard alpha-compositing "A over B", integer math over `0..=255`.
#[inline]
fn source_over(dst: (u8, u8, u8, u8), src: (u8, u8, u8, u8)) -> (u8, u8, u8, u8) {
    let (dr, dg, db, da) = (dst.0 as u32, dst.1 as u32, dst.2 as u32, dst.3 as u32);
    let (sr, sg, sb, sa) = (src.0 as u32, src.1 as u32, src.2 as u32, src.3 as u32);

    if sa == 255 || da == 0 {
        return src;
    }
    if sa == 0 {
        return dst;
    }

    let out_a = sa + da * (255 - sa) / 255;
    if out_a == 0 {
        return (0, 0, 0, 0);
    }
    let blend = |sc: u32, dc: u32| -> u8 { ((sc * sa + dc * da * (255 - sa) / 255) / out_a) as u8 };

    (blend(sr, dr), blend(sg, dg), blend(sb, db), out_a as u8)
}

/// The output of `RgbaPrinter::render_with_fragments`: the display surface
/// — composited from the fragment set via [`FragmentSet::composite`], the
/// same "simple path" available to external callers — plus the raw
/// [`FragmentSet`] itself for custom compositing.
///
/// Not `Debug` — [`Image<Rgba>`] itself isn't.
#[derive(Clone)]
pub struct FragmentOutput {
    pub surface: Image<Rgba>,
    pub fragments: FragmentSet,
}

/// Generates a [`FragmentSet`] for an already-computed [`Placement`]. See
/// `RgbaPrinter::print_str_with_fragments`'s doc comment for the public
/// entry point; this free function does the actual work so it can borrow
/// `colors` and `cache` disjointly, the same reason `paste_glyph` in
/// `full.rs` is a free function rather than an `RgbaPrinter` method.
///
/// # Algorithm
///
/// Two-pass counting sort, generated **row by row** so the counting/cursor
/// scratch is `O(width)` rather than `O(width * height)` —
/// `spf-engine-plan.md` 6.3:
///
/// 1. For each surface row, walk every glyph covering that row in paint
///    order, counting fragments per column.
/// 2. Prefix-sum those per-row counts into `frag_offsets`, threading a
///    running global cursor across rows.
/// 3. Re-walk the same rows/glyphs in the same paint order, scattering each
///    fragment to `frag_offsets[p] + cursor[p]++`.
///
/// `O(F + P)` total. Paint order within a pixel's fragment range falls out
/// automatically because pass 2 iterates in the same order as pass 1.
pub(crate) fn generate_fragments(
    colors: &mut ColorControl,
    cache: &CharacterCacheImpl,
    placement: &Placement<GlyphId>,
    config: &FragmentConfig,
) -> FragmentSet {
    let (width, height) = (placement.width, placement.height);
    let w = width as usize;
    let n_pixels = w * (height as usize);

    let mut frag_offsets = vec![0u32; n_pixels + 1];
    let mut row_counts = vec![0u32; w];
    let mut cursor: u32 = 0;

    // Pass 1: counts, row by row, folded straight into a running prefix sum.
    for y in 0..height {
        for c in row_counts.iter_mut() {
            *c = 0;
        }
        for placed in &placement.glyphs {
            if y < placed.dst_y || y >= placed.dst_y + placed.height {
                continue;
            }
            let glyph = cache.get(&placed.key).expect("character key not found in cache");
            let py = y - placed.dst_y;
            for px in 0..glyph.width {
                let dst_x = placed.dst_x + px;
                if dst_x >= width {
                    continue;
                }
                let pixel_idx = (py * glyph.width + px) as usize;
                let Some(&pixel_ref) = glyph.pixels.get(pixel_idx) else {
                    continue;
                };
                let (_, _, _, a) = colors.resolve(pixel_ref);
                if a == 0 && !config.include_rect_only {
                    continue;
                }
                row_counts[dst_x as usize] += 1;
            }
        }
        let row_base = (y as usize) * w;
        for x in 0..w {
            frag_offsets[row_base + x] = cursor;
            cursor += row_counts[x];
        }
    }
    frag_offsets[n_pixels] = cursor;
    let total = cursor as usize;

    let mut frag_rgba = vec![[0u8; 4]; total];
    let mut frag_char_index = vec![0u32; total];
    let mut frag_layer = vec![0u8; total];
    let mut frag_char_x = vec![0i16; total];
    let mut frag_char_y = vec![0i16; total];
    let mut frag_flags = vec![0u8; total];

    // Pass 2: scatter, re-walking in the same paint order so index 0 within
    // a pixel's range is bottom-most.
    let mut row_cursor = vec![0u32; w];
    for y in 0..height {
        let row_base = (y as usize) * w;
        row_cursor.copy_from_slice(&frag_offsets[row_base..row_base + w]);

        for (char_index, placed) in placement.glyphs.iter().enumerate() {
            if y < placed.dst_y || y >= placed.dst_y + placed.height {
                continue;
            }
            let glyph = cache.get(&placed.key).expect("character key not found in cache");
            let py = y - placed.dst_y;
            for px in 0..glyph.width {
                let dst_x = placed.dst_x + px;
                if dst_x >= width {
                    continue;
                }
                let pixel_idx = (py * glyph.width + px) as usize;
                let Some(&pixel_ref) = glyph.pixels.get(pixel_idx) else {
                    continue;
                };
                let (r, g, b, a) = colors.resolve(pixel_ref);
                let inked = a != 0;
                if !inked && !config.include_rect_only {
                    continue;
                }

                let slot = &mut row_cursor[dst_x as usize];
                let idx = *slot as usize;
                *slot += 1;

                frag_rgba[idx] = [r, g, b, a];
                frag_char_index[idx] = char_index as u32;
                frag_layer[idx] = 0;
                frag_char_x[idx] = px as i16;
                frag_char_y[idx] = py as i16;
                frag_flags[idx] = if inked { FragmentFlags::INKED } else { 0 };
            }
        }
    }

    FragmentSet {
        width,
        height,
        frag_rgba,
        frag_char_index,
        frag_layer,
        frag_char_x,
        frag_char_y,
        frag_flags,
        frag_offsets,
    }
}

pub(crate) fn composite_surface(fragments: &FragmentSet) -> Image<Rgba> {
    let mut surface = Image::new(fragments.width, fragments.height, Rgba::transparent());
    let w = fragments.width as usize;
    for y in 0..fragments.height {
        for x in 0..fragments.width {
            let p = (y as usize) * w + (x as usize);
            let [r, g, b, a] = fragments.composite(p);
            if a != 0 {
                surface.set_pixel(x, y, Rgba { r, g, b, a });
            }
        }
    }
    surface
}
