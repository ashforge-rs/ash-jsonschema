//! An insertion-ordered, string-keyed map.
//!
//! Several JSON Schema keywords take an object of named subschemas —
//! `properties`, `patternProperties`, `dependentSchemas`, `$defs`. JSON objects
//! are unordered by specification, but a *generated* document that reshuffles
//! its keys between builds is hostile to diffs, golden files, and code review.
//!
//! [`OrderedMap`] keeps the order the caller wrote the builder calls in. It is
//! backed by a `Vec`, so lookup is linear — irrelevant at the handful-of-keys
//! scale a schema object works at, and it costs the crate no extra dependency.

use serde::{Serialize, Serializer};

/// A string-keyed map that preserves insertion order.
///
/// Re-inserting an existing key overwrites the value **in place**, keeping the
/// key at its original position, and returns the previous value.
///
/// ```
/// # use ash_jsonschema_builder::OrderedMap;
/// let mut map = OrderedMap::new();
/// map.insert("b", 1);
/// map.insert("a", 2);
/// map.insert("b", 3);
///
/// assert_eq!(map.keys().collect::<Vec<_>>(), ["b", "a"]);
/// assert_eq!(map.get("b"), Some(&3));
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrderedMap<T> {
    entries: Vec<(String, T)>,
}

impl<T> OrderedMap<T> {
    /// An empty map.
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Inserts `value` under `key`, returning the previous value if the key was
    /// already present. An existing key keeps its position.
    pub fn insert(&mut self, key: impl Into<String>, value: T) -> Option<T> {
        let key = key.into();
        for (existing, slot) in &mut self.entries {
            if *existing == key {
                return Some(std::mem::replace(slot, value));
            }
        }
        self.entries.push((key, value));
        None
    }

    /// The value stored under `key`, if any.
    pub fn get(&self, key: &str) -> Option<&T> {
        self.entries.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    /// Whether `key` is present.
    pub fn contains_key(&self, key: &str) -> bool {
        self.entries.iter().any(|(k, _)| k == key)
    }

    /// The number of entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the map holds no entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The entries, in insertion order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &T)> {
        self.entries.iter().map(|(k, v)| (k.as_str(), v))
    }

    /// The keys, in insertion order.
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|(k, _)| k.as_str())
    }

    /// The values, in insertion order.
    pub fn values(&self) -> impl Iterator<Item = &T> {
        self.entries.iter().map(|(_, v)| v)
    }
}

impl<T> Default for OrderedMap<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K: Into<String>, T> FromIterator<(K, T)> for OrderedMap<T> {
    fn from_iter<I: IntoIterator<Item = (K, T)>>(iter: I) -> Self {
        let mut map = Self::new();
        for (key, value) in iter {
            map.insert(key, value);
        }
        map
    }
}

impl<T> IntoIterator for OrderedMap<T> {
    type Item = (String, T);
    type IntoIter = std::vec::IntoIter<(String, T)>;

    fn into_iter(self) -> Self::IntoIter {
        self.entries.into_iter()
    }
}

impl<T: Serialize> Serialize for OrderedMap<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_map(self.entries.iter().map(|(k, v)| (k, v)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_returns_the_displaced_value_and_keeps_position() {
        let mut map = OrderedMap::new();
        assert_eq!(map.insert("a", 1), None);
        assert_eq!(map.insert("b", 2), None);
        assert_eq!(map.insert("a", 9), Some(1));

        assert_eq!(map.keys().collect::<Vec<_>>(), ["a", "b"]);
        assert_eq!(map.get("a"), Some(&9));
        assert_eq!(map.len(), 2);
    }

    #[test]
    fn serializes_in_insertion_order() {
        let map: OrderedMap<u8> = [("z", 1), ("m", 2), ("a", 3)].into_iter().collect();
        let json = serde_json::to_string(&map).unwrap();
        assert_eq!(json, r#"{"z":1,"m":2,"a":3}"#);
    }

    #[test]
    fn empty_map_reports_empty() {
        let map: OrderedMap<u8> = OrderedMap::new();
        assert!(map.is_empty());
        assert!(!map.contains_key("nope"));
        assert_eq!(map.get("nope"), None);
    }
}
