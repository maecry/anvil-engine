//! Debug geometry (ADR-0013 §10): lines, boxes, spheres and frusta collected during the frame and
//! drawn by the `debug.lines` graph pass after the transparent pass (depth tested, no depth write).

use bytemuck::{Pod, Zeroable};
use foundation::math::{Mat4, Vec3, Vec4};

/// Most vertices (two per line) drawn per frame; further lines are dropped (counted, one
/// warning). shortcut: fixed 2 MiB of the upload ring, upgrade with a growable debug buffer.
pub(crate) const MAX_VERTICES: usize = 1 << 17;

/// One line end: position and RGBA8 color (`debug.vert`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub(crate) struct DebugVertex {
    pub pos: [f32; 3],
    pub color: u32,
}

/// The per-frame debug line buffer, from [`Renderer::debug_lines`](crate::Renderer::debug_lines).
/// Lines added before `render` are drawn in that frame, then the buffer is cleared.
#[derive(Default)]
pub struct DebugLines {
    verts: Vec<DebugVertex>,
    dropped: u32,
}

/// Linear color in 0..1 to RGBA8 (alpha 1).
fn pack(color: Vec3) -> u32 {
    let c = (color.clamp(Vec3::ZERO, Vec3::ONE) * 255.0 + 0.5).as_uvec3();
    c.x | (c.y << 8) | (c.z << 16) | (255 << 24)
}

impl DebugLines {
    /// One line from `a` to `b` in world space; `color` is linear RGB in 0..1.
    pub fn line(&mut self, a: Vec3, b: Vec3, color: Vec3) {
        if self.verts.len() + 2 > MAX_VERTICES {
            self.dropped += 1;
            return;
        }
        let c = pack(color);
        self.verts.push(DebugVertex {
            pos: a.to_array(),
            color: c,
        });
        self.verts.push(DebugVertex {
            pos: b.to_array(),
            color: c,
        });
    }

    /// The 12 edges of the axis-aligned box `min`..`max`.
    pub fn aabb(&mut self, min: Vec3, max: Vec3, color: Vec3) {
        let corner = |i: usize| {
            Vec3::new(
                if i & 1 == 0 { min.x } else { max.x },
                if i & 2 == 0 { min.y } else { max.y },
                if i & 4 == 0 { min.z } else { max.z },
            )
        };
        for i in 0..8 {
            for bit in [1, 2, 4] {
                if i & bit == 0 {
                    self.line(corner(i), corner(i | bit), color);
                }
            }
        }
    }

    /// Three orthogonal great circles (24 segments each) of a sphere.
    pub fn sphere(&mut self, center: Vec3, radius: f32, color: Vec3) {
        self.circle(center, Vec3::X * radius, Vec3::Y * radius, color);
        self.circle(center, Vec3::Y * radius, Vec3::Z * radius, color);
        self.circle(center, Vec3::Z * radius, Vec3::X * radius, color);
    }

    /// A circle through `center + u` and `center + v` (24 segments).
    pub fn circle(&mut self, center: Vec3, u: Vec3, v: Vec3, color: Vec3) {
        const SEGMENTS: usize = 24;
        let at = |i: usize| {
            let a = i as f32 / SEGMENTS as f32 * std::f32::consts::TAU;
            center + u * a.cos() + v * a.sin()
        };
        for i in 0..SEGMENTS {
            self.line(at(i), at(i + 1), color);
        }
    }

    /// The edges of the clip volume of `view_proj` (clip depth 0..1): its 8 corners unprojected.
    /// A volume with an infinite far plane (the camera) draws nothing.
    pub fn frustum(&mut self, view_proj: Mat4, color: Vec3) {
        let inv = view_proj.inverse();
        let mut corners = [Vec3::ZERO; 8];
        for (i, c) in corners.iter_mut().enumerate() {
            let ndc = Vec4::new(
                if i & 1 == 0 { -1.0 } else { 1.0 },
                if i & 2 == 0 { -1.0 } else { 1.0 },
                if i & 4 == 0 { 0.0 } else { 1.0 },
                1.0,
            );
            let p = inv * ndc;
            if p.w.abs() < 1e-9 {
                return;
            }
            *c = p.truncate() / p.w;
        }
        for i in 0..8 {
            for bit in [1, 2, 4] {
                if i & bit == 0 {
                    self.line(corners[i], corners[i | bit], color);
                }
            }
        }
    }

    /// Number of vertices queued (two per line).
    pub fn vertex_count(&self) -> usize {
        self.verts.len()
    }

    /// The queued vertices; the buffer is left empty by [`DebugLines::take`].
    pub(crate) fn take(&mut self) -> Vec<DebugVertex> {
        if self.dropped > 0 {
            tracing::warn!(
                dropped = self.dropped,
                max_vertices = MAX_VERTICES,
                "debug lines dropped"
            );
            self.dropped = 0;
        }
        std::mem::take(&mut self.verts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpers_emit_the_expected_line_counts() {
        let mut d = DebugLines::default();
        d.line(Vec3::ZERO, Vec3::X, Vec3::ONE);
        d.aabb(Vec3::ZERO, Vec3::ONE, Vec3::ONE);
        d.sphere(Vec3::ZERO, 1.0, Vec3::ONE);
        assert_eq!(d.vertex_count(), 2 * (1 + 12 + 72));
        // An orthographic volume unprojects to its box; the camera's infinite far plane draws nothing.
        let ortho = crate::shadow::orthographic(-1.0, 1.0, -2.0, 2.0, 0.0, 4.0);
        let before = d.vertex_count();
        d.frustum(ortho, Vec3::ONE);
        assert_eq!(d.vertex_count() - before, 24);
        let inf =
            foundation::math::camera::rh::proj::vulkan::perspective_infinite_reverse(1.0, 1.0, 0.1);
        let before = d.vertex_count();
        d.frustum(inf, Vec3::ONE);
        assert_eq!(d.vertex_count(), before);
    }

    #[test]
    fn take_clears_and_the_cap_drops() {
        let mut d = DebugLines::default();
        for _ in 0..MAX_VERTICES {
            d.line(Vec3::ZERO, Vec3::X, Vec3::ONE);
        }
        assert_eq!(d.vertex_count(), MAX_VERTICES);
        assert_eq!(d.take().len(), MAX_VERTICES);
        assert_eq!(d.vertex_count(), 0);
        assert_eq!(pack(Vec3::new(1.0, 0.0, 0.5)), 0xff80_00ff);
    }
}
