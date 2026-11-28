use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::{
    collections::{BTreeMap, HashSet},
    fmt,
    iter::Map,
    ops::Index,
    slice,
    sync::Arc,
};

type Property = (Arc<String>, Arc<String>);

/// A sorted property map. Registry descriptors share equal maps and strings,
/// and a cloned descriptor copies its map only when edited.
#[derive(Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BlockProperties(Option<Arc<Vec<Property>>>);

impl BlockProperties {
    fn entries(&self) -> &[Property] {
        self.0.as_deref().map_or(&[], Vec::as_slice)
    }

    fn find(&self, key: &str) -> Result<usize, usize> {
        self.entries()
            .binary_search_by(|(name, _)| name.as_str().cmp(key))
    }

    pub fn len(&self) -> usize {
        self.entries().len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_none()
    }

    pub fn get(&self, key: &str) -> Option<&String> {
        self.find(key).ok().map(|index| &*self.entries()[index].1)
    }

    pub fn contains_key(&self, key: &str) -> bool {
        self.find(key).is_ok()
    }

    pub fn get_mut(&mut self, key: &str) -> Option<&mut String> {
        let index = self.find(key).ok()?;
        let entries = Arc::make_mut(self.0.as_mut()?);
        Some(Arc::make_mut(&mut entries[index].1))
    }

    pub fn insert(&mut self, key: String, value: String) -> Option<String> {
        let index = self.find(&key);
        let entries = Arc::make_mut(self.0.get_or_insert_with(Arc::default));
        match index {
            Ok(index) => Some(Arc::unwrap_or_clone(std::mem::replace(
                &mut entries[index].1,
                Arc::new(value),
            ))),
            Err(index) => {
                entries.insert(index, (Arc::new(key), Arc::new(value)));
                None
            }
        }
    }

    pub fn clear(&mut self) {
        self.0 = None;
    }

    pub fn iter(&self) -> impl ExactSizeIterator<Item = (&String, &String)> {
        self.into_iter()
    }

    pub fn keys(&self) -> impl ExactSizeIterator<Item = &String> {
        self.iter().map(|(name, _)| name)
    }

    pub fn values(&self) -> impl ExactSizeIterator<Item = &String> {
        self.iter().map(|(_, value)| value)
    }

    pub(crate) fn intern(&mut self, maps: &mut HashSet<Self>, strings: &mut HashSet<Arc<String>>) {
        if self.is_empty() {
            return;
        }
        if let Some(shared) = maps.get(self) {
            *self = shared.clone();
            return;
        }
        for (key, value) in Arc::make_mut(self.0.as_mut().unwrap()) {
            for text in [key, value] {
                if let Some(shared) = strings.get(&**text) {
                    *text = shared.clone();
                } else {
                    strings.insert(text.clone());
                }
            }
        }
        maps.insert(self.clone());
    }

    #[cfg(test)]
    pub(crate) fn shares_storage(&self, other: &Self) -> bool {
        self.0
            .as_ref()
            .zip(other.0.as_ref())
            .is_some_and(|(a, b)| Arc::ptr_eq(a, b))
    }
}

impl<K: AsRef<str> + ?Sized> Index<&K> for BlockProperties {
    type Output = String;

    fn index(&self, key: &K) -> &String {
        self.get(key.as_ref()).expect("no entry found for key")
    }
}

impl<'a> IntoIterator for &'a BlockProperties {
    type Item = (&'a String, &'a String);
    type IntoIter = Map<slice::Iter<'a, Property>, fn(&Property) -> (&String, &String)>;

    fn into_iter(self) -> Self::IntoIter {
        self.entries()
            .iter()
            .map(|(name, value)| (&**name, &**value))
    }
}

impl From<BTreeMap<String, String>> for BlockProperties {
    fn from(map: BTreeMap<String, String>) -> Self {
        if map.is_empty() {
            return Self::default();
        }
        Self(Some(Arc::new(
            map.into_iter()
                .map(|(key, value)| (Arc::new(key), Arc::new(value)))
                .collect(),
        )))
    }
}

impl<const N: usize> From<[(String, String); N]> for BlockProperties {
    fn from(entries: [(String, String); N]) -> Self {
        BTreeMap::from(entries).into()
    }
}

impl FromIterator<(String, String)> for BlockProperties {
    fn from_iter<T: IntoIterator<Item = (String, String)>>(entries: T) -> Self {
        entries.into_iter().collect::<BTreeMap<_, _>>().into()
    }
}

impl fmt::Debug for BlockProperties {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map().entries(self).finish()
    }
}

impl Serialize for BlockProperties {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_map(self)
    }
}

impl<'de> Deserialize<'de> for BlockProperties {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        BTreeMap::deserialize(deserializer).map(Self::from)
    }
}
