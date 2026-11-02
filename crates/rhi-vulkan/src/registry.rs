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

    pub(crate) fn len(&self) -> usize {
        self.ids.len()
    }

    pub(crate) fn handles(&self) -> Vec<Handle<Tag>> {
        self.ids.iter().map(|(h, _)| h).collect()
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
}
