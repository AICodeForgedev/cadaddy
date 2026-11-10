//! A copy-on-write handle for the big tables of a [`crate::Drawing`] that most edits leave alone.
//!
//! Every edit makes a new `Drawing` while the undo step keeps the old one, so a field held by
//! value is copied on every step: the block table, the constraint list and the file's own handles
//! cost 0.3 MB a step on a drawing with 1,100 blocks, 2,000 steps deep. Held through a
//! [`Shared`], the old and new drawing point at one copy until an edit really changes it
//! (reading derefs to the value; a `&mut` borrow copies it if it is still shared). The project's
//! JSON texts (building spec, agent state…) are held the same way: on a whole job they run to
//! tens of megabytes, and every edit copied them (Q6278: ~47 MB a command). Replace one with
//! `field = Some(text).into()`, which makes a new copy without copying the old.

use std::ops::{Deref, DerefMut};
use std::sync::Arc;

/// A value shared between drawings until one of them changes it.
#[derive(Debug, Default)]
pub struct Shared<T>(Arc<T>);

impl<T> Shared<T> {
    pub fn new(v: T) -> Self {
        Shared(Arc::new(v))
    }
    /// Whether both handles point at the same copy (nothing changed it since one was cloned
    /// from the other).
    pub fn same(&self, other: &Shared<T>) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
    /// A handle that doesn't keep the value alive, to recognise this copy later
    /// ([`Shared::is`]): a cache of something worked out from the value keys on it. While the
    /// handle lives, no other value can take this copy's place, and a change made through any
    /// drawing moves the value to a new copy, so the key never matches a changed value.
    pub fn downgrade(&self) -> std::sync::Weak<T> {
        Arc::downgrade(&self.0)
    }
    /// Whether `w` (from [`Shared::downgrade`]) is this very copy.
    pub fn is(&self, w: &std::sync::Weak<T>) -> bool {
        std::ptr::eq(Arc::as_ptr(&self.0), w.as_ptr())
    }
    /// The value, taken out (copied only if another drawing still shares it).
    pub fn into_inner(self) -> T
    where
        T: Clone,
    {
        Arc::unwrap_or_clone(self.0)
    }
}

impl<T> Clone for Shared<T> {
    fn clone(&self) -> Self {
        Shared(Arc::clone(&self.0))
    }
}

impl<T: PartialEq> PartialEq for Shared<T> {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0) || *self.0 == *other.0
    }
}

impl<T> Deref for Shared<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T: Clone> DerefMut for Shared<T> {
    fn deref_mut(&mut self) -> &mut T {
        Arc::make_mut(&mut self.0)
    }
}

impl<T: serde::Serialize> serde::Serialize for Shared<T> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(s)
    }
}

impl<'de, T: serde::Deserialize<'de>> serde::Deserialize<'de> for Shared<T> {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        T::deserialize(d).map(Shared::new)
    }
}

/// Compares with a plain value too (`assert_eq!(d.constraints, vec![...])`).
impl<T: PartialEq> PartialEq<T> for Shared<T> {
    fn eq(&self, other: &T) -> bool {
        *self.0 == *other
    }
}

impl<T> From<T> for Shared<T> {
    fn from(v: T) -> Self {
        Shared::new(v)
    }
}

impl<A, T: FromIterator<A>> FromIterator<A> for Shared<T> {
    fn from_iter<I: IntoIterator<Item = A>>(iter: I) -> Self {
        Shared::new(T::from_iter(iter))
    }
}

impl<'a, T> IntoIterator for &'a Shared<T>
where
    &'a T: IntoIterator,
{
    type Item = <&'a T as IntoIterator>::Item;
    type IntoIter = <&'a T as IntoIterator>::IntoIter;
    fn into_iter(self) -> Self::IntoIter {
        (&*self.0).into_iter()
    }
}

impl<'a, T: Clone> IntoIterator for &'a mut Shared<T>
where
    &'a mut T: IntoIterator,
{
    type Item = <&'a mut T as IntoIterator>::Item;
    type IntoIter = <&'a mut T as IntoIterator>::IntoIter;
    fn into_iter(self) -> Self::IntoIter {
        Arc::make_mut(&mut self.0).into_iter()
    }
}

impl<T: Clone + IntoIterator> IntoIterator for Shared<T> {
    type Item = T::Item;
    type IntoIter = T::IntoIter;
    fn into_iter(self) -> Self::IntoIter {
        Arc::unwrap_or_clone(self.0).into_iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_change_copies_only_when_shared() {
        let mut a: Shared<Vec<u32>> = Shared::new(vec![1, 2]);
        let b = a.clone();
        assert!(a.same(&b));
        a.push(3);
        assert!(!a.same(&b));
        assert_eq!((&*a, &*b), (&vec![1, 2, 3], &vec![1, 2]));
        let c = a.clone();
        let _ = c.len();
        assert!(a.same(&c), "reading doesn't copy");
    }

    #[test]
    fn a_weak_key_matches_only_its_own_unchanged_copy() {
        let mut a: Shared<Option<String>> = Shared::new(Some("x".into()));
        let w = a.downgrade();
        assert!(a.is(&w));
        let b = a.clone();
        assert!(b.is(&w), "a clone is the same copy");
        drop(b);
        // The only drawing left changes it in place: the value still moves, so the key is stale.
        if let Some(t) = a.as_mut() {
            t.push('y');
        }
        assert!(!a.is(&w));
        assert_eq!(a.as_deref(), Some("xy"));
        let c: Shared<Option<String>> = Shared::new(Some("xy".into()));
        assert!(!c.is(&w), "an equal text elsewhere isn't the same copy");
    }
}
