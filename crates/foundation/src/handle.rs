use std::{fmt, hash::Hash, hash::Hasher, marker::PhantomData};

/// Typed generational key into a [`Pool`](crate::Pool). Owns nothing.
///
/// `T` is a zero-sized tag chosen by the owning crate; no bounds are placed on it.
pub struct Handle<T> {
    index: u32,
    generation: u32,
    _tag: PhantomData<fn() -> T>,
}

impl<T> Handle<T> {
    /// The always-invalid handle (generation 0 is never issued).
    pub const NULL: Self = Self {
        index: 0,
        generation: 0,
        _tag: PhantomData,
    };

    pub(crate) const fn new(index: u32, generation: u32) -> Self {
        Self {
            index,
            generation,
            _tag: PhantomData,
        }
    }

    /// Slot index.
    pub fn index(self) -> u32 {
        self.index
    }

    /// Slot generation; never 0 for a handle issued by a pool.
    pub fn generation(self) -> u32 {
        self.generation
    }

    /// True for [`Handle::NULL`].
    pub fn is_null(self) -> bool {
        self.generation == 0
    }
}

impl<T> Clone for Handle<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Copy for Handle<T> {}
impl<T> PartialEq for Handle<T> {
    fn eq(&self, o: &Self) -> bool {
        self.index == o.index && self.generation == o.generation
    }
}
impl<T> Eq for Handle<T> {}
impl<T> Hash for Handle<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.index.hash(state);
        self.generation.hash(state);
    }
}
impl<T> fmt::Debug for Handle<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Handle({}v{})", self.index, self.generation)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NotSendNotSync(#[allow(dead_code)] std::rc::Rc<()>);

    fn assert_traits<X: Copy + Send + Sync + Eq + Hash>() {}

    #[test]
    fn handle_is_copy_send_sync_for_any_t() {
        assert_traits::<Handle<NotSendNotSync>>();
    }

    #[test]
    fn handle_is_8_bytes() {
        assert_eq!(size_of::<Handle<NotSendNotSync>>(), 8);
    }

    #[test]
    fn null_handle() {
        let n = Handle::<()>::NULL;
        assert!(n.is_null());
        assert_eq!((n.index(), n.generation()), (0, 0));
    }
}
