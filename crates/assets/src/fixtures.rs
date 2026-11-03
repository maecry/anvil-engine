//! Generators for small glb and PNG files used by tests and the sample project (feature
//! `fixtures`). No third-party content: everything is computed here.

/// One mesh node of a generated glb.
pub struct FixtureMesh {
    /// Node and mesh name.
    pub name: &'static str,
    /// Node translation.
    pub translation: [f32; 3],
    /// Positions.
    pub positions: Vec<[f32; 3]>,
    /// Normals.
    pub normals: Vec<[f32; 3]>,
    /// Texture coordinates.
    pub uvs: Vec<[f32; 2]>,
    /// Triangle indices.
    pub indices: Vec<u16>,
    /// Index into the materials list.
    pub material: Option<usize>,
}

/// One material of a generated glb.
pub struct FixtureMaterial {
    /// Base color factor.
    pub base_color: [f32; 4],
    /// Uses the embedded PNG as base color texture.
    pub textured: bool,
    /// Metallic factor.
    pub metallic: f32,
    /// Roughness factor.
    pub roughness: f32,
}

/// Unit cube (edge `size`) with per-face normals and uvs, centered on the origin.
pub fn cube(size: f32) -> FixtureMesh {
    let h = size * 0.5;
    let mut m = FixtureMesh {
        name: "cube",
        translation: [0.0; 3],
        positions: vec![],
        normals: vec![],
        uvs: vec![],
        indices: vec![],
        material: None,
    };
    // (normal, tangent u axis, v axis)
    for (n, u, v) in [
        ([0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
        ([0.0, 0.0, -1.0], [-1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
        ([1.0, 0.0, 0.0], [0.0, 0.0, -1.0], [0.0, 1.0, 0.0]),
        ([-1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0]),
        ([0.0, 1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, -1.0]),
        ([0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
    ] {
        let base = m.positions.len() as u16;
        for (a, b) in [(-1.0f32, -1.0f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
            m.positions
                .push(std::array::from_fn(|i| (n[i] + u[i] * a + v[i] * b) * h));
            m.normals.push(n);
            m.uvs.push([(a + 1.0) * 0.5, 1.0 - (b + 1.0) * 0.5]);
        }
        m.indices.extend([0, 1, 2, 0, 2, 3].map(|i| base + i));
    }
    m
}

/// Horizontal quad (edge `size`) facing +Y, uvs repeated `tiles` times.
pub fn quad(size: f32, tiles: f32) -> FixtureMesh {
    let h = size * 0.5;
    FixtureMesh {
        name: "ground",
        translation: [0.0; 3],
        positions: vec![[-h, 0.0, h], [h, 0.0, h], [h, 0.0, -h], [-h, 0.0, -h]],
        normals: vec![[0.0, 1.0, 0.0]; 4],
        uvs: vec![[0.0, tiles], [tiles, tiles], [tiles, 0.0], [0.0, 0.0]],
        indices: vec![0, 1, 2, 0, 2, 3],
        material: None,
    }
}

/// A two-color checker as PNG bytes.
pub fn checker_png(size: u32, cell: u32, a: [u8; 3], b: [u8; 3]) -> Vec<u8> {
    let img = image::RgbImage::from_fn(size, size, |x, y| {
        image::Rgb(if ((x / cell) + (y / cell)).is_multiple_of(2) {
            a
        } else {
            b
        })
    });
    let mut out = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .expect("encode png into memory");
    out
}

fn pad4(v: &mut Vec<u8>, byte: u8) {
    while !v.len().is_multiple_of(4) {
        v.push(byte);
    }
}

/// Builds a binary glTF with one node per mesh, the given materials and an optional embedded
/// PNG (texture 0 / image 0).
pub fn glb(meshes: &[FixtureMesh], materials: &[FixtureMaterial], png: Option<&[u8]>) -> Vec<u8> {
    let mut bin = Vec::new();
    let (mut views, mut accessors, mut mesh_json, mut node_json) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let mut view = |bin: &mut Vec<u8>, bytes: &[u8], target: Option<u32>| {
        pad4(bin, 0);
        let offset = bin.len();
        bin.extend_from_slice(bytes);
        let t = target.map_or(String::new(), |t| format!(r#","target":{t}"#));
        views.push(format!(
            r#"{{"buffer":0,"byteOffset":{offset},"byteLength":{}{t}}}"#,
            bytes.len()
        ));
        views.len() - 1
    };
    for (mi, m) in meshes.iter().enumerate() {
        let flat = |v: &[[f32; 3]]| -> Vec<u8> {
            v.iter().flatten().flat_map(|f| f.to_le_bytes()).collect()
        };
        let (min, max) = m
            .positions
            .iter()
            .fold(([f32::MAX; 3], [f32::MIN; 3]), |(lo, hi), p| {
                (
                    std::array::from_fn(|i| lo[i].min(p[i])),
                    std::array::from_fn(|i| hi[i].max(p[i])),
                )
            });
        let n = m.positions.len();
        let pv = view(&mut bin, &flat(&m.positions), Some(34962));
        let nv = view(&mut bin, &flat(&m.normals), Some(34962));
        let uv: Vec<u8> = m
            .uvs
            .iter()
            .flatten()
            .flat_map(|f| f.to_le_bytes())
            .collect();
        let tv = view(&mut bin, &uv, Some(34962));
        let idx: Vec<u8> = m.indices.iter().flat_map(|i| i.to_le_bytes()).collect();
        let iv = view(&mut bin, &idx, Some(34963));
        let base = accessors.len();
        accessors.push(format!(
            r#"{{"bufferView":{pv},"componentType":5126,"count":{n},"type":"VEC3","min":{min:?},"max":{max:?}}}"#
        ));
        accessors.push(format!(
            r#"{{"bufferView":{nv},"componentType":5126,"count":{n},"type":"VEC3"}}"#
        ));
        accessors.push(format!(
            r#"{{"bufferView":{tv},"componentType":5126,"count":{n},"type":"VEC2"}}"#
        ));
        accessors.push(format!(
            r#"{{"bufferView":{iv},"componentType":5123,"count":{},"type":"SCALAR"}}"#,
            m.indices.len()
        ));
        let mat = m
            .material
            .map_or(String::new(), |i| format!(r#","material":{i}"#));
        mesh_json.push(format!(
            r#"{{"name":"{}","primitives":[{{"attributes":{{"POSITION":{base},"NORMAL":{},"TEXCOORD_0":{}}},"indices":{}{mat}}}]}}"#,
            m.name,
            base + 1,
            base + 2,
            base + 3
        ));
        node_json.push(format!(
            r#"{{"name":"{}","mesh":{mi},"translation":{:?}}}"#,
            m.name, m.translation
        ));
    }
    let image_json = png.map(|p| {
        let v = view(&mut bin, p, None);
        format!(
            r#","images":[{{"bufferView":{v},"mimeType":"image/png"}}],"textures":[{{"source":0}}]"#
        )
    });
    pad4(&mut bin, 0);
    let mats: Vec<String> = materials
        .iter()
        .map(|m| {
            let tex = if m.textured { r#","baseColorTexture":{"index":0}"# } else { "" };
            format!(
                r#"{{"pbrMetallicRoughness":{{"baseColorFactor":{:?},"metallicFactor":{:?},"roughnessFactor":{:?}{tex}}}}}"#,
                m.base_color, m.metallic, m.roughness
            )
        })
        .collect();
    let nodes_idx: Vec<String> = (0..meshes.len()).map(|i| i.to_string()).collect();
    let json = format!(
        r#"{{"asset":{{"version":"2.0","generator":"anvil fixtures"}},"scene":0,"scenes":[{{"nodes":[{}]}}],"nodes":[{}],"meshes":[{}],"materials":[{}],"accessors":[{}],"bufferViews":[{}],"buffers":[{{"byteLength":{}}}]{}}}"#,
        nodes_idx.join(","),
        node_json.join(","),
        mesh_json.join(","),
        mats.join(","),
        accessors.join(","),
        views.join(","),
        bin.len(),
        image_json.unwrap_or_default()
    );
    let mut json = json.into_bytes();
    pad4(&mut json, b' ');
    let total = 12 + 8 + json.len() + 8 + bin.len();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(json.len() as u32).to_le_bytes());
    out.extend_from_slice(b"JSON");
    out.extend_from_slice(&json);
    out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
    out.extend_from_slice(b"BIN\0");
    out.extend_from_slice(&bin);
    out
}
