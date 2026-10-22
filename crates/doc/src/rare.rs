//! [`Rare`]: a field most objects leave empty, kept out of line.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A field most objects leave at its empty value (their extended data, annotation scales,
/// material): one pointer in the object, the value itself allocated only once it is set. It reads
/// and changes like the value (`Deref`/`DerefMut`; changing an empty one allocates it) and
/// serializes exactly as the value does.
#[derive(Clone, Default)]
pub struct Rare<T>(Option<Box<T>>);

/// The empty value a [`Rare`] reads as when it holds nothing.
pub trait RareEmpty: Default + PartialEq + 'static {
    fn empty() -> &'static Self;
}

static NO_XDATA: Vec<crate::RawTag> = Vec::new();
static NO_SCALES: Vec<crate::AnnoScale> = Vec::new();
static NO_STRING: Option<String> = None;

impl RareEmpty for Vec<crate::RawTag> {
    fn empty() -> &'static Self {
        &NO_XDATA
    }
}
impl RareEmpty for Vec<crate::AnnoScale> {
    fn empty() -> &'static Self {
        &NO_SCALES
    }
}
impl RareEmpty for Option<String> {
    fn empty() -> &'static Self {
        &NO_STRING
    }
}

impl<T: RareEmpty> Rare<T> {
    /// Holding its empty value (whether or not it was ever allocated): for
    /// `skip_serializing_if`.
    pub fn is_empty_value(&self) -> bool {
        self.0.as_deref().is_none_or(|v| *v == T::default())
    }
    /// The value, taken out (empty left behind, nothing allocated).
    pub fn take(&mut self) -> T {
        self.0.take().map(|b| *b).unwrap_or_default()
    }
}

impl<T: RareEmpty> From<T> for Rare<T> {
    fn from(v: T) -> Self {
        if v == T::default() { Rare(None) } else { Rare(Some(Box::new(v))) }
    }
}

impl<T: RareEmpty> std::ops::Deref for Rare<T> {
    type Target = T;
    fn deref(&self) -> &T {
        self.0.as_deref().unwrap_or_else(|| T::empty())
    }
}

impl<T: RareEmpty> std::ops::DerefMut for Rare<T> {
    fn deref_mut(&mut self) -> &mut T {
        self.0.get_or_insert_with(Box::default)
    }
}

impl<'a, T> IntoIterator for &'a Rare<Vec<T>>
where
    Vec<T>: RareEmpty,
{
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<T> FromIterator<T> for Rare<Vec<T>>
where
    Vec<T>: RareEmpty,
{
    fn from_iter<I: IntoIterator<Item = T>>(it: I) -> Self {
        Rare::from(it.into_iter().collect::<Vec<T>>())
    }
}

impl<T: RareEmpty> PartialEq for Rare<T> {
    fn eq(&self, other: &Self) -> bool {
        **self == **other
    }
}

impl<T: RareEmpty + std::fmt::Debug> std::fmt::Debug for Rare<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        (**self).fmt(f)
    }
}

impl<T: RareEmpty + Serialize> Serialize for Rare<T> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        (**self).serialize(s)
    }
}

impl<'de, T: RareEmpty + Deserialize<'de>> Deserialize<'de> for Rare<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        T::deserialize(d).map(Rare::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RawTag;

    #[test]
    fn empty_until_set_and_reads_like_the_value() {
        let mut x: Rare<Vec<RawTag>> = Rare::default();
        assert!(x.is_empty() && x.0.is_none() && x.is_empty_value());
        x.push(RawTag { code: 1001, value: "APP".into() });
        assert_eq!(x.len(), 1);
        assert!(x.0.is_some() && !x.is_empty_value());
        assert_eq!(x.take().len(), 1);
        assert!(x.0.is_none());
        // Emptied but allocated equals never set.
        let mut y: Rare<Vec<RawTag>> = Rare::default();
        y.clear();
        assert_eq!(y, Rare::default());
        assert!(Rare::from(Vec::<RawTag>::new()).0.is_none());
    }

    #[test]
    fn serializes_as_the_value() {
        let m: Rare<Option<String>> = Some("Brick".to_string()).into();
        assert_eq!(serde_json::to_string(&m).unwrap(), "\"Brick\"");
        let back: Rare<Option<String>> = serde_json::from_str("null").unwrap();
        assert!(back.0.is_none() && back.is_none());
        let back: Rare<Option<String>> = serde_json::from_str("\"Brick\"").unwrap();
        assert_eq!(back.as_deref(), Some("Brick"));
    }
}
