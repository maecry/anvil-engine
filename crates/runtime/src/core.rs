//! The systems half of the runtime: world, assets, the asset-to-GPU bridge and the extract.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use assets::{Asset, AssetEvent, AssetId, AssetServer, LoadState, MaterialAsset, MeshAsset};
use foundation::math::Vec3;
use renderer::{
    AlphaMode, Camera, Environment, Light, MaterialDesc, MaterialId, MeshData, MeshId,
    MeshInstance, RenderScene, Renderer, TextureId, Vertex,
};
use rhi::Format;
use rhi_vulkan::VulkanDevice;
use scene::{GlobalTransform, LightKind, MeshRenderer, SceneFile, World};

use crate::RuntimeError;

/// World + assets + renderer glue. No window, no loop: the player's [`crate::Runtime`] and the
/// editor both drive it. Call order per frame: [`update_assets`](Self::update_assets), your own
/// logic, [`run_systems`](Self::run_systems), [`extract`](Self::extract), then render
/// `core.renderer` yourself.
pub struct RuntimeCore {
    /// The renderer (also owns the fallback resources).
    pub renderer: Renderer,
    /// Entities.
    pub world: World,
    /// Asset registry and loader.
    pub assets: AssetServer,
    // Fallbacks are stored in these maps for failed assets so they are not retried every frame;
    // events remove the entry.
    meshes: HashMap<AssetId, MeshId>,
    materials: HashMap<AssetId, MaterialId>,
    textures: HashMap<(AssetId, bool), TextureId>,
    pending_scene: Option<AssetId>,
    warned_no_camera: bool,
}

impl RuntimeCore {
    /// Creates the renderer for targets of `format` and `extent`.
    ///
    /// # Errors
    /// [`RuntimeError::Render`] / [`RuntimeError::Rhi`] from renderer creation.
    pub fn new(
        device: &VulkanDevice,
        format: Format,
        extent: (u32, u32),
        assets: AssetServer,
    ) -> Result<Self, RuntimeError> {
        Ok(Self {
            renderer: Renderer::new(device, format, extent)?,
            world: World::new(),
            assets,
            meshes: HashMap::new(),
            materials: HashMap::new(),
            textures: HashMap::new(),
            pending_scene: None,
            warned_no_camera: false,
        })
    }

    /// Starts loading the scene at `path` (relative to the project); its entities are spawned
    /// into the world by a later [`update_assets`](Self::update_assets) once it is loaded.
    ///
    /// # Errors
    /// [`RuntimeError::Asset`] for an unknown path.
    pub fn open_scene(&mut self, path: &str) -> Result<(), RuntimeError> {
        self.pending_scene = Some(self.assets.load_path(path)?);
        Ok(())
    }

    /// True while a requested scene has not been spawned yet.
    pub fn scene_pending(&self) -> bool {
        self.pending_scene.is_some()
    }

    /// Applies finished imports: reacts to events (drops stale GPU copies, updates reloaded
    /// materials in place) and spawns the requested scene when it is ready.
    pub fn update_assets(&mut self, device: &VulkanDevice) {
        let events = self.assets.update();
        let mut old_textures = Vec::new();
        for ev in events {
            let (AssetEvent::Loaded(id) | AssetEvent::Reloaded(id) | AssetEvent::Failed(id)) = ev;
            self.invalidate(
                device,
                id,
                matches!(ev, AssetEvent::Reloaded(_)),
                &mut old_textures,
            );
        }
        // A texture can reload without its materials doing so: repoint every material before the
        // replaced images are destroyed.
        if !old_textures.is_empty() {
            let mats: Vec<_> = self.materials.iter().map(|(a, m)| (*a, *m)).collect();
            for (id, mat) in mats {
                if mat == self.renderer.missing_material()
                    || mat == self.renderer.default_material()
                {
                    continue;
                }
                if let Some(Asset::Material(m)) = self.assets.get(id) {
                    let desc = self.material_desc(device, &m);
                    self.renderer.update_material(mat, &desc);
                }
            }
        }
        for t in old_textures {
            self.renderer.destroy_texture(device, t);
        }
        let Some(id) = self.pending_scene else { return };
        match self.assets.state(id) {
            Some(LoadState::Loaded) => {
                self.pending_scene = None;
                if let Some(Asset::Scene(s)) = self.assets.get(id) {
                    let spawned =
                        SceneFile::from_ron(&s.text).and_then(|f| f.spawn_into(&mut self.world));
                    match spawned {
                        Ok(e) => tracing::info!(entities = e.len(), "scene loaded"),
                        Err(e) => tracing::error!(error = %e, "scene could not be instantiated"),
                    }
                }
            }
            Some(LoadState::Failed(e)) => {
                self.pending_scene = None;
                tracing::error!(error = %e, "scene failed to load");
            }
            _ => {}
        }
    }

    /// Transform system (parents first). Camera and light placement come from the resulting
    /// [`GlobalTransform`]s at [`extract`](Self::extract).
    pub fn run_systems(&mut self) {
        scene::update_transforms(&mut self.world);
    }

    /// Builds the frame's [`RenderScene`], creating GPU resources for assets that finished
    /// loading and substituting the renderer's fallbacks for the others.
    pub fn extract(&mut self, device: &VulkanDevice) -> RenderScene {
        let draws: Vec<(MeshRenderer, GlobalTransform)> = self
            .world
            .query::<(&MeshRenderer, &GlobalTransform)>()
            .iter()
            .map(|(r, g)| (*r, *g))
            .collect();
        let meshes = draws
            .into_iter()
            .map(|(r, g)| MeshInstance {
                mesh: self.mesh(device, r.mesh),
                material: self.material(device, r.material),
                transform: g.matrix,
                prev_transform: g.prev,
            })
            .collect();
        let mut sun = None;
        let lights = self
            .world
            .query::<(&scene::Light, &GlobalTransform)>()
            .iter()
            .map(|(l, g)| {
                let pos = g.matrix.w_axis.truncate();
                let dir = g
                    .matrix
                    .transform_vector3(Vec3::NEG_Z)
                    .normalize_or(Vec3::NEG_Y);
                let color = Vec3::from(l.color);
                match l.kind {
                    LightKind::Directional => {
                        if l.cast_shadows && sun.is_none() {
                            sun = Some(dir);
                        }
                        Light::Directional {
                            direction: dir,
                            color,
                            intensity: l.intensity,
                            cast_shadows: l.cast_shadows,
                        }
                    }
                    LightKind::Point => Light::Point {
                        position: pos,
                        color,
                        intensity: l.intensity,
                        range: l.range,
                        cast_shadows: l.cast_shadows,
                    },
                    LightKind::Spot => Light::Spot {
                        position: pos,
                        direction: dir,
                        color,
                        intensity: l.intensity,
                        range: l.range,
                        inner_cone: l.inner_cone,
                        outer_cone: l.outer_cone,
                        cast_shadows: l.cast_shadows,
                    },
                }
            })
            .collect();
        let camera = self
            .world
            .query::<(&scene::Camera, &GlobalTransform)>()
            .iter()
            .next()
            .map(|(c, g)| {
                let (_, rotation, position) = g.matrix.to_scale_rotation_translation();
                Camera {
                    position,
                    rotation,
                    fov_y: c.fov_y,
                    near: c.near,
                    exposure: c.exposure,
                }
            })
            .unwrap_or_else(|| {
                if !self.pending_scene.is_some()
                    && !std::mem::replace(&mut self.warned_no_camera, true)
                {
                    tracing::warn!("no camera entity in the world, using a default view");
                }
                Camera {
                    position: Vec3::new(0.0, 2.0, 6.0),
                    rotation: foundation::math::Quat::IDENTITY,
                    fov_y: 60f32.to_radians(),
                    near: 0.1,
                    exposure: 1.0,
                }
            });
        let mut environment = Environment::default();
        if let Some(s) = sun {
            environment.sun_direction = s;
        }
        RenderScene {
            camera,
            meshes,
            lights,
            environment,
        }
    }

    /// Runs the load/extract cycle until no asset is pending and the scene is spawned (or
    /// `timeout`), so a headless screenshot shows the finished scene. Returns false on timeout.
    pub fn wait_for_assets(&mut self, device: &VulkanDevice, timeout: Duration) -> bool {
        let end = Instant::now() + timeout;
        loop {
            self.update_assets(device);
            self.run_systems();
            // Extract requests the assets entities reference and uploads the ones that are ready.
            let _ = self.extract(device);
            if !self.assets.is_busy() && !self.scene_pending() {
                return true;
            }
            if Instant::now() >= end {
                tracing::warn!("timed out waiting for assets");
                return false;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// Live preview of an unsaved material: updates the GPU copy of `id` in place from `draft`
    /// (editor material panel). Returns false when `id` has no GPU copy yet (not drawn so far).
    /// Call every frame while the draft differs from the saved asset: texture slots that are
    /// still loading resolve on a later call, and a hot reload of the old file is overwritten
    /// before the frame renders.
    pub fn preview_material(
        &mut self,
        device: &VulkanDevice,
        id: AssetId,
        draft: &MaterialAsset,
    ) -> bool {
        let Some(mat) = self.materials.get(&id).copied() else {
            return false;
        };
        if mat == self.renderer.missing_material() {
            return false;
        }
        let desc = self.material_desc(device, draft);
        self.renderer.update_material(mat, &desc)
    }

    /// Destroys the renderer's resources. Call after `device.wait_idle()`.
    pub fn destroy(self, device: &VulkanDevice) {
        self.renderer.destroy(device);
    }

    fn mesh(&mut self, device: &VulkanDevice, id: AssetId) -> MeshId {
        let fallback = self.renderer.fallback_mesh();
        if id.is_nil() {
            return fallback;
        }
        if let Some(m) = self.meshes.get(&id) {
            return *m;
        }
        match self.assets.load(id) {
            LoadState::Loading => fallback,
            LoadState::Failed(_) => {
                self.meshes.insert(id, fallback);
                fallback
            }
            LoadState::Loaded => {
                let made = match self.assets.get(id) {
                    Some(Asset::Mesh(m)) => self
                        .renderer
                        .create_mesh(device, "asset", &mesh_data(&m))
                        .map_err(|e| e.to_string()),
                    _ => Err("not a mesh".to_owned()),
                };
                let gpu = made.unwrap_or_else(|e| {
                    tracing::warn!(%id, error = %e, "mesh upload failed, using the fallback");
                    fallback
                });
                self.meshes.insert(id, gpu);
                gpu
            }
        }
    }

    fn texture(&mut self, device: &VulkanDevice, id: AssetId, srgb: bool) -> Option<TextureId> {
        if let Some(t) = self.textures.get(&(id, srgb)) {
            return Some(*t);
        }
        if self.assets.load(id) != LoadState::Loaded {
            return None;
        }
        let Some(Asset::Texture(t)) = self.assets.get(id) else {
            return None;
        };
        let gpu = self
            .renderer
            .create_texture(device, "asset", &t.rgba8, t.width, t.height, srgb)
            .unwrap_or_else(|e| {
                tracing::warn!(%id, error = %e, "texture upload failed, using white");
                self.renderer.white_texture()
            });
        self.textures.insert((id, srgb), gpu);
        Some(gpu)
    }

    fn material_desc(&mut self, device: &VulkanDevice, m: &MaterialAsset) -> MaterialDesc<'static> {
        let tex =
            |s: &mut Self, t: Option<AssetId>, srgb| t.and_then(|t| s.texture(device, t, srgb));
        MaterialDesc {
            label: "asset",
            base_color: m.base_color,
            base_color_texture: tex(self, m.base_color_texture, true),
            metallic: m.metallic,
            roughness: m.roughness,
            metallic_roughness_texture: tex(self, m.metallic_roughness_texture, false),
            normal_texture: tex(self, m.normal_texture, false),
            normal_scale: m.normal_scale,
            occlusion_texture: tex(self, m.occlusion_texture, false),
            occlusion_strength: m.occlusion_strength,
            emissive: m.emissive,
            emissive_texture: tex(self, m.emissive_texture, true),
            alpha_mode: match m.alpha_mode {
                assets::AlphaModeAsset::Opaque => AlphaMode::Opaque,
                assets::AlphaModeAsset::Mask(c) => AlphaMode::Mask(c),
                assets::AlphaModeAsset::Blend => AlphaMode::Blend,
            },
            double_sided: m.double_sided,
        }
    }

    fn material(&mut self, device: &VulkanDevice, id: AssetId) -> MaterialId {
        if id.is_nil() {
            return self.renderer.default_material();
        }
        if let Some(m) = self.materials.get(&id) {
            return *m;
        }
        let missing = self.renderer.missing_material();
        match self.assets.load(id) {
            LoadState::Loading => self.renderer.default_material(),
            LoadState::Failed(_) => {
                self.materials.insert(id, missing);
                missing
            }
            LoadState::Loaded => {
                let gpu = match self.assets.get(id) {
                    Some(Asset::Material(m)) => {
                        let desc = self.material_desc(device, &m);
                        self.renderer.create_material(&desc)
                    }
                    _ => missing,
                };
                self.materials.insert(id, gpu);
                gpu
            }
        }
    }

    /// Drops the GPU copies of `id` so the next extract rebuilds (or falls back). A reloaded
    /// material is updated in place so instances keep their id.
    fn invalidate(
        &mut self,
        device: &VulkanDevice,
        id: AssetId,
        reloaded: bool,
        old_textures: &mut Vec<TextureId>,
    ) {
        if let Some(m) = self.meshes.remove(&id)
            && m != self.renderer.fallback_mesh()
        {
            self.renderer.destroy_mesh(device, m);
        }
        for srgb in [false, true] {
            if let Some(t) = self.textures.remove(&(id, srgb))
                && t != self.renderer.white_texture()
            {
                old_textures.push(t);
            }
        }
        let Some(mat) = self.materials.get(&id).copied() else {
            return;
        };
        let owned = mat != self.renderer.missing_material();
        if reloaded
            && owned
            && let Some(Asset::Material(m)) = self.assets.get(id)
        {
            let desc = self.material_desc(device, &m);
            if self.renderer.update_material(mat, &desc) {
                return;
            }
        }
        self.materials.remove(&id);
        if owned {
            self.renderer.destroy_material(mat);
        }
    }
}

fn mesh_data(m: &MeshAsset) -> MeshData {
    MeshData {
        vertices: (0..m.positions.len())
            .map(|i| Vertex {
                pos: m.positions[i],
                normal: m.normals.get(i).copied().unwrap_or([0.0, 1.0, 0.0]),
                uv: m.uvs.get(i).copied().unwrap_or([0.0; 2]),
                tangent: m.tangents.get(i).copied().unwrap_or([1.0, 0.0, 0.0, 1.0]),
            })
            .collect(),
        indices: m.indices.clone(),
        lods: Vec::new(),
    }
}
