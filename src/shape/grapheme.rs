use crate::Vec;

/// How many chars starting at `chars[start]` form one grapheme cluster
/// (always >= 1, and <= `chars.len() - start`).
///
/// Deliberately position-based rather than whole-text boundaries computed
/// up front: a successful trie match is allowed to end mid-cluster (see
/// module docs) — e.g. a font with a bare "e" entry but no "e" + combining
/// accent entry matches just the "e", leaving the combining accent to be
/// evaluated fresh from its own position, with no memory of what preceded
/// it. Whole-text boundaries computed in advance would be wrong the moment
/// that happens; asking "how far does a cluster extend from here" is not.
///
/// `chars` is `text.char_indices().collect()` — byte offsets are needed by
/// the `std` implementation ([`unicode_segmentation::GraphemeCursor`] is
/// byte-indexed), and the shaper already builds this pairing for its own
/// purposes, so no separate re-scan is needed here.
#[cfg(feature = "std")]
pub(crate) fn cluster_len_at(chars: &[(usize, char)], start: usize, text: &str) -> usize {
    use unicode_segmentation::GraphemeCursor;

    let byte_start = chars[start].0;
    let mut cursor = GraphemeCursor::new(byte_start, text.len(), true);
    let next_byte = cursor
        .next_boundary(text, 0)
        .ok()
        .flatten()
        .unwrap_or(text.len());

    let mut n = 0;
    let mut idx = start;
    while idx < chars.len() && chars[idx].0 < next_byte {
        n += 1;
        idx += 1;
    }
    n.max(1)
}

/// `no_std` approximation: covers combining marks, ZWJ, variation
/// selectors, regional indicator (flag) pairing, and Hangul jamo — the
/// common cases, not the full UAX #29 algorithm.
#[cfg(not(feature = "std"))]
pub(crate) fn cluster_len_at(chars: &[(usize, char)], start: usize, _text: &str) -> usize {
    let mut n = 1;
    let mut idx = start + 1;
    let mut ri_run: u32 = if is_regional_indicator(chars[start].1) {
        1
    } else {
        0
    };

    while idx < chars.len() {
        let c = chars[idx].1;
        let prev = chars[idx - 1].1;
        let is_ri = is_regional_indicator(c);
        // ZWJ joins whatever follows, regardless of its own category. An RI
        // continues only on an odd run-so-far (completing a pair).
        let continues = prev == '\u{200D}'
            || is_extending(c)
            || (is_ri && ri_run % 2 == 1)
            || (is_hangul_jamo(c) && is_hangul_jamo(prev));

        ri_run = if is_ri { ri_run + 1 } else { 0 };

        if !continues {
            break;
        }
        n += 1;
        idx += 1;
    }

    n
}

#[cfg(not(feature = "std"))]
fn is_extending(c: char) -> bool {
    matches!(c as u32,
        0x0300..=0x036F   // Combining Diacritical Marks
        | 0x1AB0..=0x1AFF // Combining Diacritical Marks Extended
        | 0x1DC0..=0x1DFF // Combining Diacritical Marks Supplement
        | 0x20D0..=0x20FF // Combining Diacritical Marks for Symbols
        | 0xFE20..=0xFE2F // Combining Half Marks
        | 0x200D          // Zero Width Joiner
        | 0xFE00..=0xFE0F // Variation Selectors
        | 0xE0100..=0xE01EF // Variation Selectors Supplement
    )
}

#[cfg(not(feature = "std"))]
fn is_regional_indicator(c: char) -> bool {
    matches!(c as u32, 0x1F1E6..=0x1F1FF)
}

#[cfg(not(feature = "std"))]
fn is_hangul_jamo(c: char) -> bool {
    matches!(c as u32, 0x1100..=0x11FF | 0x3130..=0x318F | 0xA960..=0xA97F | 0xD7B0..=0xD7FF)
}

/// Whether `text` spans more than one grapheme cluster — the distinction
/// between a font's presentational ligature (multiple clusters, one glyph,
/// e.g. "ffi") and a single cluster made of multiple codepoints (e.g. "e"
/// + combining accent, which is not a ligature).
pub(crate) fn is_multi_cluster(text: &str) -> bool {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    if chars.is_empty() {
        return false;
    }
    cluster_len_at(&chars, 0, text) < chars.len()
}

#[cfg(all(test, feature = "std"))]
mod std_tests {
    use super::*;

    fn chars_of(s: &str) -> Vec<(usize, char)> {
        s.char_indices().collect()
    }

    #[test]
    fn precomposed_accent_is_one_cluster_one_char() {
        let text = "é"; // U+00E9, single codepoint
        let chars = chars_of(text);
        assert_eq!(cluster_len_at(&chars, 0, text), 1);
        assert!(!is_multi_cluster(text));
    }

    #[test]
    fn decomposed_accent_is_one_cluster_two_chars() {
        let text = "e\u{0301}"; // 'e' + combining acute accent
        let chars = chars_of(text);
        assert_eq!(chars.len(), 2);
        assert_eq!(cluster_len_at(&chars, 0, text), 2);
        assert!(!is_multi_cluster(text)); // one cluster, not a ligature
    }

    #[test]
    fn two_plain_letters_are_two_clusters() {
        let text = "ab";
        assert!(is_multi_cluster(text)); // this is what makes a trie entry a ligature
    }

    #[test]
    fn zwj_emoji_sequence_is_one_cluster() {
        // "man" + ZWJ + "woman" + ZWJ + "girl": 5 codepoints, one grapheme cluster.
        let text = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}";
        let chars = chars_of(text);
        assert_eq!(cluster_len_at(&chars, 0, text), chars.len());
        assert!(!is_multi_cluster(text));
    }

    #[test]
    fn flag_emoji_pairs_two_regional_indicators() {
        // US flag: two regional indicator codepoints, one cluster.
        let text = "\u{1F1FA}\u{1F1F8}";
        let chars = chars_of(text);
        assert_eq!(cluster_len_at(&chars, 0, text), 2);
    }

    #[test]
    fn three_regional_indicators_pair_then_leave_one_alone() {
        let text = "\u{1F1FA}\u{1F1F8}\u{1F1E6}";
        let chars = chars_of(text);
        assert_eq!(cluster_len_at(&chars, 0, text), 2); // first pair
        assert_eq!(cluster_len_at(&chars, 2, text), 1); // odd one out, alone
    }

    #[test]
    fn cluster_len_at_works_from_an_arbitrary_mid_string_position() {
        let text = "ae\u{0301}b"; // 'a', 'e'+accent (one cluster), 'b'
        let chars = chars_of(text);
        assert_eq!(cluster_len_at(&chars, 0, text), 1); // "a"
        assert_eq!(cluster_len_at(&chars, 1, text), 2); // "e" + accent
        assert_eq!(cluster_len_at(&chars, 3, text), 1); // "b"
    }
}

#[cfg(all(test, not(feature = "std")))]
mod no_std_tests {
    use super::*;

    fn chars_of(s: &str) -> Vec<(usize, char)> {
        s.char_indices().collect()
    }

    #[test]
    fn combining_mark_extends_the_previous_cluster() {
        let text = "e\u{0301}";
        let chars = chars_of(text);
        assert_eq!(cluster_len_at(&chars, 0, text), 2);
        assert!(!is_multi_cluster(text));
    }

    #[test]
    fn two_plain_letters_are_two_clusters() {
        assert!(is_multi_cluster("ab"));
    }

    #[test]
    fn zwj_sequence_is_one_cluster() {
        let text = "\u{1F468}\u{200D}\u{1F469}";
        let chars = chars_of(text);
        assert_eq!(cluster_len_at(&chars, 0, text), chars.len());
    }

    #[test]
    fn regional_indicators_pair_up() {
        let text = "\u{1F1FA}\u{1F1F8}\u{1F1E6}";
        let chars = chars_of(text);
        assert_eq!(cluster_len_at(&chars, 0, text), 2);
        assert_eq!(cluster_len_at(&chars, 2, text), 1);
    }

    #[test]
    fn hangul_jamo_run_is_one_cluster() {
        let text = "\u{1100}\u{1161}\u{11A8}"; // choseong+jungseong+jongseong
        let chars = chars_of(text);
        assert_eq!(cluster_len_at(&chars, 0, text), chars.len());
    }
}
