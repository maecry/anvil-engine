//! Cross-frame pool of transient textures and buffers (ADR-0012 §6), generic over the
//! description `D` (the key), the handle `H` and the state `S` a pooled object was left in.

use rhi::RhiError;

/// Unused entries are destroyed after this many frames.
pub(crate) const EVICT_AFTER_FRAMES: u64 = 8;

pub(crate) struct Entry<D, H, S> {
    pub desc: D,
    pub handle: H,
    /// State the object was left in by its last frame.
    pub state: S,
    last_used: u64,
}

pub(crate) struct TransientPool<D, H, S> {
    pub entries: Vec<Entry<D, H, S>>,
    frames_in_flight: u64,
}

impl<D: PartialEq + Copy, H: Copy, S: Copy> TransientPool<D, H, S> {
    pub fn new(frames_in_flight: u32) -> Self {
        Self {
            entries: Vec::new(),
            frames_in_flight: u64::from(frames_in_flight.max(1)),
        }
    }

    /// Entry index and whether it was just created. An entry is reusable once the frame that
    /// last used it has retired: `begin_frame` of `frame` waited for frame `frame - fif`.
    // shortcut: linear scan and no memory aliasing, fine for tens of transients; index by desc
    // (and alias memory) when M5 adds many resolution-sized targets.
    pub fn acquire(
        &mut self,
        desc: D,
        frame: u64,
        initial: S,
        create: impl FnOnce() -> Result<H, RhiError>,
    ) -> Result<(usize, bool), RhiError> {
        let fif = self.frames_in_flight;
        if let Some(i) = self
            .entries
            .iter()
            .position(|e| e.desc == desc && e.last_used + fif <= frame)
        {
            self.entries[i].last_used = frame;
            return Ok((i, false));
        }
        self.entries.push(Entry {
            desc,
            handle: create()?,
            state: initial,
            last_used: frame,
        });
        Ok((self.entries.len() - 1, true))
    }

    /// Removes entries unused for [`EVICT_AFTER_FRAMES`], handing their objects to `destroy`.
    /// Indices handed out by `acquire` are invalid afterwards.
    pub fn evict(&mut self, frame: u64, mut destroy: impl FnMut(H)) -> usize {
        let before = self.entries.len();
        self.entries.retain(|e| {
            let dead = e.last_used + EVICT_AFTER_FRAMES <= frame;
            if dead {
                destroy(e.handle);
            }
            !dead
        });
        before - self.entries.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::TransientDesc;
    use rhi::{Format, TextureHandle, TextureState, TextureTag, TextureUsage};

    type TransientPool = super::TransientPool<TransientDesc, TextureHandle, TextureState>;

    fn desc(w: u32) -> TransientDesc {
        TransientDesc {
            width: w,
            height: w,
            format: Format::Rgba8Unorm,
            usage: TextureUsage::SAMPLED,
        }
    }

    fn handles(n: usize) -> Vec<TextureHandle> {
        let mut p = foundation::Pool::<TextureTag>::new();
        (0..n).map(|_| p.insert(TextureTag)).collect()
    }

    #[test]
    fn reuse_only_after_the_frame_slot_retired() {
        let h = handles(4);
        let mut pool = TransientPool::new(2);
        let mut next = h.iter().copied();
        let mut get = |pool: &mut TransientPool, frame| {
            pool.acquire(desc(8), frame, TextureState::Undefined, || {
                Ok(next.next().unwrap())
            })
            .unwrap()
        };
        assert_eq!(get(&mut pool, 1), (0, true));
        // Frame 2 may still overlap frame 1 on the GPU: a second texture is created.
        assert_eq!(get(&mut pool, 2), (1, true));
        // Frame 3 follows the retirement of frame 1: reuse.
        assert_eq!(get(&mut pool, 3), (0, false));
        assert_eq!(get(&mut pool, 4), (1, false));
        // A different desc never matches.
        assert_eq!(
            pool.acquire(desc(16), 5, TextureState::Undefined, || Ok(h[2]))
                .unwrap(),
            (2, true)
        );
    }

    #[test]
    fn unused_entries_are_evicted() {
        let h = handles(2);
        let mut pool = TransientPool::new(2);
        pool.acquire(desc(8), 1, TextureState::Undefined, || Ok(h[0]))
            .unwrap();
        pool.acquire(desc(16), 5, TextureState::Undefined, || Ok(h[1]))
            .unwrap();
        let mut destroyed = Vec::new();
        assert_eq!(
            pool.evict(1 + EVICT_AFTER_FRAMES - 1, |t| destroyed.push(t)),
            0
        );
        assert_eq!(pool.evict(1 + EVICT_AFTER_FRAMES, |t| destroyed.push(t)), 1);
        assert_eq!(destroyed, [h[0]]);
        assert_eq!(pool.entries.len(), 1);
    }
}
