//! Generational registry producing `rhi` tag-typed handles.
//!
//! `foundation::Pool<T>` issues `Handle<T>` where `T` is the stored type, and `Handle`s cannot be
//! re-tagged outside `foundation`. The `rhi` handles are `Handle<XxxTag>`, so a `Pool<XxxTag>`
//! allocates the ids and payloads live in a parallel vector indexed by the handle index.

use foundation::{Handle, Pool};

pub(crate) struct Registry<Tag, V> {
    ids: Pool<Tag>,
    vals: Vec<Option<V>>,
    mk: fn() -> Tag,
}

impl<Tag, V> Registry<Tag, V> {
    pub(crate) fn new(mk: fn() -> Tag) -> Self {
        Self {
            ids: Pool::new(),
            vals: Vec::new(),
            mk,
        }
    }

    pub(crate) fn insert(&mut self, v: V) -> Handle<Tag> {
        let h = self.ids.insert((self.mk)());
        let i = h.index() as usize;
        if self.vals.len() <= i {
            self.vals.resize_with(i + 1, || None);
        }
        self.vals[i] = Some(v);
        h
    }

    pub(crate) fn get(&self, h: Handle<Tag>) -> Option<&V> {
        self.ids.get(h)?;
        self.vals.get(h.index() as usize)?.as_ref()
    }

    pub(crate) fn get_mut(&mut self, h: Handle<Tag>) -> Option<&mut V> {
        self.ids.get(h)?;
        self.vals.get_mut(h.index() as usize)?.as_mut()
    }

    pub(crate) fn remove(&mut self, h: Handle<Tag>) -> Option<V> {
        self.ids.remove(h)?;
        self.vals.get_mut(h.index() as usize)?.take()
    }

    /// Removes the value but keeps the id allocated: `get` is `None` (stale) and the index is
    /// not reused until [`Registry::release`] (bindless slot quarantine, ADR-0010 §4).
    pub(crate) fn take(&mut self, h: Handle<Tag>) -> Option<V> {
        self.ids.get(h)?;
        self.vals.get_mut(h.index() as usize)?.take()
    }

    /// Frees the id of a quarantined (or live) entry so the index can be reused.
    pub(crate) fn release(&mut self, h: Handle<Tag>) {
        if self.ids.remove(h).is_some()
            && let Some(v) = self.vals.get_mut(h.index() as usize)
        {
            *v = None;
        }
    }

    /// Number of live values (quarantined ids are excluded).
    pub(crate) fn len(&self) -> usize {
        self.vals.iter().filter(|v| v.is_some()).count()
    }

    /// Handles of live values.
    pub(crate) fn handles(&self) -> Vec<Handle<Tag>> {
        self.ids
            .iter()
            .filter(|(h, _)| self.vals[h.index() as usize].is_some())
            .map(|(h, _)| h)
            .collect()
    }
}

/// Debug name for an unlabeled object, e.g. `pipeline#3v1` (ADR-0009 §4).
pub(crate) fn generated_name<T>(kind: &str, h: Handle<T>) -> String {
    format!("{kind}#{}v{}", h.index(), h.generation())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rhi::{TextureHandle, TextureTag};

    #[test]
    fn stale_after_remove_and_reuse() {
        let mut r: Registry<TextureTag, u32> = Registry::new(|| TextureTag);
        let a: TextureHandle = r.insert(7);
        assert_eq!(r.get(a), Some(&7));
        assert_eq!(r.remove(a), Some(7));
        assert_eq!(r.get(a), None);
        let b = r.insert(9);
        assert_eq!(b.index(), a.index());
        assert_ne!(b.generation(), a.generation());
        assert_eq!(r.get(a), None);
        assert_eq!(r.get(b), Some(&9));
        assert_eq!(r.len(), 1);
        assert_eq!(generated_name("texture", b), "texture#0v2");
    }

    #[test]
    fn take_quarantines_index_until_release() {
        let mut r: Registry<TextureTag, u32> = Registry::new(|| TextureTag);
        let a = r.insert(1);
        let keep = r.insert(2);
        assert_eq!(r.len(), 2);
        assert_eq!(r.take(a), Some(1));
        assert_eq!(r.get(a), None);
        assert_eq!(r.take(a), None);
        assert_eq!(r.len(), 1, "quarantined entries are not counted");
        assert_eq!(r.handles(), vec![keep]);
        let b = r.insert(3);
        assert_ne!(b.index(), a.index(), "quarantined index must not be reused");
        r.release(a);
        r.release(a); // idempotent
        let c = r.insert(4);
        assert_eq!(c.index(), a.index());
        assert_ne!(c.generation(), a.generation());
        assert_eq!(r.get(a), None);
        assert_eq!(r.get(c), Some(&4));
        assert_eq!(r.len(), 3);
    }
}
