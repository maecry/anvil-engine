//! The mesh arena (ADR-0013 §6): every mesh lives in one shared vertex buffer and one shared
//! index buffer, suballocated here, so a single index-buffer bind and one vertex address serve
//! every draw. Mesh records hold offsets; draws use the record's vertex offset as base vertex.

use rhi::{BufferDesc, BufferHandle, BufferUsage, Device, Memory, RhiError};

use crate::mesh::Vertex;

/// Vertices the arena holds. shortcut: fixed capacity (12 MiB of vertices, 4 MiB of indices),
/// `create_mesh` fails when it is full; grow by allocating a bigger pair and re-uploading when a
/// scene needs it (M6 asset streaming).
pub(crate) const VERTEX_CAPACITY: u32 = 256 * 1024;
/// 32-bit indices the arena holds.
pub(crate) const INDEX_CAPACITY: u32 = 1024 * 1024;

/// First-fit range allocator over `0..capacity` units with coalescing free.
#[derive(Debug)]
pub(crate) struct RangeAlloc {
    /// Free ranges `(start, len)`, sorted by start, never adjacent.
    free: Vec<(u32, u32)>,
}

impl RangeAlloc {
    pub fn new(capacity: u32) -> Self {
        Self {
            free: vec![(0, capacity)],
        }
    }

    /// `len` units, or `None` when no free range is big enough.
    pub fn alloc(&mut self, len: u32) -> Option<u32> {
        let i = self.free.iter().position(|&(_, l)| l >= len)?;
        let (start, l) = self.free[i];
        if l == len {
            self.free.remove(i);
        } else {
            self.free[i] = (start + len, l - len);
        }
        Some(start)
    }

    /// Returns a range obtained from [`Self::alloc`].
    pub fn free(&mut self, start: u32, len: u32) {
        let at = self.free.partition_point(|&(s, _)| s < start);
        self.free.insert(at, (start, len));
        // Coalesce with the next, then the previous range.
        if at + 1 < self.free.len() && self.free[at].0 + self.free[at].1 == self.free[at + 1].0 {
            self.free[at].1 += self.free[at + 1].1;
            self.free.remove(at + 1);
        }
        if at > 0 && self.free[at - 1].0 + self.free[at - 1].1 == self.free[at].0 {
            self.free[at - 1].1 += self.free[at].1;
            self.free.remove(at);
        }
    }

    /// Units still free.
    pub fn free_units(&self) -> u32 {
        self.free.iter().map(|f| f.1).sum()
    }
}

/// Where a mesh lives in the arena.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ArenaSlot {
    /// First vertex (the base vertex of every draw of the mesh).
    pub vertex: u32,
    pub vertex_count: u32,
    /// First index of the mesh's index data (all levels of detail, one after another).
    pub index: u32,
    pub index_count: u32,
}

/// The shared buffers and their allocators.
pub(crate) struct Arena {
    pub vertices: BufferHandle,
    pub indices: BufferHandle,
    /// BDA of the vertex buffer (the one vertex address of every draw).
    pub vertex_address: u64,
    vertex_alloc: RangeAlloc,
    index_alloc: RangeAlloc,
}

impl Arena {
    pub fn new<D: Device>(device: &D) -> Result<Self, RhiError> {
        let make = |label, size, usage| {
            device.create_buffer(&BufferDesc {
                label: Some(label),
                size,
                usage: usage | BufferUsage::TRANSFER_DST,
                memory: Memory::GpuOnly,
            })
        };
        let vertices = make(
            "arena.vertices",
            u64::from(VERTEX_CAPACITY) * size_of::<Vertex>() as u64,
            BufferUsage::STORAGE,
        )?;
        let indices = match make(
            "arena.indices",
            u64::from(INDEX_CAPACITY) * 4,
            BufferUsage::INDEX,
        ) {
            Ok(b) => b,
            Err(e) => {
                device.destroy_buffer(vertices);
                return Err(e);
            }
        };
        Ok(Self {
            vertices,
            indices,
            vertex_address: device.buffer_address(vertices)?,
            vertex_alloc: RangeAlloc::new(VERTEX_CAPACITY),
            index_alloc: RangeAlloc::new(INDEX_CAPACITY),
        })
    }

    /// Reserves space for `vertex_count` vertices and `index_count` indices.
    pub fn alloc(&mut self, vertex_count: u32, index_count: u32) -> Result<ArenaSlot, RhiError> {
        let full = |what: &str, want: u32, free: u32| {
            RhiError::InvalidDescriptor(format!(
                "mesh arena full: {what} {want} requested, {free} free"
            ))
        };
        let vertex = self
            .vertex_alloc
            .alloc(vertex_count)
            .ok_or_else(|| full("vertices", vertex_count, self.vertex_alloc.free_units()))?;
        let Some(index) = self.index_alloc.alloc(index_count) else {
            self.vertex_alloc.free(vertex, vertex_count);
            return Err(full("indices", index_count, self.index_alloc.free_units()));
        };
        Ok(ArenaSlot {
            vertex,
            vertex_count,
            index,
            index_count,
        })
    }

    pub fn free(&mut self, s: ArenaSlot) {
        self.vertex_alloc.free(s.vertex, s.vertex_count);
        self.index_alloc.free(s.index, s.index_count);
    }

    pub fn destroy<D: Device>(self, device: &D) {
        device.destroy_buffer(self.vertices);
        device.destroy_buffer(self.indices);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_fit_and_coalescing() {
        let mut a = RangeAlloc::new(100);
        let (x, y, z) = (
            a.alloc(30).unwrap(),
            a.alloc(30).unwrap(),
            a.alloc(30).unwrap(),
        );
        assert_eq!((x, y, z), (0, 30, 60));
        assert!(a.alloc(11).is_none(), "only 10 left");
        a.free(y, 30);
        assert_eq!(a.alloc(20), Some(30), "reuses the hole");
        a.free(30, 20);
        a.free(x, 30);
        a.free(z, 30);
        // Everything coalesced back into one range.
        assert_eq!(a.free, [(0, 100)]);
        assert_eq!(a.free_units(), 100);
        assert_eq!(a.alloc(100), Some(0));
        assert_eq!(a.alloc(1), None);
    }

    #[test]
    fn free_order_does_not_fragment() {
        let mut a = RangeAlloc::new(64);
        let s: Vec<_> = (0..8).map(|_| a.alloc(8).unwrap()).collect();
        for i in [3, 1, 5, 2, 0, 7, 4, 6] {
            a.free(s[i], 8);
        }
        assert_eq!(a.free, [(0, 64)]);
    }
}
