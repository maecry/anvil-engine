use crate::Handle;

struct Slot<T> {
    generation: u32,
    value: Option<T>,
}

/// Generational slot pool. Not thread-safe; owners wrap it when sharing.
///
/// Generation 0 is never issued. A slot whose generation would pass `u32::MAX`
/// is retired (never reused) instead of wrapping.
pub struct Pool<T> {
    slots: Vec<Slot<T>>,
    free: Vec<u32>,
    len: usize,
}

impl<T> Pool<T> {
    /// Creates an empty pool.
    pub fn new() -> Self {
        Self {
            slots: Vec::new(),
            free: Vec::new(),
            len: 0,
        }
    }

    /// Stores `value`, reusing a free slot if any.
    ///
    /// # Panics
    /// If more than `u32::MAX` slots would be needed.
    pub fn insert(&mut self, value: T) -> Handle<T> {
        self.len += 1;
        if let Some(index) = self.free.pop() {
            let slot = &mut self.slots[index as usize];
            slot.value = Some(value);
            return Handle::new(index, slot.generation);
        }
        let index = u32::try_from(self.slots.len()).expect("pool slot index overflow");
        self.slots.push(Slot {
            generation: 1,
            value: Some(value),
        });
        Handle::new(index, 1)
    }

    fn slot(&self, h: Handle<T>) -> Option<&Slot<T>> {
        self.slots
            .get(h.index() as usize)
            .filter(|s| s.generation == h.generation())
    }

    /// Returns the value if `h` is live; `None` if out of range, vacant or stale.
    pub fn get(&self, h: Handle<T>) -> Option<&T> {
        self.slot(h)?.value.as_ref()
    }

    /// Mutable variant of [`Pool::get`].
    pub fn get_mut(&mut self, h: Handle<T>) -> Option<&mut T> {
        self.slots
            .get_mut(h.index() as usize)
            .filter(|s| s.generation == h.generation())?
            .value
            .as_mut()
    }

    /// Removes and returns the value, invalidating `h` and all copies of it.
    pub fn remove(&mut self, h: Handle<T>) -> Option<T> {
        let slot = self
            .slots
            .get_mut(h.index() as usize)
            .filter(|s| s.generation == h.generation())?;
        let value = slot.value.take()?;
        self.len -= 1;
        if let Some(next) = slot.generation.checked_add(1) {
            slot.generation = next;
            self.free.push(h.index());
        } // else: generation exhausted, slot retired
        Some(value)
    }

    /// Number of live values.
    pub fn len(&self) -> usize {
        self.len
    }

    /// True if no values are live.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Iterates live values with their handles, skipping vacant slots.
    pub fn iter(&self) -> impl Iterator<Item = (Handle<T>, &T)> {
        self.slots.iter().enumerate().filter_map(|(i, s)| {
            // Index fits u32: insert guarantees it.
            s.value
                .as_ref()
                .map(|v| (Handle::new(i as u32, s.generation), v))
        })
    }
}

impl<T> Default for Pool<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn insert_get_remove() {
        let mut p = Pool::new();
        let h = p.insert("a");
        assert_eq!(h.generation(), 1);
        assert_eq!(p.get(h), Some(&"a"));
        *p.get_mut(h).unwrap() = "b";
        assert_eq!(p.len(), 1);
        assert_eq!(p.remove(h), Some("b"));
        assert_eq!(p.len(), 0);
        assert!(p.is_empty());
    }

    #[test]
    fn stale_after_remove() {
        let mut p = Pool::new();
        let h = p.insert(1);
        p.remove(h);
        assert_eq!(p.get(h), None);
        assert_eq!(p.get_mut(h), None);
        assert_eq!(p.remove(h), None);
    }

    #[test]
    fn stale_after_slot_reuse() {
        let mut p = Pool::new();
        let old = p.insert(1);
        p.remove(old);
        let new = p.insert(2);
        assert_eq!(new.index(), old.index());
        assert_ne!(new.generation(), old.generation());
        assert_eq!(p.get(old), None);
        assert_eq!(p.get(new), Some(&2));
    }

    #[test]
    fn null_and_out_of_range_are_invalid() {
        let mut p = Pool::new();
        assert_eq!(p.get(Handle::NULL), None);
        p.insert(1);
        assert_eq!(p.get(Handle::NULL), None);
        assert_eq!(p.remove(Handle::NULL), None);
        assert_eq!(p.get(Handle::new(99, 1)), None);
    }

    #[test]
    fn generation_retired_at_max() {
        let mut p = Pool::new();
        let h = p.insert(1);
        p.slots[0].generation = u32::MAX;
        let h_max = Handle::new(h.index(), u32::MAX);
        assert_eq!(p.remove(h_max), Some(1));
        assert!(p.free.is_empty());
        let n = p.insert(2);
        assert_ne!(n.index(), h.index()); // retired slot not reused
        assert_eq!(p.get(h_max), None);
        assert!(p.slots[0].value.is_none());
    }

    #[test]
    fn iter_skips_vacant() {
        let mut p = Pool::new();
        let a = p.insert('a');
        let b = p.insert('b');
        let c = p.insert('c');
        p.remove(b);
        let got: Vec<_> = p.iter().collect();
        assert_eq!(got, vec![(a, &'a'), (c, &'c')]);
    }

    #[test]
    fn matches_hashmap_model() {
        // Deterministic LCG instead of proptest (no extra dependency).
        let (mut p, mut model, mut live, mut dead) =
            (Pool::new(), HashMap::new(), Vec::new(), Vec::new());
        let mut s: u64 = 12345;
        for i in 0..2000u32 {
            s = s
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            if !(s >> 33).is_multiple_of(3) || live.is_empty() {
                let h = p.insert(i);
                model.insert(h, i);
                live.push(h);
            } else {
                let h = live.swap_remove((s >> 40) as usize % live.len());
                assert_eq!(p.remove(h), model.remove(&h));
                dead.push(h);
            }
        }
        assert_eq!(p.len(), model.len());
        for (h, v) in &model {
            assert_eq!(p.get(*h), Some(v));
        }
        for h in dead {
            assert_eq!(p.get(h), None);
        }
    }
}
