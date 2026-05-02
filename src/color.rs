use crate::Vec;

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
#[derive(Debug, Clone, Default)]
pub struct ColorControl {
    /// One `Vec<ColorEntry>` per layout-level color table.
    /// Indexed directly by layout color table index.
    /// Empty inner `Vec`s represent color tables not used by this font.
    pub tables: Vec<Vec<ColorEntry>>,
}

impl ColorControl {
    /// Construct a `ColorControl` pre-sized to `layout_color_table_count` slots.
    /// Slots for color tables not referenced by the font remain empty.
    pub fn with_capacity(layout_color_table_count: usize) -> Self {
        Self {
            tables: vec![Vec::new(); layout_color_table_count],
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
    /// `pixel.color_table_index` is a layout-level index, so resolution is
    /// a direct array lookup with no additional indirection.
    /// Returns transparent black for out-of-range references rather than panicking.
    #[inline]
    pub(crate) fn resolve(&self, pixel: PixelRef) -> (u8, u8, u8, u8) {
        self.tables
            .get(pixel.color_table_index as usize)
            .and_then(|t| t.get(pixel.color_index as usize))
            .map(|e| (e.r, e.g, e.b, e.a))
            .unwrap_or((0, 0, 0, 0))
    }
}

// ---------------------------------------------------------------------------
// PixelRef
// ---------------------------------------------------------------------------

/// A reference to a single pixel's color in the layout-level color table space.
///
/// Stored inside [`AbstractCharacter::pixels`](crate::cache::AbstractCharacter)
/// instead of baked RGBA values, so that mutating a [`ColorControl`] entry
/// is immediately reflected on the next render call with no cache invalidation.
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
