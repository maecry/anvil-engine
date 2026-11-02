//! Mesh vertex format and built-in procedural meshes.

use foundation::math::Vec3;

/// The one vertex layout of every mesh (ADR-0011 §4); 48 bytes, read by the shaders through BDA.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    /// Object-space position.
    pub pos: [f32; 3],
    /// Object-space unit normal.
    pub normal: [f32; 3],
    /// Texture coordinates (origin top-left, glTF convention).
    pub uv: [f32; 2],
    /// Object-space tangent (xyz) along +u and bitangent sign (w).
    pub tangent: [f32; 4],
}

/// CPU-side mesh: vertices plus a triangle list (counter-clockwise front faces).
#[derive(Clone, Debug, Default)]
pub struct MeshData {
    /// Vertices.
    pub vertices: Vec<Vertex>,
    /// Triangle list indices into `vertices`.
    pub indices: Vec<u32>,
}

/// Appends a planar quad with unit normal `n`, centered at `n * dist` and spanned by the unit
/// vectors `t` (+u) and `b` (+v) with half-extent `half`. Winding is counter-clockwise seen from `n`.
fn push_quad(m: &mut MeshData, n: Vec3, t: Vec3, b: Vec3, dist: f32, half: f32) {
    let w = n.cross(t).dot(b).signum();
    let base = m.vertices.len() as u32;
    for (su, sv) in [(-1.0f32, -1.0f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
        let p = n * dist + (t * su + b * sv) * half;
        m.vertices.push(Vertex {
            pos: p.to_array(),
            normal: n.to_array(),
            uv: [su * 0.5 + 0.5, sv * 0.5 + 0.5],
            tangent: [t.x, t.y, t.z, w],
        });
    }
    // (−,−) (+,−) (+,+) (−,+) is counter-clockwise from `n` iff t x b points along n.
    let ccw = t.cross(b).dot(n) > 0.0;
    let q = if ccw {
        [0, 1, 2, 0, 2, 3]
    } else {
        [0, 2, 1, 0, 3, 2]
    };
    m.indices.extend(q.iter().map(|i| base + i));
}

/// A square in the XZ plane facing +Y, `size` wide, centered on the origin.
pub fn plane(size: f32) -> MeshData {
    let mut m = MeshData::default();
    push_quad(&mut m, Vec3::Y, Vec3::X, Vec3::Z, 0.0, size * 0.5);
    m
}

/// A cube with edge length 1 centered on the origin (24 vertices, per-face normals).
pub fn cube() -> MeshData {
    let mut m = MeshData::default();
    for (n, t, b) in [
        (Vec3::X, -Vec3::Z, -Vec3::Y),
        (-Vec3::X, Vec3::Z, -Vec3::Y),
        (Vec3::Y, Vec3::X, Vec3::Z),
        (-Vec3::Y, Vec3::X, -Vec3::Z),
        (Vec3::Z, Vec3::X, -Vec3::Y),
        (-Vec3::Z, -Vec3::X, -Vec3::Y),
    ] {
        push_quad(&mut m, n, t, b, 0.5, 0.5);
    }
    m
}

/// A UV sphere of radius 0.5 with `segments` around the Y axis and `rings` pole to pole.
/// `u` runs around the axis, `v` from the top pole (0) to the bottom pole (1).
///
/// # Panics
/// If `segments < 3` or `rings < 2`.
pub fn uv_sphere(segments: u32, rings: u32) -> MeshData {
    assert!(
        segments >= 3 && rings >= 2,
        "uv_sphere needs >= 3 segments and >= 2 rings"
    );
    let mut m = MeshData::default();
    for r in 0..=rings {
        let v = r as f32 / rings as f32;
        let theta = v * std::f32::consts::PI;
        for s in 0..=segments {
            let u = s as f32 / segments as f32;
            let phi = u * std::f32::consts::TAU;
            let n = Vec3::new(
                theta.sin() * phi.cos(),
                theta.cos(),
                theta.sin() * phi.sin(),
            );
            // d(pos)/du; degenerate at the poles, where any horizontal direction is valid.
            let t = Vec3::new(-phi.sin(), 0.0, phi.cos());
            // d(pos)/dv points toward the bottom pole.
            let dv = Vec3::new(
                theta.cos() * phi.cos(),
                -theta.sin(),
                theta.cos() * phi.sin(),
            );
            let w = n.cross(t).dot(dv).signum();
            m.vertices.push(Vertex {
                pos: (n * 0.5).to_array(),
                normal: n.to_array(),
                uv: [u, v],
                tangent: [t.x, t.y, t.z, if w == 0.0 { 1.0 } else { w }],
            });
        }
    }
    let stride = segments + 1;
    for r in 0..rings {
        for s in 0..segments {
            let a = r * stride + s;
            let b = a + stride;
            m.indices.extend([a, a + 1, b, a + 1, b + 1, b]);
        }
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(m: &MeshData) {
        assert_eq!(m.indices.len() % 3, 0);
        for tri in m.indices.chunks(3) {
            let [a, b, c] = [tri[0], tri[1], tri[2]].map(|i| m.vertices[i as usize]);
            let (pa, pb, pc) = (Vec3::from(a.pos), Vec3::from(b.pos), Vec3::from(c.pos));
            let face = (pb - pa).cross(pc - pa);
            if face.length() < 1e-6 {
                continue; // pole sliver
            }
            assert!(
                face.dot(Vec3::from(a.normal)) > 0.0,
                "triangle faces inward"
            );
        }
        for v in &m.vertices {
            let (n, t) = (Vec3::from(v.normal), Vec3::from_slice(&v.tangent));
            assert!((n.length() - 1.0).abs() < 1e-4 && (t.length() - 1.0).abs() < 1e-4);
            assert!(n.dot(t).abs() < 1e-4, "tangent not orthogonal to normal");
            assert!(v.tangent[3] == 1.0 || v.tangent[3] == -1.0);
        }
    }

    #[test]
    fn builtin_meshes_are_wound_outward_with_orthogonal_tangents() {
        check(&plane(10.0));
        check(&cube());
        check(&uv_sphere(24, 12));
    }

    #[test]
    fn mesh_counts() {
        assert_eq!((cube().vertices.len(), cube().indices.len()), (24, 36));
        let s = uv_sphere(8, 4);
        assert_eq!((s.vertices.len(), s.indices.len()), (9 * 5, 8 * 4 * 6));
        assert_eq!(std::mem::size_of::<Vertex>(), 48);
    }
}
