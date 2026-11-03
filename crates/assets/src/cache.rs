//! Imported-artifact cache: `<project>/.anvil/imported/<id>.bin`, a fixed header (magic,
//! importer version, source hash) followed by the bundle's entries. Binary, not serde, for the
//! bulk data (mesh and image bytes are copied raw); small text assets embed RON.

use std::sync::Arc;

use bytemuck::{Pod, Zeroable};

use crate::import::Imported;
use crate::{
    Asset, AssetError, AssetId, IMPORTER_VERSION, MaterialAsset, MeshAsset, PrefabAsset,
    SceneAsset, TextureAsset,
};

const MAGIC: [u8; 4] = *b"ANVC";

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Header {
    magic: [u8; 4],
    importer_version: u32,
    source_hash: u64,
    count: u32,
    pad: u32,
}

/// FNV-1a 64 of the source bytes.
// shortcut: a non-cryptographic 64-bit hash, upgrade if collisions between edits ever matter.
pub(crate) fn hash(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

fn put(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
    out.extend_from_slice(bytes);
}

fn ron_bytes<T: serde::Serialize>(v: &T) -> Vec<u8> {
    ron::to_string(v).unwrap_or_default().into_bytes()
}

/// Serializes a bundle with the given source hash.
pub(crate) fn encode(bundle: &[Imported], source_hash: u64) -> Vec<u8> {
    let header = Header {
        magic: MAGIC,
        importer_version: IMPORTER_VERSION,
        source_hash,
        count: bundle.len() as u32,
        pad: 0,
    };
    let mut out = bytemuck::bytes_of(&header).to_vec();
    for item in bundle {
        out.extend_from_slice(&item.id.0.to_le_bytes());
        put(&mut out, item.name.as_bytes());
        let mut payload = Vec::new();
        let tag: u8 = match &item.asset {
            Asset::Mesh(m) => {
                for b in [
                    bytemuck::cast_slice::<_, u8>(&m.positions),
                    bytemuck::cast_slice(&m.normals),
                    bytemuck::cast_slice(&m.uvs),
                    bytemuck::cast_slice(&m.tangents),
                    bytemuck::cast_slice(&m.indices),
                ] {
                    put(&mut payload, b);
                }
                0
            }
            Asset::Texture(t) => {
                payload.extend_from_slice(&t.width.to_le_bytes());
                payload.extend_from_slice(&t.height.to_le_bytes());
                payload.extend_from_slice(&t.rgba8);
                1
            }
            Asset::Material(m) => {
                payload = ron_bytes(&**m);
                2
            }
            Asset::Prefab(p) => {
                payload = ron_bytes(&**p);
                3
            }
            Asset::Scene(s) => {
                payload = ron_bytes(&**s);
                4
            }
        };
        out.push(tag);
        put(&mut out, &payload);
    }
    out
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], AssetError> {
        if self.0.len() < n {
            return Err(AssetError::Corrupt("cache entry truncated".into()));
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Ok(a)
    }

    fn u32(&mut self) -> Result<u32, AssetError> {
        Ok(u32::from_le_bytes(
            self.take(4)?.try_into().unwrap_or_default(),
        ))
    }

    fn block(&mut self) -> Result<&'a [u8], AssetError> {
        let n = u64::from_le_bytes(self.take(8)?.try_into().unwrap_or_default());
        self.take(usize::try_from(n).map_err(|_| AssetError::Corrupt("cache size".into()))?)
    }
}

fn pods<T: Pod>(b: &[u8]) -> Result<Vec<T>, AssetError> {
    if !b.len().is_multiple_of(size_of::<T>()) {
        return Err(AssetError::Corrupt("cache array size".into()));
    }
    Ok(b.chunks_exact(size_of::<T>())
        .map(bytemuck::pod_read_unaligned)
        .collect())
}

fn ron_of<T: serde::de::DeserializeOwned>(b: &[u8]) -> Result<T, AssetError> {
    std::str::from_utf8(b)
        .map_err(|e| AssetError::Corrupt(e.to_string()))
        .and_then(|s| ron::from_str(s).map_err(|e| AssetError::Corrupt(e.to_string())))
}

/// Decodes a cache file. `Err` for anything that is not a valid entry for this importer
/// version and `source_hash` (the caller reimports).
pub(crate) fn decode(bytes: &[u8], source_hash: u64) -> Result<Vec<Imported>, AssetError> {
    let hs = size_of::<Header>();
    if bytes.len() < hs {
        return Err(AssetError::Corrupt("cache header truncated".into()));
    }
    let header: Header = bytemuck::pod_read_unaligned(&bytes[..hs]);
    if header.magic != MAGIC
        || header.importer_version != IMPORTER_VERSION
        || header.source_hash != source_hash
    {
        return Err(AssetError::Corrupt("cache stale".into()));
    }
    let mut r = Reader(&bytes[hs..]);
    let mut out = Vec::new();
    for _ in 0..header.count {
        let id = AssetId(u128::from_le_bytes(
            r.take(16)?.try_into().unwrap_or_default(),
        ));
        let name = String::from_utf8_lossy(r.block()?).into_owned();
        let tag = r.take(1)?[0];
        let mut p = Reader(r.block()?);
        let asset = match tag {
            0 => Asset::Mesh(Arc::new(MeshAsset {
                positions: pods(p.block()?)?,
                normals: pods(p.block()?)?,
                uvs: pods(p.block()?)?,
                tangents: pods(p.block()?)?,
                indices: pods(p.block()?)?,
            })),
            1 => {
                let (width, height) = (p.u32()?, p.u32()?);
                Asset::Texture(Arc::new(TextureAsset {
                    width,
                    height,
                    rgba8: p.0.to_vec(),
                }))
            }
            2 => Asset::Material(Arc::new(ron_of::<MaterialAsset>(p.0)?)),
            3 => Asset::Prefab(Arc::new(ron_of::<PrefabAsset>(p.0)?)),
            4 => Asset::Scene(Arc::new(ron_of::<SceneAsset>(p.0)?)),
            _ => return Err(AssetError::Corrupt("cache entry kind".into())),
        };
        out.push(Imported { id, name, asset });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_stale() {
        let bundle = vec![
            Imported {
                id: AssetId(7),
                name: "mesh/0/0".into(),
                asset: Asset::Mesh(Arc::new(MeshAsset {
                    positions: vec![[1.0, 2.0, 3.0]; 3],
                    normals: vec![[0.0, 1.0, 0.0]; 3],
                    uvs: vec![[0.5, 0.5]; 3],
                    tangents: vec![[1.0, 0.0, 0.0, 1.0]; 3],
                    indices: vec![0, 1, 2],
                })),
            },
            Imported {
                id: AssetId(8),
                name: String::new(),
                asset: Asset::Texture(Arc::new(TextureAsset {
                    width: 1,
                    height: 1,
                    rgba8: vec![1, 2, 3, 4],
                })),
            },
        ];
        let bytes = encode(&bundle, 42);
        let back = decode(&bytes, 42).unwrap();
        assert_eq!(back.len(), 2);
        match (&back[0].asset, &bundle[0].asset) {
            (Asset::Mesh(a), Asset::Mesh(b)) => assert_eq!(a, b),
            _ => panic!("kind"),
        }
        assert!(decode(&bytes, 43).is_err(), "source hash mismatch");
        assert!(decode(&bytes[..bytes.len() - 3], 42).is_err(), "truncated");
    }
}
