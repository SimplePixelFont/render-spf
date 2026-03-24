use crate::Vec;

/// A simple ordered key-value store backed by two parallel [`Vec`]s.
///
/// Designed for `no_std` environments where [`std::collections::HashMap`] is
/// unavailable. Lookup is O(n) — acceptable for small character sets typical
/// of embedded font caches. Duplicate keys are not deduplicated; the first
/// inserted entry wins on lookup.
#[derive(Default, Clone, Debug)]
pub struct VecMap<K, V> {
    pub(crate) keys: Vec<K>,
    pub(crate) values: Vec<V>,
}

impl<K, V> VecMap<K, V> {
    pub const fn new() -> Self {
        Self {
            keys: Vec::new(),
            values: Vec::new(),
        }
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            keys: Vec::with_capacity(capacity),
            values: Vec::with_capacity(capacity),
        }
    }

    pub(crate) fn insert(&mut self, key: K, value: V) {
        self.keys.push(key);
        self.values.push(value);
    }

    pub fn get(&self, key: &K) -> Option<&V>
    where
        K: PartialEq,
    {
        self.keys.iter().position(|k| k == key).map(|i| &self.values[i])
    }

    pub fn len(&self) -> usize {
        self.keys.len()
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }
}