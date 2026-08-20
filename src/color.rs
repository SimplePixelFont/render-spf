use crate::{vec, Vec};

// ---------------------------------------------------------------------------
// ColorType
// ---------------------------------------------------------------------------

/// Signals the intended mutability of a color entry.
///
/// - [`Dynamic`](ColorType::Dynamic) — a deliberate customisation point.
///   These are the colors the user is *expected* to change (foreground,
///   shadow, highlight, etc.).
/// - [`Absolute`](ColorType::Absolute) — carries a "leave me alone" signal,
///   but can still be changed via [`ColorControl::set`] if needed.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum ColorType {
    #[default]
    Dynamic,
    Absolute,
}

// ---------------------------------------------------------------------------
// ColorEntry
// ---------------------------------------------------------------------------

/// A single entry in the live color palette.
///
/// Preserves the original SPF-defined color so [`Dynamic`](ColorType::Dynamic)
/// entries can always be reset to their authored values.
#[derive(Debug, Clone)]
pub struct ColorEntry {
    /// Whether this color is a [`Dynamic`](ColorType::Dynamic) customisation
    /// point or an [`Absolute`](ColorType::Absolute) stable value.
    pub color_type: ColorType,

    /// The original RGBA value as defined in the SPF color table.
    /// Never modified after construction.
    original_r: u8,
    original_g: u8,
    original_b: u8,
    original_a: u8,

    /// The current RGBA value used at render time.
    /// Modified by [`ColorControl::set`] and reset by [`ColorControl::reset`].
    pub(crate) r: u8,
    pub(crate) g: u8,
    pub(crate) b: u8,
    pub(crate) a: u8,
}

impl ColorEntry {
    pub(crate) fn new(color_type: ColorType, r: u8, g: u8, b: u8, a: u8) -> Self {
        Self {
            color_type,
            original_r: r,
            original_g: g,
            original_b: b,
            original_a: a,
            r,
            g,
            b,
            a,
        }
    }

    /// Returns the original SPF-defined RGBA value.
    pub fn original(&self) -> (u8, u8, u8, u8) {
        (self.original_r, self.original_g, self.original_b, self.original_a)
    }

    /// Returns the current RGBA value used at render time.
    pub fn current(&self) -> (u8, u8, u8, u8) {
        (self.r, self.g, self.b, self.a)
    }
}

// ---------------------------------------------------------------------------
// ColorControl
// ---------------------------------------------------------------------------

/// The live color palette for an [`RgbaPrinter`](crate::cache::RgbaPrinter).
///
/// # Indexing by layout-level color table index
///
/// `ColorControl` is sized to `layout.color_tables.len()` at construction
/// time, with one slot per color table in the layout. Each slot is indexed
/// by the **layout-level** color table index — the same index used in
/// `layout.color_tables`.
///
/// [`PixelRef::color_table_index`] always stores a layout-level index,
/// resolved at build time from the pixmap table's dependency list. This
/// means the same `ColorControl` is unambiguous across glyphs from different
/// pixmap tables, even when those tables link the same color tables in
/// different orders.
///
/// **Example** — pixmap table 1 links `[A, B]`, pixmap table 2 links `[B, A]`:
/// ```text
/// ColorControl.tables[0] = A_entries  ← layout color table 0 (A)
/// ColorControl.tables[1] = B_entries  ← layout color table 1 (B)
///
/// Glyph from PT1, dep-local 0 → layout index 0 → A  ✓
/// Glyph from PT2, dep-local 0 → layout index 1 → B  ✓
/// ```
///
/// # Customisation
///
/// [`Dynamic`](ColorType::Dynamic) colors are the intended customisation
/// surface. [`Absolute`](ColorType::Absolute) colors are stable by convention
/// but can still be overridden via [`set`](Self::set).
///
/// # Example
/// ```ignore
/// // Inspect Dynamic colors in layout color table 0
/// for (idx, entry) in printer.colors.dynamic(0) {
///     println!("index {}: {:?}", idx, entry.current());
/// }
///
/// // Override layout color table 0, entry 0 → red
/// printer.colors.set(0, 0, 255, 0, 0, 255);
///
/// // Reset all Dynamic colors
/// printer.colors.reset_dynamic();
///
/// let image = printer.print_str("Hello");
/// ```
#[derive(Debug, Clone)]
pub struct ColorControl {
    /// One `Vec<ColorEntry>` per layout-level color table.
    /// Indexed directly by layout color table index.
    /// Empty inner `Vec`s represent color tables not used by this font.
    ///
    /// Mutating a `ColorEntry`'s color in place (rather than through
    /// [`set`](Self::set)/[`reset`](Self::reset)/etc.) or structurally
    /// changing a table's length (push/remove) does **not** invalidate
    /// [`resolve`](Self::resolve)'s render-time cache — use the methods
    /// below for anything that needs to show up in the next render. A
    /// structural length change also shifts which table wins pixel
    /// resolution elsewhere in the font (`color_table_index` is baked into
    /// each glyph's pixels at cache-build time), which needs a full glyph
    /// cache reload regardless of this cache, so it's never safe to do
    /// live without one.
    pub tables: Vec<Vec<ColorEntry>>,

    /// Render-time cache, rebuilt lazily on the next [`resolve`](Self::resolve)
    /// call after [`dirty`](Self::dirty) is set. See [`FlatPalette`].
    ///
    /// Plain fields, not interior mutability: `resolve` takes `&mut self`
    /// instead. `Cell`/`RefCell` are `!Sync`, which would make `ColorControl`
    /// (and so `RgbaPrinter`) unusable inside a `static ... RwLock<...>`
    /// font registry — exactly how web-spf stores printers.
    flat: FlatPalette,
    dirty: bool,
}

impl Default for ColorControl {
    fn default() -> Self {
        Self {
            tables: Vec::new(),
            flat: FlatPalette::default(),
            dirty: true,
        }
    }
}

impl ColorControl {
    /// Construct a `ColorControl` pre-sized to `layout_color_table_count` slots.
    /// Slots for color tables not referenced by the font remain empty.
    pub fn with_capacity(layout_color_table_count: usize) -> Self {
        Self {
            tables: vec![Vec::new(); layout_color_table_count],
            flat: FlatPalette::default(),
            dirty: true,
        }
    }

    /// Override any color by layout-level color table index and palette index.
    ///
    /// Works for both [`Dynamic`](ColorType::Dynamic) and
    /// [`Absolute`](ColorType::Absolute) entries — `color_type` is a signal,
    /// not a hard lock.
    ///
    /// Silently ignores out-of-range indexes.
    pub fn set(&mut self, table: usize, index: usize, r: u8, g: u8, b: u8, a: u8) {
        if let Some(entry) = self.tables.get_mut(table).and_then(|t| t.get_mut(index)) {
            entry.r = r;
            entry.g = g;
            entry.b = b;
            entry.a = a;
            self.dirty = true;
        }
    }

    /// Reset a single entry to its original SPF-defined value.
    /// Silently ignores out-of-range indexes.
    pub fn reset(&mut self, table: usize, index: usize) {
        if let Some(entry) = self.tables.get_mut(table).and_then(|t| t.get_mut(index)) {
            entry.r = entry.original_r;
            entry.g = entry.original_g;
            entry.b = entry.original_b;
            entry.a = entry.original_a;
            self.dirty = true;
        }
    }

    /// Reset all [`Dynamic`](ColorType::Dynamic) entries across all tables
    /// to their original SPF-defined values.
    ///
    /// [`Absolute`](ColorType::Absolute) entries are left unchanged.
    pub fn reset_dynamic(&mut self) {
        for table in &mut self.tables {
            for entry in table.iter_mut() {
                if entry.color_type == ColorType::Dynamic {
                    entry.r = entry.original_r;
                    entry.g = entry.original_g;
                    entry.b = entry.original_b;
                    entry.a = entry.original_a;
                }
            }
        }
        self.dirty = true;
    }

    /// Reset all entries (Dynamic and Absolute) to their original values.
    pub fn reset_all(&mut self) {
        for table in &mut self.tables {
            for entry in table.iter_mut() {
                entry.r = entry.original_r;
                entry.g = entry.original_g;
                entry.b = entry.original_b;
                entry.a = entry.original_a;
            }
        }
        self.dirty = true;
    }

    /// Iterate [`Dynamic`](ColorType::Dynamic) entries in a layout-level
    /// color table, yielding `(palette_index, &ColorEntry)` pairs.
    pub fn dynamic(&self, table: usize) -> impl Iterator<Item = (usize, &ColorEntry)> {
        self.tables
            .get(table)
            .map(|t| t.as_slice())
            .unwrap_or(&[])
            .iter()
            .enumerate()
            .filter(|(_, e)| e.color_type == ColorType::Dynamic)
    }

    /// Iterate [`Absolute`](ColorType::Absolute) entries in a layout-level
    /// color table, yielding `(palette_index, &ColorEntry)` pairs.
    pub fn absolute(&self, table: usize) -> impl Iterator<Item = (usize, &ColorEntry)> {
        self.tables
            .get(table)
            .map(|t| t.as_slice())
            .unwrap_or(&[])
            .iter()
            .enumerate()
            .filter(|(_, e)| e.color_type == ColorType::Absolute)
    }

    /// Total number of color table slots (equal to `layout.color_tables.len()`).
    /// Not all slots are necessarily populated — unused tables remain empty.
    pub fn table_count(&self) -> usize {
        self.tables.len()
    }

    /// Resolve a [`PixelRef`] to its current RGBA value.
    ///
    /// `pixel.color_table_index` is a layout-level index. Reads are served
    /// from the [`FlatPalette`] render-time cache, rebuilt here on first
    /// use after any of [`set`](Self::set)/[`reset`](Self::reset)/
    /// [`reset_dynamic`](Self::reset_dynamic)/[`reset_all`](Self::reset_all)
    /// — not on every call, and not per-pixel. Takes `&mut self` (plain
    /// fields, not interior mutability) purely so the cache can be
    /// rebuilt in place — see the note on [`flat`](Self::flat)'s field docs.
    /// Returns transparent black for out-of-range references rather than panicking.
    #[inline]
    pub(crate) fn resolve(&mut self, pixel: PixelRef) -> (u8, u8, u8, u8) {
        if self.dirty {
            self.flat = FlatPalette::rebuild(&self.tables);
            self.dirty = false;
        }
        self.flat.resolve(pixel)
    }
}

// ---------------------------------------------------------------------------
// FlatPalette
// ---------------------------------------------------------------------------

/// A flattened render-time view of [`ColorControl::tables`]: one `offsets`
/// entry per table (plus a trailing sentinel) into a single concatenated
/// `colors` array, in table order.
///
/// Replaces the nested `Vec<Vec<ColorEntry>>` pointer-chase — a dependent
/// allocation-to-allocation hop per pixel — with two reads into small,
/// densely-packed arrays. Semantics are identical to walking `tables`
/// directly: same two-level table/index indexing, same
/// out-of-range-returns-transparent-black behaviour. Only the indirection
/// mechanism changes.
///
/// Deliberately not a dense `(table << 8) | index` LUT: that's a fixed
/// 256×256×4 = 256KB allocation regardless of how many colors a font
/// actually defines, and worse for cache/TLB behaviour than this compact
/// form — real fonts use a handful of colors across a handful of tables,
/// and this stays proportional to that.
#[derive(Debug, Clone, Default)]
struct FlatPalette {
    /// `offsets[t]..offsets[t + 1]` is table `t`'s slice of `colors`.
    /// Length is always `tables.len() + 1`.
    offsets: Vec<u32>,
    /// Every table's entries, concatenated in table order.
    colors: Vec<[u8; 4]>,
}

impl FlatPalette {
    fn rebuild(tables: &[Vec<ColorEntry>]) -> Self {
        let mut offsets = Vec::with_capacity(tables.len() + 1);
        let mut colors = Vec::new();

        offsets.push(0);
        for table in tables {
            for entry in table {
                colors.push([entry.r, entry.g, entry.b, entry.a]);
            }
            offsets.push(colors.len() as u32);
        }

        Self { offsets, colors }
    }

    #[inline]
    fn resolve(&self, pixel: PixelRef) -> (u8, u8, u8, u8) {
        let table = pixel.color_table_index as usize;
        let index = pixel.color_index as usize;

        let Some(&base) = self.offsets.get(table) else {
            return (0, 0, 0, 0);
        };
        let Some(&end) = self.offsets.get(table + 1) else {
            return (0, 0, 0, 0);
        };

        let base = base as usize;
        if index >= (end as usize - base) {
            return (0, 0, 0, 0);
        }

        let [r, g, b, a] = self.colors[base + index];
        (r, g, b, a)
    }
}

// ---------------------------------------------------------------------------
// PixelRef
// ---------------------------------------------------------------------------

/// A reference to a single pixel's color in the layout-level color table space.
///
/// Stored inside [`AbstractCharacter::pixels`](crate::cache::AbstractCharacter)
/// instead of baked RGBA values, so that mutating a [`ColorControl`] entry
/// via [`set`](ColorControl::set)/[`reset`](ColorControl::reset)/etc. is
/// reflected on the next render call — cache invalidation for
/// [`ColorControl`]'s internal [`FlatPalette`] happens automatically inside
/// those methods, not something a caller needs to trigger.
///
/// # Index semantics
///
/// `color_table_index` is a **layout-level** color table index — the same
/// index used in `layout.color_tables`. It is resolved once at build time
/// from the pixmap table's dep-local ordering, so it remains unambiguous
/// regardless of which pixmap table produced the glyph.
///
/// When `Pixmap::per_pixel_color_table_indexes` is absent, `color_table_index`
/// defaults to the layout index of the pixmap table's first dependency color
/// table.
///
/// Identical in memory to a `u16` (two `u8` fields, 2 bytes) but
/// self-documenting and debuggable without bit manipulation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct PixelRef {
    /// Layout-level color table index. Direct index into
    /// [`ColorControl::tables`].
    pub color_table_index: u8,

    /// Index into the selected color table's palette entries.
    pub color_index: u8,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_entry(color_type: ColorType, r: u8, g: u8, b: u8) -> ColorEntry {
        ColorEntry::new(color_type, r, g, b, 255)
    }

    fn two_table_control() -> ColorControl {
        let mut control = ColorControl::with_capacity(2);
        control.tables[0] = vec![
            make_entry(ColorType::Dynamic, 255, 0, 0),
            make_entry(ColorType::Absolute, 0, 255, 0),
        ];
        control.tables[1] = vec![
            make_entry(ColorType::Dynamic, 0, 0, 255),
            make_entry(ColorType::Dynamic, 10, 20, 30),
            make_entry(ColorType::Dynamic, 40, 50, 60),
        ];
        control
    }

    #[test]
    fn flat_palette_matches_nested_resolution_across_tables() {
        let mut control = two_table_control();
        assert_eq!(
            control.resolve(PixelRef { color_table_index: 0, color_index: 0 }),
            (255, 0, 0, 255)
        );
        assert_eq!(
            control.resolve(PixelRef { color_table_index: 0, color_index: 1 }),
            (0, 255, 0, 255)
        );
        // Table 1's offset must land after all of table 0's entries.
        assert_eq!(
            control.resolve(PixelRef { color_table_index: 1, color_index: 0 }),
            (0, 0, 255, 255)
        );
        assert_eq!(
            control.resolve(PixelRef { color_table_index: 1, color_index: 2 }),
            (40, 50, 60, 255)
        );
    }

    #[test]
    fn flat_palette_out_of_range_returns_transparent_black() {
        let mut control = two_table_control();
        // Out-of-range index within a valid table.
        assert_eq!(
            control.resolve(PixelRef { color_table_index: 0, color_index: 5 }),
            (0, 0, 0, 0)
        );
        // Out-of-range table entirely.
        assert_eq!(
            control.resolve(PixelRef { color_table_index: 9, color_index: 0 }),
            (0, 0, 0, 0)
        );
    }

    #[test]
    fn dirty_flag_starts_true_and_clears_after_first_resolve() {
        let mut control = two_table_control();
        assert!(control.dirty);
        control.resolve(PixelRef { color_table_index: 0, color_index: 0 });
        assert!(!control.dirty);
    }

    #[test]
    fn set_marks_dirty_and_is_reflected_on_next_resolve() {
        let mut control = two_table_control();
        control.resolve(PixelRef { color_table_index: 0, color_index: 0 }); // warm the cache
        assert!(!control.dirty);

        control.set(0, 0, 9, 9, 9, 9);
        assert!(control.dirty);
        assert_eq!(
            control.resolve(PixelRef { color_table_index: 0, color_index: 0 }),
            (9, 9, 9, 9)
        );
        assert!(!control.dirty);
    }

    #[test]
    fn reset_dynamic_leaves_absolute_entries_unchanged() {
        let mut control = two_table_control();
        control.set(0, 0, 1, 1, 1, 1); // Dynamic entry
        control.set(0, 1, 2, 2, 2, 2); // Absolute entry
        control.reset_dynamic();

        assert_eq!(
            control.resolve(PixelRef { color_table_index: 0, color_index: 0 }),
            (255, 0, 0, 255) // Dynamic: reverted
        );
        assert_eq!(
            control.resolve(PixelRef { color_table_index: 0, color_index: 1 }),
            (2, 2, 2, 2) // Absolute: left as overridden
        );
    }

    #[test]
    fn reset_all_reverts_dynamic_and_absolute() {
        let mut control = two_table_control();
        control.set(0, 0, 1, 1, 1, 1);
        control.set(0, 1, 2, 2, 2, 2);
        control.reset_all();

        assert_eq!(
            control.resolve(PixelRef { color_table_index: 0, color_index: 0 }),
            (255, 0, 0, 255)
        );
        assert_eq!(
            control.resolve(PixelRef { color_table_index: 0, color_index: 1 }),
            (0, 255, 0, 255)
        );
    }
}
