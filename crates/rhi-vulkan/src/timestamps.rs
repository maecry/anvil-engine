//! GPU timestamp queries: one query pool per frame slot (ADR-0013 §2).

use std::sync::atomic::{AtomicU32, Ordering};

use ash::vk;
use rhi::{MAX_TIMESTAMPS, RhiError, TimestampInfo};

use crate::error::map_vk;

/// Query pools of all frame slots plus how many queries the last frame of each slot wrote.
pub(crate) struct Timestamps {
    pub info: TimestampInfo,
    pub pools: Vec<vk::QueryPool>,
    pub written: Vec<AtomicU32>,
}

impl Timestamps {
    /// `None` when the queue family cannot write timestamps or the period is zero.
    pub(crate) fn new(
        raw: &ash::Device,
        frames: u32,
        period_ns: f32,
        valid_bits: u32,
        compute_and_graphics: bool,
    ) -> Result<Option<Self>, RhiError> {
        if valid_bits == 0 || !compute_and_graphics || period_ns <= 0.0 {
            tracing::info!(valid_bits, compute_and_graphics, "timestamps unsupported");
            return Ok(None);
        }
        let mut t = Self {
            info: TimestampInfo {
                period_ns,
                valid_bits,
            },
            pools: Vec::new(),
            written: (0..frames).map(|_| AtomicU32::new(0)).collect(),
        };
        let ci = vk::QueryPoolCreateInfo::default()
            .query_type(vk::QueryType::TIMESTAMP)
            .query_count(MAX_TIMESTAMPS);
        for _ in 0..frames {
            // SAFETY: plain creation on a live device; destroyed in `destroy`.
            match unsafe { raw.create_query_pool(&ci, None) } {
                Ok(p) => t.pools.push(p),
                Err(r) => {
                    t.destroy(raw);
                    return Err(map_vk("vkCreateQueryPool", r));
                }
            }
        }
        tracing::info!(period_ns, valid_bits, "timestamps enabled");
        Ok(Some(t))
    }

    /// Destroys the pools; the device must be idle.
    pub(crate) fn destroy(&self, raw: &ash::Device) {
        for &p in &self.pools {
            // SAFETY: device idle, pool owned here and destroyed once.
            unsafe { raw.destroy_query_pool(p, None) };
        }
    }

    /// Ticks written by the last frame of `slot`, masked to the valid bits.
    pub(crate) fn read(&self, raw: &ash::Device, slot: u32) -> Result<Vec<u64>, RhiError> {
        let (Some(pool), Some(n)) = (
            self.pools.get(slot as usize),
            self.written.get(slot as usize),
        ) else {
            return Ok(Vec::new());
        };
        let n = n.load(Ordering::Acquire) as usize;
        let mut out = vec![0u64; n];
        if n == 0 {
            return Ok(out);
        }
        // SAFETY: the pool is alive and `out` holds `n` 64-bit results; the caller waited for the
        // slot's frame, so the queries are available (a never-submitted frame is NOT_READY).
        match unsafe {
            raw.get_query_pool_results(*pool, 0, &mut out, vk::QueryResultFlags::TYPE_64)
        } {
            Ok(()) => {}
            Err(vk::Result::NOT_READY) => return Ok(Vec::new()),
            Err(r) => return Err(map_vk("vkGetQueryPoolResults", r)),
        }
        if self.info.valid_bits < 64 {
            let mask = (1u64 << self.info.valid_bits) - 1;
            out.iter_mut().for_each(|t| *t &= mask);
        }
        Ok(out)
    }
}
