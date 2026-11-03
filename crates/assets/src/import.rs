//! Importers: source file -> CPU assets. Pure functions of the file, never panic on bad input
//! (the caller also catches unwinds as a last resort).

use std::path::Path;
use std::sync::Arc;

use serde::Deserialize;

use crate::{
    AlphaModeAsset, Asset, AssetError, AssetId, MATERIAL_VERSION, MaterialAsset, MeshAsset,
    PrefabAsset, PrefabMesh, PrefabNode, SCENE_VERSION, SceneAsset, TextureAsset,
};

/// One asset produced by an import. The first of a bundle is the file's own asset.
pub(crate) struct Imported {
    pub id: AssetId,
    pub name: String,
    pub asset: Asset,
}

/// Imports `path` (already read into `bytes`; glTF re-reads it to resolve external buffers).
pub(crate) fn import(
    path: &Path,
    importer: &str,
    id: AssetId,
    bytes: &[u8],
) -> Result<Vec<Imported>, AssetError> {
    let one = |asset| {
        vec![Imported {
            id,
            name: String::new(),
            asset,
        }]
    };
    Ok(match importer {
        "gltf" => import_gltf(path, id)?,
        "image" => one(Asset::Texture(Arc::new(import_image(bytes)?))),
        "ktx2" => one(Asset::Texture(Arc::new(import_ktx2(bytes)?))),
        "material" => one(Asset::Material(Arc::new(import_material(bytes)?))),
        "scene" => one(Asset::Scene(Arc::new(import_scene(bytes)?))),
        other => return Err(AssetError::Format(format!("unknown importer {other}"))),
    })
}

fn import_image(bytes: &[u8]) -> Result<TextureAsset, AssetError> {
    let img = image::load_from_memory(bytes).map_err(|e| AssetError::Format(e.to_string()))?;
    // shortcut: EXR/HDR is clamped to 8-bit LDR RGBA, upgrade when the renderer takes float textures.
    let rgba = img.to_rgba8();
    Ok(TextureAsset {
        width: rgba.width(),
        height: rgba.height(),
        rgba8: rgba.into_raw(),
    })
}

fn import_ktx2(bytes: &[u8]) -> Result<TextureAsset, AssetError> {
    let fmt = |m: &str| AssetError::Format(m.to_owned());
    let r = ktx2::Reader::new(bytes).map_err(|e| AssetError::Format(format!("ktx2: {e:?}")))?;
    let h = r.header();
    // shortcut: only uncompressed RGBA8 level 0 is read; BCn/ASTC/Basis and supercompression are rejected until the RHI has compressed formats.
    if h.supercompression_scheme.is_some()
        || h.pixel_depth > 0
        || h.face_count != 1
        || h.layer_count > 1
        || h.pixel_height == 0
    {
        return Err(fmt(
            "ktx2: supercompressed, 3D, cube or array textures are not supported",
        ));
    }
    if h.format != Some(ktx2::Format::R8G8B8A8_UNORM)
        && h.format != Some(ktx2::Format::R8G8B8A8_SRGB)
    {
        return Err(AssetError::Format(format!(
            "ktx2: format {:?} is not uncompressed RGBA8",
            h.format
        )));
    }
    let level = r.levels().next().ok_or_else(|| fmt("ktx2: no levels"))?;
    let expect = u64::from(h.pixel_width) * u64::from(h.pixel_height) * 4;
    if level.data.len() as u64 != expect {
        return Err(AssetError::Corrupt("ktx2: level size mismatch".into()));
    }
    Ok(TextureAsset {
        width: h.pixel_width,
        height: h.pixel_height,
        rgba8: level.data.to_vec(),
    })
}

fn import_material(bytes: &[u8]) -> Result<MaterialAsset, AssetError> {
    #[derive(Deserialize)]
    struct Head {
        version: u32,
    }
    let text = std::str::from_utf8(bytes).map_err(|e| AssetError::Format(e.to_string()))?;
    let head: Head = ron::from_str(text).map_err(|e| AssetError::Format(e.to_string()))?;
    if head.version > MATERIAL_VERSION {
        return Err(AssetError::Version {
            found: head.version,
            supported: MATERIAL_VERSION,
        });
    }
    ron::from_str(text).map_err(|e| AssetError::Format(e.to_string()))
}

fn import_scene(bytes: &[u8]) -> Result<SceneAsset, AssetError> {
    #[derive(Deserialize)]
    struct MeshRenderer {
        mesh: AssetId,
        material: AssetId,
    }
    #[derive(Deserialize)]
    struct Entity {
        #[serde(default)]
        mesh_renderer: Option<MeshRenderer>,
    }
    #[derive(Deserialize)]
    struct Head {
        version: u32,
        #[serde(default)]
        entities: Vec<Entity>,
    }
    let text = std::str::from_utf8(bytes).map_err(|e| AssetError::Format(e.to_string()))?;
    let head: Head = ron::from_str(text).map_err(|e| AssetError::Format(e.to_string()))?;
    if head.version > SCENE_VERSION {
        return Err(AssetError::Version {
            found: head.version,
            supported: SCENE_VERSION,
        });
    }
    let mut deps: Vec<AssetId> = head
        .entities
        .iter()
        .filter_map(|e| e.mesh_renderer.as_ref())
        .flat_map(|m| [m.mesh, m.material])
        .filter(|i| !i.is_nil())
        .collect();
    deps.sort();
    deps.dedup();
    Ok(SceneAsset {
        text: text.to_owned(),
        deps,
    })
}

fn gltf_err(e: gltf::Error) -> AssetError {
    match e {
        gltf::Error::Io(e) => AssetError::Io(e.to_string()),
        e => AssetError::Format(format!("gltf: {e}")),
    }
}

/// Sub-asset ids/names of a glTF file.
fn mesh_name(mesh: usize, prim: usize) -> String {
    format!("mesh/{mesh}/{prim}")
}

fn import_gltf(path: &Path, root: AssetId) -> Result<Vec<Imported>, AssetError> {
    // shortcut: the cache key covers the .gltf/.glb file only, edits to external .bin/.png files are not noticed until the main file changes.
    let (doc, buffers, images) = gltf::import(path).map_err(gltf_err)?;
    let mut out = vec![Imported {
        id: root,
        name: String::new(),
        asset: Asset::Prefab(Arc::default()),
    }];
    let sub = |name: &str| AssetId::derive(root, name);

    for (i, img) in images.iter().enumerate() {
        let (w, h) = (img.width as usize, img.height as usize);
        let px = &img.pixels;
        let rgba8: Vec<u8> = match img.format {
            gltf::image::Format::R8G8B8A8 => px.clone(),
            gltf::image::Format::R8G8B8 => px
                .as_chunks::<3>()
                .0
                .iter()
                .flat_map(|c| [c[0], c[1], c[2], 255])
                .collect(),
            gltf::image::Format::R8G8 => px
                .as_chunks::<2>()
                .0
                .iter()
                .flat_map(|c| [c[0], c[1], 0, 255])
                .collect(),
            gltf::image::Format::R8 => px.iter().flat_map(|&c| [c, c, c, 255]).collect(),
            f => {
                return Err(AssetError::Format(format!(
                    "gltf image format {f:?} unsupported"
                )));
            }
        };
        if w == 0 || h == 0 || rgba8.len() != w * h * 4 {
            return Err(AssetError::Corrupt(format!("gltf image {i} has bad size")));
        }
        let name = format!("image/{i}");
        out.push(Imported {
            id: sub(&name),
            name,
            asset: Asset::Texture(Arc::new(TextureAsset {
                width: img.width,
                height: img.height,
                rgba8,
            })),
        });
    }

    let tex = |t: Option<gltf::texture::Info>| {
        t.map(|t| sub(&format!("image/{}", t.texture().source().index())))
    };
    for (i, m) in doc.materials().enumerate() {
        let Some(_) = m.index() else { continue };
        let pbr = m.pbr_metallic_roughness();
        let tex_id = |idx: usize| Some(sub(&format!("image/{idx}")));
        let name = format!("material/{i}");
        out.push(Imported {
            id: sub(&name),
            name,
            asset: Asset::Material(Arc::new(MaterialAsset {
                version: MATERIAL_VERSION,
                base_color: pbr.base_color_factor(),
                base_color_texture: tex(pbr.base_color_texture()),
                metallic: pbr.metallic_factor(),
                roughness: pbr.roughness_factor(),
                metallic_roughness_texture: tex(pbr.metallic_roughness_texture()),
                normal_texture: m
                    .normal_texture()
                    .and_then(|t| tex_id(t.texture().source().index())),
                normal_scale: m.normal_texture().map_or(1.0, |t| t.scale()),
                occlusion_texture: m
                    .occlusion_texture()
                    .and_then(|t| tex_id(t.texture().source().index())),
                occlusion_strength: m.occlusion_texture().map_or(1.0, |t| t.strength()),
                emissive: m.emissive_factor(),
                emissive_texture: tex(m.emissive_texture()),
                alpha_mode: match m.alpha_mode() {
                    gltf::material::AlphaMode::Opaque => AlphaModeAsset::Opaque,
                    gltf::material::AlphaMode::Mask => {
                        AlphaModeAsset::Mask(m.alpha_cutoff().unwrap_or(0.5))
                    }
                    gltf::material::AlphaMode::Blend => AlphaModeAsset::Blend,
                },
                double_sided: m.double_sided(),
            })),
        });
    }

    // Per glTF mesh: the (mesh id, material id) of each triangle primitive.
    let mut prims: Vec<Vec<PrefabMesh>> = Vec::new();
    for (mi, mesh) in doc.meshes().enumerate() {
        let mut list = Vec::new();
        for (pi, prim) in mesh.primitives().enumerate() {
            if prim.mode() != gltf::mesh::Mode::Triangles {
                tracing::warn!(path = %path.display(), mesh = mi, "non-triangle primitive skipped");
                continue;
            }
            let reader = prim.reader(|b| buffers.get(b.index()).map(|d| &d.0[..]));
            let positions: Vec<[f32; 3]> = reader
                .read_positions()
                .ok_or_else(|| AssetError::Format(format!("mesh {mi} has no positions")))?
                .collect();
            let n = positions.len();
            let indices: Vec<u32> = match reader.read_indices() {
                Some(i) => i.into_u32().collect(),
                None => (0..n as u32).collect(),
            };
            if !indices.len().is_multiple_of(3) || indices.iter().any(|&i| i as usize >= n) {
                return Err(AssetError::Corrupt(format!("mesh {mi} has bad indices")));
            }
            let uvs: Vec<[f32; 2]> = reader
                .read_tex_coords(0)
                .map(|t| t.into_f32().collect())
                .unwrap_or_else(|| vec![[0.0; 2]; n]);
            let normals: Vec<[f32; 3]> = reader
                .read_normals()
                .map(|v| v.collect())
                .unwrap_or_else(|| gen_normals(&positions, &indices));
            let tangents: Vec<[f32; 4]> = reader
                .read_tangents()
                .map(|v| v.collect())
                .unwrap_or_else(|| gen_tangents(&positions, &normals, &uvs, &indices));
            if uvs.len() != n || normals.len() != n || tangents.len() != n {
                return Err(AssetError::Corrupt(format!(
                    "mesh {mi} attribute counts differ"
                )));
            }
            let name = mesh_name(mi, pi);
            let id = sub(&name);
            out.push(Imported {
                id,
                name,
                asset: Asset::Mesh(Arc::new(MeshAsset {
                    positions,
                    normals,
                    uvs,
                    tangents,
                    indices,
                })),
            });
            let material = prim
                .material()
                .index()
                .map_or(AssetId::NIL, |m| sub(&format!("material/{m}")));
            list.push(PrefabMesh { mesh: id, material });
        }
        prims.push(list);
    }

    let mut nodes = Vec::new();
    if let Some(scene) = doc.default_scene().or_else(|| doc.scenes().next()) {
        for n in scene.nodes() {
            walk(n, None, 0, &prims, &mut nodes)?;
        }
    }
    out[0].asset = Asset::Prefab(Arc::new(PrefabAsset { nodes }));
    Ok(out)
}

fn walk(
    node: gltf::Node,
    parent: Option<u32>,
    depth: u32,
    prims: &[Vec<PrefabMesh>],
    out: &mut Vec<PrefabNode>,
) -> Result<(), AssetError> {
    if depth > 128 {
        return Err(AssetError::Corrupt("node hierarchy too deep".into()));
    }
    let (t, r, s) = node.transform().decomposed();
    let name = node.name().unwrap_or("node").to_owned();
    let list = node
        .mesh()
        .and_then(|m| prims.get(m.index()))
        .map_or(&[][..], |v| &v[..]);
    let me = out.len() as u32;
    out.push(PrefabNode {
        name: name.clone(),
        parent,
        translation: t,
        rotation: r,
        scale: s,
        mesh: list.first().copied(),
    });
    // Extra primitives of a multi-primitive mesh become identity children.
    for (i, extra) in list.iter().enumerate().skip(1) {
        out.push(PrefabNode {
            name: format!("{name}.{i}"),
            parent: Some(me),
            translation: [0.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
            mesh: Some(*extra),
        });
    }
    for c in node.children() {
        walk(c, Some(me), depth + 1, prims, out)?;
    }
    Ok(())
}

/// Area-weighted smooth normals.
fn gen_normals(pos: &[[f32; 3]], idx: &[u32]) -> Vec<[f32; 3]> {
    use foundation::math::Vec3;
    let mut n = vec![Vec3::ZERO; pos.len()];
    for t in idx.as_chunks::<3>().0 {
        let p = |i: u32| Vec3::from(pos[i as usize]);
        let f = (p(t[1]) - p(t[0])).cross(p(t[2]) - p(t[0]));
        for &i in t {
            n[i as usize] += f;
        }
    }
    n.into_iter()
        .map(|v| v.try_normalize().unwrap_or(Vec3::Y).to_array())
        .collect()
}

/// Per-vertex tangents from the uv gradients (xyz) and the bitangent sign (w).
fn gen_tangents(pos: &[[f32; 3]], nrm: &[[f32; 3]], uv: &[[f32; 2]], idx: &[u32]) -> Vec<[f32; 4]> {
    use foundation::math::{Vec2, Vec3};
    let mut tan = vec![Vec3::ZERO; pos.len()];
    let mut bit = vec![Vec3::ZERO; pos.len()];
    for t in idx.as_chunks::<3>().0 {
        let (a, b, c) = (t[0] as usize, t[1] as usize, t[2] as usize);
        let (e1, e2) = (
            Vec3::from(pos[b]) - Vec3::from(pos[a]),
            Vec3::from(pos[c]) - Vec3::from(pos[a]),
        );
        let (d1, d2) = (
            Vec2::from(uv[b]) - Vec2::from(uv[a]),
            Vec2::from(uv[c]) - Vec2::from(uv[a]),
        );
        let det = d1.x * d2.y - d2.x * d1.y;
        if det.abs() < 1e-12 {
            continue;
        }
        let r = 1.0 / det;
        let (tg, bt) = ((e1 * d2.y - e2 * d1.y) * r, (e2 * d1.x - e1 * d2.x) * r);
        for i in [a, b, c] {
            tan[i] += tg;
            bit[i] += bt;
        }
    }
    (0..pos.len())
        .map(|i| {
            let n = Vec3::from(nrm[i]);
            let t = (tan[i] - n * n.dot(tan[i]))
                .try_normalize()
                .unwrap_or_else(|| n.any_orthonormal_vector());
            let w = if n.cross(t).dot(bit[i]) < 0.0 {
                -1.0
            } else {
                1.0
            };
            [t.x, t.y, t.z, w]
        })
        .collect()
}
