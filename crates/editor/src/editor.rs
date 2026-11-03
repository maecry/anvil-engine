//! [`Editor`]: the editor's state and the single place where [`EditorCommand`]s are applied.

use std::path::{Path, PathBuf};

use assets::{Asset, AssetId, AssetKind, AssetServer, LoadState, MATERIAL_VERSION, MaterialAsset};
use debug::panels::FrameHistory;
use foundation::math::{EulerRot, Mat4, Quat, Vec3};
use hecs::Entity;
use renderer::{Camera as RenderCamera, RenderScene};
use rhi::{Device, Format};
use rhi_vulkan::VulkanDevice;
use runtime::{Project, RuntimeCore};
use scene::{
    Camera, EntityUuid, GlobalTransform, Light, MeshRenderer, Name, SceneFile, Transform, World,
    instantiate_prefab,
};

use crate::EditorError;
use crate::command::{AssetRef, ComponentKind, EditorCommand, EntityRef};
use crate::pick::{self, Ray};
use crate::template;
use crate::ui::UiState;

/// Color format of the viewport target (the renderer encodes to gamma, egui shows it as is).
pub const VIEWPORT_FORMAT: Format = Format::Rgba8Unorm;

/// What [`Editor::apply`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The command took effect.
    Done,
    /// Not yet possible (an asset is still loading): apply it again next frame.
    Wait,
}

/// The editor's free-flying viewport camera. It is not an entity.
#[derive(Clone, Copy, Debug)]
pub struct FlyCamera {
    /// World position.
    pub position: Vec3,
    /// Rotation about +Y, radians.
    pub yaw: f32,
    /// Rotation about X, radians.
    pub pitch: f32,
}

impl FlyCamera {
    /// Looking from `eye` at `target`.
    pub fn looking_at(eye: Vec3, target: Vec3) -> Self {
        let f = (target - eye).normalize();
        Self {
            position: eye,
            yaw: (-f.x).atan2(-f.z),
            pitch: f.y.asin(),
        }
    }

    /// Orientation.
    pub fn rotation(&self) -> Quat {
        Quat::from_euler(EulerRot::YXZ, self.yaw, self.pitch, 0.0)
    }

    /// As a render camera.
    pub fn camera(&self) -> RenderCamera {
        RenderCamera {
            position: self.position,
            rotation: self.rotation(),
            fov_y: 60f32.to_radians(),
            near: 0.1,
            exposure: 1.0,
        }
    }
}

/// An unsaved `.material` being edited.
pub struct MaterialDraft {
    /// Project-relative file.
    pub path: String,
    /// The asset it is registered as.
    pub id: Option<AssetId>,
    /// Edited contents (previewed live).
    pub draft: MaterialAsset,
    /// Contents of the file.
    pub saved: MaterialAsset,
}

/// A translate-gizmo drag in progress.
#[derive(Clone, Copy, Debug)]
pub struct GizmoDrag {
    /// 0, 1, 2 for X, Y, Z.
    pub axis: usize,
    /// Axis parameter under the cursor when the drag began.
    pub start_t: f32,
    /// World position of the entity when the drag began.
    pub start_pos: Vec3,
}

/// Editor state. Everything that changes the project goes through [`Editor::apply`].
pub struct Editor {
    /// World, assets and the renderer bridge (the same pieces the player runs).
    pub core: RuntimeCore,
    pub(crate) project: Option<(PathBuf, Project)>,
    pub(crate) scene_path: Option<String>,
    pub(crate) selected: Option<EntityUuid>,
    pub(crate) camera: FlyCamera,
    pub(crate) material: Option<MaterialDraft>,
    /// The world as RON while playing.
    pub(crate) snapshot: Option<String>,
    pub(crate) pending: Vec<EditorCommand>,
    pub(crate) screenshot: Option<PathBuf>,
    pub(crate) idle_frames: u32,
    pub(crate) quit: bool,
    pub(crate) status: String,
    pub(crate) history: FrameHistory,
    pub(crate) ui: UiState,
    pub(crate) gizmo: Option<GizmoDrag>,
    /// Viewport rectangle of the last UI pass, in egui points.
    pub(crate) viewport_rect: Option<debug::egui::Rect>,
    /// Egui id of the viewport texture, set by the app before the UI runs.
    pub(crate) viewport_tex: Option<debug::egui::TextureId>,
}

const DEFAULT_EYE: Vec3 = Vec3::new(6.0, 4.0, 8.0);

impl Editor {
    /// A new editor without a project; open one with [`EditorCommand::OpenProject`].
    ///
    /// # Errors
    /// Renderer creation failures.
    pub fn new(device: &VulkanDevice) -> Result<Self, EditorError> {
        let assets = AssetServer::new(std::env::temp_dir().join("anvil-editor-no-project"))?;
        Ok(Self {
            core: RuntimeCore::new(device, VIEWPORT_FORMAT, (1280, 720), assets)?,
            project: None,
            scene_path: None,
            selected: None,
            camera: FlyCamera::looking_at(DEFAULT_EYE, Vec3::ZERO),
            material: None,
            snapshot: None,
            pending: Vec::new(),
            screenshot: None,
            idle_frames: 0,
            quit: false,
            status: "No project open: File > New Project / Open Project".into(),
            history: FrameHistory::default(),
            ui: UiState::default(),
            gizmo: None,
            viewport_rect: None,
            viewport_tex: None,
        })
    }

    /// True while the world is a copy running under Play.
    pub fn playing(&self) -> bool {
        self.snapshot.is_some()
    }

    /// True once a command asked to quit.
    pub fn quit_requested(&self) -> bool {
        self.quit
    }

    /// The screenshot requested for this frame, taken by the app after rendering.
    pub(crate) fn take_screenshot(&mut self) -> Option<PathBuf> {
        self.screenshot.take()
    }

    fn project_dir(&self) -> Result<&Path, EditorError> {
        self.project
            .as_ref()
            .map(|(d, _)| d.as_path())
            .ok_or_else(|| EditorError::cmd("no project is open"))
    }

    /// Start of a frame: finished imports and the idle counter.
    pub fn begin_frame(&mut self, device: &VulkanDevice) {
        self.core.update_assets(device);
        if self.core.assets.is_busy() || self.core.scene_pending() {
            self.idle_frames = 0;
        } else {
            self.idle_frames += 1;
        }
    }

    /// Applies the commands the UI queued, logging failures into the status line.
    pub fn apply_pending(&mut self, device: &VulkanDevice) {
        for cmd in std::mem::take(&mut self.pending) {
            // A queued command that must wait is retried by queueing it again.
            match self.apply(device, cmd.clone()) {
                Ok(Outcome::Wait) => self.pending.push(cmd),
                Ok(Outcome::Done) => {}
                Err(e) => {
                    tracing::error!(error = %e, ?cmd, "editor command failed");
                    self.status = format!("error: {e}");
                }
            }
        }
    }

    // ---- lookups --------------------------------------------------------------------------

    pub(crate) fn entity(&self, r: &EntityRef) -> Result<Entity, EditorError> {
        let found = match r {
            EntityRef::Name(n) => self
                .core
                .world
                .query::<(Entity, &Name)>()
                .iter()
                .find(|(_, name)| name.0 == *n)
                .map(|(e, _)| e),
            EntityRef::Uuid(u) => self.core.world.find_uuid(*u),
        };
        found.ok_or_else(|| EditorError::cmd(format!("no entity {r:?}")))
    }

    pub(crate) fn uuid_of(&self, e: Entity) -> Option<EntityUuid> {
        self.core.world.get::<&EntityUuid>(e).ok().map(|u| *u)
    }

    pub(crate) fn name_of(&self, e: Entity) -> String {
        self.core
            .world
            .get::<&Name>(e)
            .map_or_else(|_| "(unnamed)".into(), |n| n.0.clone())
    }

    pub(crate) fn selected_entity(&self) -> Option<Entity> {
        self.core.world.find_uuid(self.selected?)
    }

    fn asset(&self, r: &AssetRef, kind: AssetKind) -> Result<AssetId, EditorError> {
        let id = match r {
            AssetRef::Path(p) => self
                .core
                .assets
                .id_for_path(p)
                .ok_or_else(|| EditorError::cmd(format!("unknown asset path {p}")))?,
            AssetRef::Id(id) => *id,
        };
        match self.core.assets.list().iter().find(|a| a.id == id) {
            Some(a) if a.kind == kind => Ok(id),
            Some(a) => Err(EditorError::cmd(format!(
                "{r:?} is a {:?}, expected {kind:?}",
                a.kind
            ))),
            None => Err(EditorError::cmd(format!("unknown asset {r:?}"))),
        }
    }

    // ---- bounds and picking ---------------------------------------------------------------

    fn mesh_bounds(&self, id: AssetId) -> (Vec3, Vec3) {
        // shortcut: bounds are recomputed from the vertices on each call, cache per mesh when scenes get large.
        if !id.is_nil()
            && let Some(Asset::Mesh(m)) = self.core.assets.get(id)
            && !m.positions.is_empty()
        {
            return m.positions.iter().fold(
                (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)),
                |(lo, hi), p| (lo.min(Vec3::from(*p)), hi.max(Vec3::from(*p))),
            );
        }
        // The built-in cube stands in for NIL and for meshes that are not loaded.
        (Vec3::splat(-0.5), Vec3::splat(0.5))
    }

    fn world_bounds_of(&self, e: Entity) -> Option<(Vec3, Vec3)> {
        let r = self.core.world.get::<&MeshRenderer>(e).ok()?;
        let g = self.core.world.get::<&GlobalTransform>(e).ok()?;
        let (lo, hi) = self.mesh_bounds(r.mesh);
        Some(pick::transform_aabb(&g.matrix, lo, hi))
    }

    /// World box around `e` and its descendants' meshes; a small box at the origin of the
    /// entity when there is no mesh below it.
    pub(crate) fn selection_bounds(&self, e: Entity) -> (Vec3, Vec3) {
        let mut stack = vec![e];
        let mut acc: Option<(Vec3, Vec3)> = None;
        while let Some(x) = stack.pop() {
            stack.extend(self.core.world.children_of(x));
            if let Some((lo, hi)) = self.world_bounds_of(x) {
                acc = Some(acc.map_or((lo, hi), |(a, b)| (a.min(lo), b.max(hi))));
            }
        }
        acc.unwrap_or_else(|| {
            let p = self.world_position(e);
            (p - Vec3::splat(0.25), p + Vec3::splat(0.25))
        })
    }

    pub(crate) fn world_position(&self, e: Entity) -> Vec3 {
        self.core
            .world
            .get::<&GlobalTransform>(e)
            .map_or(Vec3::ZERO, |g| g.matrix.w_axis.truncate())
    }

    /// Nearest `MeshRenderer` entity whose world box the ray hits.
    pub(crate) fn pick(&self, ray: &Ray) -> Option<Entity> {
        self.core
            .world
            .query::<(Entity, &MeshRenderer)>()
            .iter()
            .filter_map(|(e, _)| {
                let (lo, hi) = self.world_bounds_of(e)?;
                Some((pick::ray_aabb(ray, lo, hi)?, e))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, e)| e)
    }

    // ---- frame products -------------------------------------------------------------------

    /// Runs the transform system and extracts the frame. While editing the fly camera renders
    /// (and the selection box and gizmo are queued as debug lines); while playing, the scene's
    /// own camera does.
    pub fn render_scene(&mut self, device: &VulkanDevice) -> RenderScene {
        self.core.run_systems();
        let mut scene = self.core.extract(device);
        self.preview_material(device);
        if !self.playing() {
            scene.camera = self.camera.camera();
            self.draw_selection();
        }
        scene
    }

    /// Shows the material draft while it differs from the saved file or from the loaded asset
    /// (a save is reimported ~200 ms later, an old reload may land after newer edits). Runs
    /// after the extract so a GPU copy made from the stale asset is fixed before it is drawn.
    fn preview_material(&mut self, device: &VulkanDevice) {
        let Some(m) = &self.material else { return };
        let Some(id) = m.id else { return };
        let stale = matches!(self.core.assets.get(id), Some(Asset::Material(a)) if *a != m.draft);
        if m.draft != m.saved || stale {
            // shortcut: previews every frame while they differ (texture slots may still be loading), upgrade to change-driven when material counts grow.
            let draft = m.draft.clone();
            self.core.preview_material(device, id, &draft);
        }
    }

    fn draw_selection(&mut self) {
        let Some(e) = self.selected_entity() else {
            return;
        };
        let (lo, hi) = self.selection_bounds(e);
        let origin = self.world_position(e);
        let len = (origin - self.camera.position).length().max(0.5) * crate::ui::GIZMO_SCALE;
        let active = self.gizmo.map(|g| g.axis);
        let lines = self.core.renderer.debug_lines();
        lines.aabb(lo, hi, Vec3::new(1.0, 0.8, 0.1));
        for (i, axis) in [Vec3::X, Vec3::Y, Vec3::Z].into_iter().enumerate() {
            let color = if active == Some(i) {
                Vec3::new(1.0, 1.0, 0.2)
            } else {
                axis
            };
            lines.line(origin, origin + axis * len, color);
            // A second, offset line makes the handle read thicker than one pixel.
            let side = axis.any_orthonormal_vector() * len * 0.02;
            lines.line(origin + side, origin + axis * len + side, color);
        }
    }

    // ---- the command switchboard ----------------------------------------------------------

    /// Applies one command.
    ///
    /// # Errors
    /// [`EditorError`] when the command is invalid in the current state.
    pub fn apply(
        &mut self,
        device: &VulkanDevice,
        cmd: EditorCommand,
    ) -> Result<Outcome, EditorError> {
        use EditorCommand as C;
        match cmd {
            C::NewProject { path } => {
                template::create_project(&path)?;
                self.open_project(device, &path)?;
            }
            C::OpenProject { path } => self.open_project(device, &path)?,
            C::ImportFile { src, dest } => {
                let target = self.project_dir()?.join("assets").join(&dest);
                if let Some(dir) = target.parent() {
                    std::fs::create_dir_all(dir).map_err(|e| EditorError::io(dir, &e))?;
                }
                std::fs::copy(&src, &target).map_err(|e| EditorError::io(&src, &e))?;
                self.core.assets.rescan()?;
            }
            C::Select { entity } => {
                self.selected = match entity {
                    Some(r) => self.uuid_of(self.entity(&r)?),
                    None => None,
                };
            }
            C::CreateEntity { name, parent } => {
                let parent = parent.map(|p| self.entity(&p)).transpose()?;
                let e = self.core.world.spawn_entity(&name);
                self.core.world.set_parent(e, parent)?;
                self.selected = self.uuid_of(e);
            }
            C::DeleteEntity { entity } => {
                let e = self.entity(&entity)?;
                self.core.world.despawn_recursive(e);
                if self.selected_entity().is_none() {
                    self.selected = None;
                }
            }
            C::Rename { entity, name } => {
                let e = self.entity(&entity)?;
                let _ = self.core.world.insert_one(e, Name(name));
            }
            C::SetTransform {
                entity,
                translation,
                rotation_deg,
                scale,
            } => {
                let e = self.entity(&entity)?;
                let [x, y, z] = rotation_deg.map(f32::to_radians);
                let t = Transform {
                    translation: Vec3::from(translation),
                    rotation: Quat::from_euler(EulerRot::YXZ, y, x, z),
                    scale: Vec3::from(scale),
                };
                let _ = self.core.world.insert_one(e, t);
            }
            C::SetParent { entity, parent } => {
                let e = self.entity(&entity)?;
                let parent = parent.map(|p| self.entity(&p)).transpose()?;
                // Keep the entity where it is in the world.
                scene::update_transforms(&mut self.core.world);
                let global = self.global_matrix(e);
                self.core.world.set_parent(e, parent)?;
                let parent_global = parent.map_or(Mat4::IDENTITY, |p| self.global_matrix(p));
                let (scale, rotation, translation) =
                    (parent_global.inverse() * global).to_scale_rotation_translation();
                let _ = self.core.world.insert_one(
                    e,
                    Transform {
                        translation,
                        rotation,
                        scale,
                    },
                );
            }
            C::AddComponent { entity, component } => {
                let e = self.entity(&entity)?;
                let w = &mut self.core.world;
                let _ = match component {
                    ComponentKind::MeshRenderer => w.insert_one(
                        e,
                        MeshRenderer {
                            mesh: AssetId::NIL,
                            material: AssetId::NIL,
                        },
                    ),
                    ComponentKind::Light => w.insert_one(e, Light::default()),
                    ComponentKind::Camera => w.insert_one(e, Camera::default()),
                };
            }
            C::RemoveComponent { entity, component } => {
                let e = self.entity(&entity)?;
                let w = &mut self.core.world;
                let _ = match component {
                    ComponentKind::MeshRenderer => w.remove_one::<MeshRenderer>(e).map(drop),
                    ComponentKind::Light => w.remove_one::<Light>(e).map(drop),
                    ComponentKind::Camera => w.remove_one::<Camera>(e).map(drop),
                };
            }
            C::SetLight { entity, light } => {
                let e = self.entity(&entity)?;
                let _ = self.core.world.insert_one(e, light);
            }
            C::SetCamera { entity, camera } => {
                let e = self.entity(&entity)?;
                let _ = self.core.world.insert_one(e, camera);
            }
            C::AssignMesh { entity, mesh } => {
                let e = self.entity(&entity)?;
                let id = match mesh {
                    Some(r) => self.asset(&r, AssetKind::Mesh)?,
                    None => AssetId::NIL,
                };
                self.edit_renderer(e, |r| r.mesh = id);
            }
            C::AssignMaterial { entity, material } => {
                let e = self.entity(&entity)?;
                let id = match material {
                    Some(r) => self.asset(&r, AssetKind::Material)?,
                    None => AssetId::NIL,
                };
                self.edit_renderer(e, |r| r.material = id);
            }
            C::InstantiateModel {
                model,
                name,
                translation,
            } => {
                let id = self.asset(&model, AssetKind::Prefab)?;
                match self.core.assets.load(id) {
                    LoadState::Loading => return Ok(Outcome::Wait),
                    LoadState::Failed(e) => {
                        return Err(EditorError::cmd(format!("model failed to load: {e}")));
                    }
                    LoadState::Loaded => {}
                }
                let Some(Asset::Prefab(prefab)) = self.core.assets.get(id) else {
                    return Err(EditorError::cmd("model is not a prefab"));
                };
                let root = instantiate_prefab(
                    &mut self.core.world,
                    &prefab,
                    &name,
                    Transform::from_translation(Vec3::from(translation)),
                );
                self.selected = self.uuid_of(root);
            }
            C::CreateMaterial { path } => {
                let full = self.asset_file(&path, "material")?;
                if full.exists() {
                    return Err(EditorError::cmd(format!("{path} already exists")));
                }
                let m = MaterialAsset {
                    metallic: 0.0,
                    roughness: 0.5,
                    ..MaterialAsset::default()
                };
                write_material(&full, &m)?;
                self.core.assets.rescan()?;
                self.open_material(&path)?;
            }
            C::OpenMaterial { path } => self.open_material(&path)?,
            C::EditMaterial { path, material } => {
                if self.material.as_ref().is_none_or(|m| m.path != path) {
                    self.open_material(&path)?;
                }
                if let Some(m) = &mut self.material {
                    m.draft = material;
                }
            }
            C::SaveMaterial { path } => {
                let full = self.asset_file(&path, "material")?;
                let m = self
                    .material
                    .as_mut()
                    .filter(|m| m.path == path)
                    .ok_or_else(|| EditorError::cmd(format!("{path} is not open for editing")))?;
                write_material(&full, &m.draft)?;
                // The watcher reimports the file shortly; the saved contents equal the
                // draft, so the reload changes nothing visible.
                m.saved = m.draft.clone();
            }
            C::CreateScene { path } => {
                let full = self.asset_file(&path, "scene")?;
                let empty = SceneFile {
                    version: assets::SCENE_VERSION,
                    entities: Vec::new(),
                };
                write_text(&full, &empty.to_ron())?;
                self.core.assets.rescan()?;
            }
            C::OpenScene { path } => self.open_scene_file(&path)?,
            C::SaveScene => {
                if self.playing() {
                    return Err(EditorError::cmd("stop playing before saving"));
                }
                let path = self
                    .scene_path
                    .clone()
                    .ok_or_else(|| EditorError::cmd("no scene is open"))?;
                let full = self.project_dir()?.join(&path);
                write_text(&full, &SceneFile::from_world(&self.core.world).to_ron())?;
                self.status = format!("saved {path}");
            }
            C::Play => {
                if self.playing() {
                    return Err(EditorError::cmd("already playing"));
                }
                self.snapshot = Some(SceneFile::from_world(&self.core.world).to_ron());
                self.gizmo = None;
            }
            C::Stop => {
                let text = self
                    .snapshot
                    .take()
                    .ok_or_else(|| EditorError::cmd("not playing"))?;
                self.restore(&text)?;
            }
            C::Screenshot { path } => {
                // Settled: no import in flight for a few frames (GPU copies are made on extract).
                if self.idle_frames < 3 {
                    return Ok(Outcome::Wait);
                }
                self.screenshot = Some(path);
            }
            C::WaitFrames(_) => {}
            C::AssertEntity {
                name,
                has_mesh,
                parent,
                material,
                translation,
            } => self.assert_entity(&name, has_mesh, parent, material, translation)?,
            C::AssertEntityCount { at_least } => {
                let n = self.core.world.query::<&EntityUuid>().iter().count();
                if n < at_least {
                    return Err(EditorError::cmd(format!(
                        "assert: {n} entities, expected at least {at_least}"
                    )));
                }
            }
            C::Quit => self.quit = true,
        }
        Ok(Outcome::Done)
    }

    fn global_matrix(&self, e: Entity) -> Mat4 {
        self.core
            .world
            .get::<&GlobalTransform>(e)
            .map_or(Mat4::IDENTITY, |g| g.matrix)
    }

    fn edit_renderer(&mut self, e: Entity, f: impl FnOnce(&mut MeshRenderer)) {
        if let Ok(mut r) = self.core.world.get::<&mut MeshRenderer>(e) {
            f(&mut r);
            return;
        }
        let mut r = MeshRenderer {
            mesh: AssetId::NIL,
            material: AssetId::NIL,
        };
        f(&mut r);
        let _ = self.core.world.insert_one(e, r);
    }

    fn assert_entity(
        &self,
        name: &str,
        has_mesh: Option<bool>,
        parent: Option<String>,
        material: Option<AssetRef>,
        translation: Option<[f32; 3]>,
    ) -> Result<(), EditorError> {
        let fail = |what: String| Err(EditorError::cmd(format!("assert {name:?}: {what}")));
        let e = self.entity(&EntityRef::Name(name.to_owned()))?;
        let mesh = self.core.world.get::<&MeshRenderer>(e).ok().map(|r| *r);
        if let Some(want) = has_mesh
            && mesh.is_some() != want
        {
            return fail(format!("has_mesh is {}", mesh.is_some()));
        }
        if let Some(want) = parent {
            let actual = self
                .core
                .world
                .parent_of(e)
                .map_or_else(String::new, |p| self.name_of(p));
            if actual != want {
                return fail(format!("parent is {actual:?}, expected {want:?}"));
            }
        }
        if let Some(r) = material {
            let want = self.asset(&r, AssetKind::Material)?;
            if mesh.map(|m| m.material) != Some(want) {
                return fail(format!("material is not {r:?}"));
            }
        }
        if let Some(want) = translation {
            let got = self.core.world.get::<&Transform>(e).map(|t| t.translation);
            if !got
                .as_ref()
                .is_ok_and(|g| g.abs_diff_eq(Vec3::from(want), 1e-3))
            {
                return fail(format!("translation is {got:?}, expected {want:?}"));
            }
        }
        Ok(())
    }

    // ---- project, scene and material files ------------------------------------------------

    /// `path` inside the project with the extension `ext`.
    fn asset_file(&self, path: &str, ext: &str) -> Result<PathBuf, EditorError> {
        let p = Path::new(path);
        let ok = p.starts_with("assets")
            && p.extension().is_some_and(|e| e == ext)
            && !p
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir));
        if !ok {
            return Err(EditorError::cmd(format!(
                "{path:?} must be a relative assets/**/*.{ext} path"
            )));
        }
        Ok(self.project_dir()?.join(p))
    }

    fn open_project(&mut self, device: &VulkanDevice, dir: &Path) -> Result<(), EditorError> {
        let project = Project::load(dir)?;
        let mut assets = AssetServer::new(dir)?;
        assets.enable_hot_reload()?;
        // A fresh core: the GPU copies of the previous project's assets go with the old one.
        let new = RuntimeCore::new(device, VIEWPORT_FORMAT, (1280, 720), assets)?;
        let old = std::mem::replace(&mut self.core, new);
        device.wait_idle()?;
        old.destroy(device);
        let startup = project.startup_scene.clone();
        tracing::info!(project = %project.name, dir = %dir.display(), "project opened");
        self.status = format!("opened {}", project.name);
        self.project = Some((dir.to_owned(), project));
        self.material = None;
        self.snapshot = None;
        self.gizmo = None;
        self.camera = FlyCamera::looking_at(DEFAULT_EYE, Vec3::ZERO);
        self.open_scene_file(&startup)
    }

    fn open_scene_file(&mut self, path: &str) -> Result<(), EditorError> {
        let full = self.project_dir()?.join(path);
        let text = std::fs::read_to_string(&full).map_err(|e| EditorError::io(&full, &e))?;
        self.restore(&text)?;
        self.scene_path = Some(path.to_owned());
        self.snapshot = None;
        Ok(())
    }

    /// Replaces the world with the entities of a scene text. Selection is kept when the
    /// entity still exists (uuids survive).
    fn restore(&mut self, text: &str) -> Result<(), EditorError> {
        let file = SceneFile::from_ron(text)?;
        let mut world = World::new();
        file.spawn_into(&mut world)?;
        self.core.world = world;
        if self.selected_entity().is_none() {
            self.selected = None;
        }
        Ok(())
    }

    fn open_material(&mut self, path: &str) -> Result<(), EditorError> {
        let full = self.asset_file(path, "material")?;
        let text = std::fs::read_to_string(&full).map_err(|e| EditorError::io(&full, &e))?;
        let m: MaterialAsset =
            ron::from_str(&text).map_err(|e| EditorError::cmd(format!("{path}: {e}")))?;
        if m.version > MATERIAL_VERSION {
            return Err(EditorError::cmd(format!(
                "{path}: material is from a newer engine"
            )));
        }
        self.material = Some(MaterialDraft {
            path: path.to_owned(),
            id: self.core.assets.id_for_path(path),
            draft: m.clone(),
            saved: m,
        });
        Ok(())
    }
}

fn write_text(path: &Path, text: &str) -> Result<(), EditorError> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| EditorError::io(dir, &e))?;
    }
    std::fs::write(path, text).map_err(|e| EditorError::io(path, &e))
}

fn write_material(path: &Path, m: &MaterialAsset) -> Result<(), EditorError> {
    let text = ron::ser::to_string_pretty(m, ron::ser::PrettyConfig::default())
        .map_err(|e| EditorError::cmd(e.to_string()))?;
    write_text(path, &text)
}
