use super::grapheme::cluster_len_at;
use super::trie::{GlyphId, Trie};
use crate::Vec;

/// The output of [`shape`]ing a run of text against one font's [`Trie`].
///
/// Shape on text change, not per frame — the render path then only touches
/// `glyphs` (a direct index per entry, no hashing, no `String`s in the hot
/// loop). `cp_offsets`/`cp_data` exist because a ligature maps to more than
/// one codepoint — this is the shape a future batched processor's
/// `codepointsFor(charIndex)` needs. `byte_offsets` is needed for cursor
/// placement and hit-testing in an editor (click inside an `ffi` glyph →
/// which of its three source positions?); reconstructing it after the fact
/// is much harder than emitting it here.
#[derive(Debug, Clone, Default)]
pub struct Shaped {
    /// One entry per rendered glyph, in `layout()`'s expected `keys` order
    /// — `Shaped::glyphs` is exactly what gets passed to `layout`.
    pub glyphs: Vec<GlyphId>,
    /// `cp_offsets[i]..cp_offsets[i + 1]` indexes `cp_data` for glyph `i`.
    /// Length is always `glyphs.len() + 1`.
    pub cp_offsets: Vec<u32>,
    /// Every glyph's source codepoints, concatenated in glyph order.
    pub cp_data: Vec<char>,
    /// The source-`&str` byte offset each glyph started at. Length matches
    /// `glyphs.len()` — one start position per glyph; a glyph's end is the
    /// next glyph's start, or the source text's length for the last one.
    pub byte_offsets: Vec<u32>,
}

/// Shape `text` against `trie`: maximal munch per [`Trie::match_longest`],
/// falling back to consuming one whole grapheme cluster (never a lone
/// combining mark) and recording `notdef` when the trie has no match at
/// all at a position. The font's own trie entries are always tried first —
/// grapheme clustering only ever decides how much of the input to skip
/// past on failure, it never overrides a successful trie match (which is
/// allowed to end mid-cluster; see [`cluster_len_at`]'s docs).
///
/// `notdef` should be a real, valid glyph in the caller's cache — reserved
/// index 0 (a blank glyph) by convention, so a font with total lookup
/// failures still lays out sensibly instead of panicking on the render
/// path's `cache.get()`.
pub fn shape(text: &str, trie: &Trie, notdef: GlyphId, allow_ligatures: bool) -> Shaped {
    let indexed: Vec<(usize, char)> = text.char_indices().collect();
    let chars: Vec<char> = indexed.iter().map(|&(_, c)| c).collect();

    let mut glyphs = Vec::with_capacity(chars.len());
    let mut cp_offsets = Vec::with_capacity(chars.len() + 1);
    cp_offsets.push(0u32);
    let mut cp_data = Vec::with_capacity(chars.len());
    let mut byte_offsets = Vec::with_capacity(chars.len());

    let mut i = 0usize;
    while i < chars.len() {
        let (end, glyph) = match trie.match_longest(&chars, i, allow_ligatures) {
            Some((end, glyph)) => (end, glyph),
            None => (i + cluster_len_at(&indexed, i, text), notdef),
        };
        // Defensive: a zero-length match (only possible from a malformed
        // trie with an empty-string entry) would otherwise loop forever.
        let end = end.max(i + 1);

        glyphs.push(glyph);
        byte_offsets.push(indexed[i].0 as u32);
        cp_data.extend_from_slice(&chars[i..end.min(chars.len())]);
        cp_offsets.push(cp_data.len() as u32);
        i = end;
    }

    Shaped {
        glyphs,
        cp_offsets,
        cp_data,
        byte_offsets,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vec;

    const NOTDEF: GlyphId = GlyphId(0);

    fn build(entries: &[(&str, u32, bool)]) -> Trie {
        let owned: Vec<(&str, GlyphId, bool)> = entries
            .iter()
            .map(|&(k, id, lig)| (k, GlyphId(id), lig))
            .collect();
        Trie::build(&owned)
    }

    #[test]
    fn ascii_text_shapes_one_glyph_per_char() {
        let trie = build(&[("a", 1, false), ("b", 2, false)]);
        let shaped = shape("ab", &trie, NOTDEF, true);
        assert_eq!(shaped.glyphs, vec![GlyphId(1), GlyphId(2)]);
        assert_eq!(shaped.byte_offsets, vec![0, 1]);
        assert_eq!(shaped.cp_offsets, vec![0, 1, 2]);
        assert_eq!(shaped.cp_data, vec!['a', 'b']);
    }

    #[test]
    fn ligature_maps_multiple_codepoints_to_one_glyph() {
        let trie = build(&[("f", 1, false), ("ffi", 2, true)]);
        let shaped = shape("ffi", &trie, NOTDEF, true);
        assert_eq!(shaped.glyphs, vec![GlyphId(2)]);
        assert_eq!(shaped.cp_data, vec!['f', 'f', 'i']);
        assert_eq!(shaped.cp_offsets, vec![0, 3]); // one glyph, 3 codepoints
        assert_eq!(shaped.byte_offsets, vec![0]);
    }

    #[test]
    fn maximal_munch_ffl_matches_ff_then_falls_back_on_l() {
        let trie = build(&[("f", 10, false), ("ff", 1, true), ("ffi", 2, true)]);
        let shaped = shape("ffl", &trie, NOTDEF, true);
        // "ff" matches (2 chars), then "l" has no trie entry -> notdef,
        // consuming the whole (single-char) cluster.
        assert_eq!(shaped.glyphs, vec![GlyphId(1), NOTDEF]);
        assert_eq!(shaped.byte_offsets, vec![0, 2]);
        assert_eq!(shaped.cp_data, vec!['f', 'f', 'l']);
    }

    #[test]
    fn ligature_toggle_off_falls_back_to_shorter_non_ligature_match() {
        let trie = build(&[("f", 10, false), ("ffi", 2, true)]);
        let shaped = shape("ffi", &trie, NOTDEF, false);
        // Ligature disallowed: only "f" (non-ligature) matches, three times.
        assert_eq!(shaped.glyphs, vec![GlyphId(10), GlyphId(10), NOTDEF]);
    }

    #[test]
    fn combining_mark_without_font_entry_is_one_notdef_not_two() {
        let trie = build(&[("x", 1, false)]); // font knows nothing about 'e' or accents
        let shaped = shape("e\u{0301}x", &trie, NOTDEF, true);
        // "e" + combining accent: no trie entry, one grapheme cluster ->
        // ONE notdef (not one notdef for 'e' and a second for the stray
        // accent). Then "x" matches normally.
        assert_eq!(shaped.glyphs, vec![NOTDEF, GlyphId(1)]);
        assert_eq!(shaped.cp_data, vec!['e', '\u{0301}', 'x']);
        assert_eq!(shaped.cp_offsets, vec![0, 2, 3]);
        assert_eq!(shaped.byte_offsets, vec![0, 3]); // accent is 2 bytes in utf-8
    }

    #[test]
    fn partial_match_leaving_a_combining_mark_is_its_own_notdef() {
        // Font has a bare "e" but not "e"+accent -- the trie match is
        // allowed to end mid-cluster, leaving the accent to fend for
        // itself at the next position.
        let trie = build(&[("e", 1, false)]);
        let shaped = shape("e\u{0301}", &trie, NOTDEF, true);
        assert_eq!(shaped.glyphs, vec![GlyphId(1), NOTDEF]);
        assert_eq!(shaped.cp_data, vec!['e', '\u{0301}']);
    }

    #[test]
    fn empty_text_shapes_to_nothing() {
        let trie = build(&[("a", 1, false)]);
        let shaped = shape("", &trie, NOTDEF, true);
        assert!(shaped.glyphs.is_empty());
        assert_eq!(shaped.cp_offsets, vec![0]);
    }

    #[test]
    fn total_lookup_failure_still_produces_notdef_glyphs_not_a_panic() {
        let trie = Trie::default();
        let shaped = shape("xyz", &trie, NOTDEF, true);
        assert_eq!(shaped.glyphs, vec![NOTDEF, NOTDEF, NOTDEF]);
    }
}
