use crate::Vec;

/// A direct index into a backend's flat glyph storage
/// (`Vec<AbstractCharacter>` for the std backend, `Vec<AbstractCharacterU8>`
/// for embedded). Scoped to whichever cache produced it — not comparable
/// across different fonts/printers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct GlyphId(pub u32);

#[derive(Debug, Clone)]
struct TrieNode {
    glyph: Option<GlyphId>,
    /// True when this node's key spans more than one grapheme cluster (a
    /// font's presentational choice, e.g. "ffi" as one glyph) as opposed to
    /// one cluster made of multiple codepoints (e.g. "e" + combining acute,
    /// which is a single user-perceived character, not a ligature).
    is_ligature: bool,
    edge_start: u32,
    edge_len: u16,
}

const ASCII_DISPATCH_LEN: usize = 128;

/// A flat trie mapping codepoint sequences to [`GlyphId`]s, replacing a
/// `HashMap<String, _>` (std) / linear scan (embedded) with one lookup
/// structure shared by both backends. Plain `Vec`s throughout, so it works
/// under `no_std`.
///
/// Built once per font load via [`Trie::build`]; queried per print call via
/// [`Trie::match_longest`], which implements maximal munch with
/// last-terminal memory — see its docs for the algorithm.
#[derive(Debug, Clone)]
pub struct Trie {
    nodes: Vec<TrieNode>,
    /// Every node's children, concatenated; a node's own slice is
    /// `edges[edge_start..edge_start + edge_len]`, sorted by `char` within
    /// that slice. Linear scan over a node's slice beats binary search —
    /// real fonts have few children per node.
    edges: Vec<(char, u32)>,
    /// Direct dispatch for the root's ASCII children — the hottest node by
    /// a wide margin. `-1` means no such child. Non-ASCII root children
    /// (and every non-root node) still go through `edges`.
    ascii_root: [i32; ASCII_DISPATCH_LEN],
    max_key_len: usize,
}

impl Default for Trie {
    fn default() -> Self {
        Self::build(&[])
    }
}

impl Trie {
    /// Build a trie from `(code_points, glyph, is_ligature)` entries.
    /// Later entries for an already-inserted key overwrite earlier ones.
    pub fn build(entries: &[(&str, GlyphId, bool)]) -> Self {
        let mut nodes: Vec<TrieNode> = Vec::with_capacity(entries.len() + 1);
        nodes.push(TrieNode {
            glyph: None,
            is_ligature: false,
            edge_start: 0,
            edge_len: 0,
        });
        // Staging adjacency, parallel to `nodes`, flattened into `edges`
        // once every entry has been inserted.
        let mut staging: Vec<Vec<(char, u32)>> = Vec::with_capacity(entries.len() + 1);
        staging.push(Vec::new());
        let mut max_key_len = 0usize;

        for &(key, glyph, is_ligature) in entries {
            let mut node = 0u32;
            let mut len = 0usize;
            for c in key.chars() {
                len += 1;
                let existing = staging[node as usize]
                    .iter()
                    .find(|&&(edge_c, _)| edge_c == c)
                    .map(|&(_, idx)| idx);
                node = match existing {
                    Some(idx) => idx,
                    None => {
                        let new_idx = nodes.len() as u32;
                        nodes.push(TrieNode {
                            glyph: None,
                            is_ligature: false,
                            edge_start: 0,
                            edge_len: 0,
                        });
                        staging.push(Vec::new());
                        staging[node as usize].push((c, new_idx));
                        new_idx
                    }
                };
            }
            nodes[node as usize].glyph = Some(glyph);
            nodes[node as usize].is_ligature = is_ligature;
            max_key_len = max_key_len.max(len);
        }

        let mut edges = Vec::new();
        for (i, mut children) in staging.into_iter().enumerate() {
            children.sort_by_key(|&(c, _)| c);
            nodes[i].edge_start = edges.len() as u32;
            nodes[i].edge_len = children.len() as u16;
            edges.extend(children);
        }

        let mut ascii_root = [-1i32; ASCII_DISPATCH_LEN];
        let root_start = nodes[0].edge_start as usize;
        let root_len = nodes[0].edge_len as usize;
        for &(c, idx) in &edges[root_start..root_start + root_len] {
            if (c as u32) < ASCII_DISPATCH_LEN as u32 {
                ascii_root[c as usize] = idx as i32;
            }
        }

        Self {
            nodes,
            edges,
            ascii_root,
            max_key_len,
        }
    }

    /// The longest key inserted, in codepoints. Zero for an empty trie.
    pub fn max_key_len(&self) -> usize {
        self.max_key_len
    }

    #[inline]
    fn find_child(&self, node: u32, c: char) -> Option<u32> {
        if node == 0 && (c as u32) < ASCII_DISPATCH_LEN as u32 {
            let idx = self.ascii_root[c as usize];
            return if idx >= 0 { Some(idx as u32) } else { None };
        }
        let n = &self.nodes[node as usize];
        let start = n.edge_start as usize;
        let end = start + n.edge_len as usize;
        self.edges[start..end]
            .iter()
            .find(|&&(edge_c, _)| edge_c == c)
            .map(|&(_, idx)| idx)
    }

    /// Maximal munch with last-terminal memory: descend `chars[start..]` one
    /// codepoint at a time, remembering the deepest terminal seen so far,
    /// and stop as soon as no edge exists for the next codepoint (or the
    /// input ends). Returns the remembered terminal — `None` if the descent
    /// never passed through one.
    ///
    /// Example: trie has `ff` and `ffi`, input `ffl` → descend `f` (no
    /// edge check needed, not terminal), `f` (terminal, remember `ff`),
    /// then no `l` edge from the `ff` node → return the remembered `ff`
    /// match, having consumed 2 codepoints. The caller restarts matching
    /// at the `l`. This is O(n) amortized: each call either advances past
    /// the whole matched span or fails after one wasted step.
    ///
    /// `allow_ligatures = false` skips recording (but still descends past)
    /// terminals whose key spanned more than one grapheme cluster.
    pub fn match_longest(
        &self,
        chars: &[char],
        start: usize,
        allow_ligatures: bool,
    ) -> Option<(usize, GlyphId)> {
        let mut node = 0u32;
        let mut j = start;
        let mut last: Option<(usize, GlyphId)> = None;

        loop {
            let n = &self.nodes[node as usize];
            if let Some(g) = n.glyph {
                if allow_ligatures || !n.is_ligature {
                    last = Some((j, g));
                }
            }
            if j >= chars.len() {
                break;
            }
            match self.find_child(node, chars[j]) {
                Some(child) => {
                    node = child;
                    j += 1;
                }
                None => break,
            }
        }

        last
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chars_of(s: &str) -> Vec<char> {
        s.chars().collect()
    }

    #[test]
    fn empty_trie_matches_nothing() {
        let trie = Trie::default();
        let chars = chars_of("a");
        assert_eq!(trie.match_longest(&chars, 0, true), None);
    }

    #[test]
    fn single_char_ascii_match_uses_dispatch_table() {
        let trie = Trie::build(&[("a", GlyphId(0), false), ("b", GlyphId(1), false)]);
        let chars = chars_of("ab");
        assert_eq!(trie.match_longest(&chars, 0, true), Some((1, GlyphId(0))));
        assert_eq!(trie.match_longest(&chars, 1, true), Some((2, GlyphId(1))));
    }

    #[test]
    fn non_ascii_match_falls_through_to_edges() {
        let trie = Trie::build(&[("é", GlyphId(5), false)]);
        let chars = chars_of("é");
        assert_eq!(trie.match_longest(&chars, 0, true), Some((1, GlyphId(5))));
    }

    #[test]
    fn maximal_munch_with_last_terminal_memory() {
        // "ff" and "ffi" both defined; input "ffl" should match "ff" (2
        // chars), not fail outright at the unmatched "l".
        let trie = Trie::build(&[("ff", GlyphId(0), true), ("ffi", GlyphId(1), true)]);
        let chars = chars_of("ffl");
        assert_eq!(trie.match_longest(&chars, 0, true), Some((2, GlyphId(0))));
    }

    #[test]
    fn maximal_munch_prefers_the_longer_match_when_available() {
        let trie = Trie::build(&[("ff", GlyphId(0), true), ("ffi", GlyphId(1), true)]);
        let chars = chars_of("ffi");
        assert_eq!(trie.match_longest(&chars, 0, true), Some((3, GlyphId(1))));
    }

    #[test]
    fn descent_failure_with_no_terminal_at_all_returns_none() {
        let trie = Trie::build(&[("xy", GlyphId(0), false)]);
        let chars = chars_of("xz");
        assert_eq!(trie.match_longest(&chars, 0, true), None);
    }

    #[test]
    fn ligature_toggle_off_skips_ligature_terminals() {
        let trie = Trie::build(&[
            ("f", GlyphId(0), false),
            ("ffi", GlyphId(1), true), // ligature: 3 clusters -> 1 glyph
        ]);
        let chars = chars_of("ffi");
        // Ligatures allowed: matches the full "ffi" ligature.
        assert_eq!(trie.match_longest(&chars, 0, true), Some((3, GlyphId(1))));
        // Ligatures disabled: "ffi" node is skipped even though reached;
        // falls back to the single "f" terminal encountered along the way.
        assert_eq!(trie.match_longest(&chars, 0, false), Some((1, GlyphId(0))));
    }

    #[test]
    fn max_key_len_tracks_the_longest_inserted_key() {
        let trie = Trie::build(&[("a", GlyphId(0), false), ("ffi", GlyphId(1), true)]);
        assert_eq!(trie.max_key_len(), 3);
    }

    #[test]
    fn later_entry_for_same_key_overwrites_earlier_one() {
        let trie = Trie::build(&[("a", GlyphId(0), false), ("a", GlyphId(9), false)]);
        let chars = chars_of("a");
        assert_eq!(trie.match_longest(&chars, 0, true), Some((1, GlyphId(9))));
    }
}
