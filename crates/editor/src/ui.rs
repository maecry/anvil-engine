//! The editor's panels. Panels only read the editor and push [`EditorCommand`]s.

use assets::{AssetInfo, AssetKind, AssetStats, LoadState, MaterialAsset};
use debug::egui::{self, Color32, Key, PointerButton, Sense};
use debug::panels;
use foundation::math::{EulerRot, Vec3};
use hecs::Entity;
use rhi::DeviceDiagnostics;
use scene::{Camera, Light, LightKind, MeshRenderer};

use crate::command::{AssetRef, ComponentKind, EditorCommand as C, EntityRef};
use crate::editor::{Editor, GizmoDrag};
use crate::pick;

/// Gizmo arm length as a fraction of the distance to the camera.
pub const GIZMO_SCALE: f32 = 0.18;
/// Pixels within which a click grabs a gizmo handle.
const GRAB_PIXELS: f32 = 10.0;

/// Text fields and dialogs of the panels.
#[derive(Default)]
pub struct UiState {
    dialog: Dialog,
    path: String,
    new_material: String,
    new_scene: String,
}

#[derive(Default, PartialEq, Eq)]
enum Dialog {
    #[default]
    None,
    New,
    Open,
}

fn uuid_ref(ed: &Editor, e: Entity) -> Option<EntityRef> {
    ed.uuid_of(e).map(EntityRef::Uuid)
}

impl Editor {
    /// Builds the whole editor UI for one egui pass and queues the commands it produces.
    pub fn ui(&mut self, ctx: &egui::Context, diag: &DeviceDiagnostics) {
        let mut cmds: Vec<C> = Vec::new();
        let mut st = std::mem::take(&mut self.ui);
        // Opening the browser asks for every source asset, so failures show up in it.
        let infos = self.core.assets.list();
        for a in infos.iter().filter(|a| a.parent.is_none()) {
            self.core.assets.load(a.id);
        }

        self.menu_bar(ctx, &mut st, &mut cmds);
        egui::SidePanel::left("hierarchy")
            .default_width(260.0)
            .show(ctx, |ui| {
                ui.heading("Hierarchy");
                egui::ScrollArea::vertical().show(ui, |ui| self.hierarchy(ui, &mut cmds));
            });
        egui::SidePanel::right("inspector")
            .default_width(340.0)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.heading("Inspector");
                    self.inspector(ui, &infos, &mut cmds);
                    ui.separator();
                    ui.heading("Material");
                    self.material_editor(ui, &infos, &mut cmds);
                });
            });
        egui::TopBottomPanel::bottom("bottom")
            .default_height(250.0)
            .resizable(true)
            .show(ctx, |ui| {
                ui.columns(2, |cols| {
                    cols[0].heading("Assets");
                    egui::ScrollArea::vertical()
                        .id_salt("assets")
                        .show(&mut cols[0], |ui| {
                            self.asset_browser(ui, &infos, &mut st, &mut cmds)
                        });
                    cols[1].heading("Debug");
                    egui::ScrollArea::vertical()
                        .id_salt("debug")
                        .show(&mut cols[1], |ui| self.debug_panels(ui, diag));
                });
            });
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ctx, |ui| self.viewport(ui, &mut cmds));
        self.dialogs(ctx, &mut st, &mut cmds);
        self.shortcuts(ctx, &mut cmds);

        self.ui = st;
        self.pending.extend(cmds);
    }

    fn shortcuts(&self, ctx: &egui::Context, cmds: &mut Vec<C>) {
        if ctx.wants_keyboard_input() {
            return;
        }
        let (delete, save) = ctx.input(|i| {
            (
                i.key_pressed(Key::Delete),
                i.modifiers.command && i.key_pressed(Key::S),
            )
        });
        if delete && let Some(e) = self.selected_entity().and_then(|e| uuid_ref(self, e)) {
            cmds.push(C::DeleteEntity { entity: e });
        }
        if save {
            cmds.push(C::SaveScene);
        }
    }

    // ---- menu and dialogs -----------------------------------------------------------------

    fn menu_bar(&self, ctx: &egui::Context, st: &mut UiState, cmds: &mut Vec<C>) {
        egui::TopBottomPanel::top("menu").show(ctx, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui.button("New Project...").clicked() {
                        st.dialog = Dialog::New;
                        ui.close();
                    }
                    if ui.button("Open Project...").clicked() {
                        st.dialog = Dialog::Open;
                        ui.close();
                    }
                    if ui.button("Save Scene   Ctrl+S").clicked() {
                        cmds.push(C::SaveScene);
                        ui.close();
                    }
                    if ui.button("Quit").clicked() {
                        cmds.push(C::Quit);
                        ui.close();
                    }
                });
                ui.menu_button("Scene", |ui| {
                    if ui.button("Create Empty").clicked() {
                        cmds.push(C::CreateEntity {
                            name: "Entity".into(),
                            parent: None,
                        });
                        ui.close();
                    }
                });
                ui.separator();
                if self.playing() {
                    if ui.button("Stop").clicked() {
                        cmds.push(C::Stop);
                    }
                } else if ui.button("Play").clicked() {
                    cmds.push(C::Play);
                }
                ui.separator();
                let project = self
                    .project
                    .as_ref()
                    .map_or("(no project)", |(_, p)| p.name.as_str());
                let scene = self.scene_path.as_deref().unwrap_or("(no scene)");
                ui.label(format!("{project}  |  {scene}"));
                ui.separator();
                ui.label(&self.status);
            });
        });
    }

    fn dialogs(&self, ctx: &egui::Context, st: &mut UiState, cmds: &mut Vec<C>) {
        if st.dialog == Dialog::None {
            return;
        }
        let new = st.dialog == Dialog::New;
        let mut open = true;
        let mut done = false;
        egui::Window::new(if new { "New project" } else { "Open project" })
            .open(&mut open)
            .collapsible(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label("Project directory:");
                ui.text_edit_singleline(&mut st.path);
                if ui.button(if new { "Create" } else { "Open" }).clicked() && !st.path.is_empty() {
                    let path = st.path.clone().into();
                    cmds.push(if new {
                        C::NewProject { path }
                    } else {
                        C::OpenProject { path }
                    });
                    done = true;
                }
            });
        if done || !open {
            st.dialog = Dialog::None;
        }
    }

    // ---- hierarchy ------------------------------------------------------------------------

    fn hierarchy(&self, ui: &mut egui::Ui, cmds: &mut Vec<C>) {
        let w = &self.core.world;
        let mut roots: Vec<(String, Entity)> = w
            .query::<(Entity, &scene::EntityUuid)>()
            .without::<&scene::Parent>()
            .iter()
            .map(|(e, _)| (self.name_of(e), e))
            .collect();
        roots.sort_by(|a, b| a.0.cmp(&b.0));
        for (_, e) in roots {
            self.hierarchy_node(ui, e, cmds);
        }
        // Clicking the empty area below the tree deselects.
        let rest = ui.available_size_before_wrap().y.max(24.0);
        let (_, r) = ui.allocate_exact_size(egui::vec2(ui.available_width(), rest), Sense::click());
        if r.clicked() {
            cmds.push(C::Select { entity: None });
        }
        r.context_menu(|ui| {
            if ui.button("Create Empty").clicked() {
                cmds.push(C::CreateEntity {
                    name: "Entity".into(),
                    parent: None,
                });
                ui.close();
            }
        });
    }

    fn hierarchy_node(&self, ui: &mut egui::Ui, e: Entity, cmds: &mut Vec<C>) {
        let Some(uuid) = self.uuid_of(e) else { return };
        let kids = self.core.world.children_of(e);
        let row = |ui: &mut egui::Ui, cmds: &mut Vec<C>| {
            let r = ui.selectable_label(self.selected == Some(uuid), self.name_of(e));
            if r.clicked() {
                cmds.push(C::Select {
                    entity: Some(EntityRef::Uuid(uuid)),
                });
            }
            r.context_menu(|ui| {
                let me = EntityRef::Uuid(uuid);
                if ui.button("Add child").clicked() {
                    cmds.push(C::CreateEntity {
                        name: "Entity".into(),
                        parent: Some(me.clone()),
                    });
                    ui.close();
                }
                if let Some(sel) = self.selected
                    && sel != uuid
                    && ui.button("Make selected a child of this").clicked()
                {
                    cmds.push(C::SetParent {
                        entity: EntityRef::Uuid(sel),
                        parent: Some(me.clone()),
                    });
                    ui.close();
                }
                if self.core.world.parent_of(e).is_some() && ui.button("Unparent").clicked() {
                    cmds.push(C::SetParent {
                        entity: me.clone(),
                        parent: None,
                    });
                    ui.close();
                }
                if ui.button("Delete").clicked() {
                    cmds.push(C::DeleteEntity { entity: me });
                    ui.close();
                }
            });
        };
        if kids.is_empty() {
            row(ui, cmds);
        } else {
            let id = ui.make_persistent_id(("node", uuid.0));
            egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, true)
                .show_header(ui, |ui| row(ui, cmds))
                .body(|ui| {
                    for k in kids {
                        self.hierarchy_node(ui, k, cmds);
                    }
                });
        }
    }

    // ---- inspector ------------------------------------------------------------------------

    fn inspector(&self, ui: &mut egui::Ui, infos: &[AssetInfo], cmds: &mut Vec<C>) {
        let Some(e) = self.selected_entity() else {
            ui.label("Nothing selected.");
            return;
        };
        let w = &self.core.world;
        let me = EntityRef::Uuid(self.selected.unwrap_or(scene::EntityUuid(0)));
        let mut name = self.name_of(e);
        if ui.text_edit_singleline(&mut name).changed() {
            cmds.push(C::Rename {
                entity: me.clone(),
                name,
            });
        }
        if let Ok(t) = w.get::<&scene::Transform>(e) {
            // shortcut: euler angles are recomputed from the quaternion every frame, so a drag near gimbal lock jumps; keep per-entity euler state when that matters.
            let (y, x, z) = t.rotation.to_euler(EulerRot::YXZ);
            let mut tr = t.translation.to_array();
            let mut rot = [x, y, z].map(f32::to_degrees);
            let mut sc = t.scale.to_array();
            let mut changed = false;
            egui::Grid::new("transform").num_columns(2).show(ui, |ui| {
                changed |= vec3_row(ui, "Position", &mut tr, 0.05);
                changed |= vec3_row(ui, "Rotation", &mut rot, 0.5);
                changed |= vec3_row(ui, "Scale", &mut sc, 0.01);
            });
            if changed {
                cmds.push(C::SetTransform {
                    entity: me.clone(),
                    translation: tr,
                    rotation_deg: rot,
                    scale: sc,
                });
            }
        }
        if let Ok(r) = w.get::<&MeshRenderer>(e) {
            ui.separator();
            let removed = component_header(ui, "Mesh Renderer");
            let pick = |ui: &mut egui::Ui, label: &str, kind, current: assets::AssetId| {
                asset_combo(ui, label, infos, kind, current)
            };
            let mesh = pick(ui, "Mesh", AssetKind::Mesh, r.mesh);
            if let Some(m) = mesh {
                cmds.push(C::AssignMesh {
                    entity: me.clone(),
                    mesh: (!m.is_nil()).then_some(AssetRef::Id(m)),
                });
            }
            if let Some(m) = pick(ui, "Material", AssetKind::Material, r.material) {
                cmds.push(C::AssignMaterial {
                    entity: me.clone(),
                    material: (!m.is_nil()).then_some(AssetRef::Id(m)),
                });
            }
            if removed {
                cmds.push(C::RemoveComponent {
                    entity: me.clone(),
                    component: ComponentKind::MeshRenderer,
                });
            }
        }
        if let Ok(l) = w.get::<&Light>(e) {
            ui.separator();
            let removed = component_header(ui, "Light");
            let mut n = *l;
            egui::ComboBox::from_label("Kind")
                .selected_text(format!("{:?}", n.kind))
                .show_ui(ui, |ui| {
                    for k in [LightKind::Directional, LightKind::Point, LightKind::Spot] {
                        ui.selectable_value(&mut n.kind, k, format!("{k:?}"));
                    }
                });
            ui.color_edit_button_rgb(&mut n.color);
            ui.add(
                egui::DragValue::new(&mut n.intensity)
                    .speed(0.1)
                    .prefix("intensity "),
            );
            if n.kind != LightKind::Directional {
                ui.add(
                    egui::DragValue::new(&mut n.range)
                        .speed(0.1)
                        .prefix("range "),
                );
            }
            if n.kind == LightKind::Spot {
                ui.add(egui::Slider::new(&mut n.inner_cone, 0.0..=1.5).text("inner"));
                ui.add(egui::Slider::new(&mut n.outer_cone, 0.0..=1.57).text("outer"));
            }
            ui.checkbox(&mut n.cast_shadows, "cast shadows");
            if n != *l {
                cmds.push(C::SetLight {
                    entity: me.clone(),
                    light: n,
                });
            }
            if removed {
                cmds.push(C::RemoveComponent {
                    entity: me.clone(),
                    component: ComponentKind::Light,
                });
            }
        }
        if let Ok(c) = w.get::<&Camera>(e) {
            ui.separator();
            let removed = component_header(ui, "Camera");
            let mut n = *c;
            let mut fov = n.fov_y.to_degrees();
            ui.add(egui::Slider::new(&mut fov, 10.0..=120.0).text("fov"));
            n.fov_y = fov.to_radians();
            ui.add(
                egui::DragValue::new(&mut n.near)
                    .speed(0.01)
                    .prefix("near "),
            );
            ui.add(
                egui::DragValue::new(&mut n.exposure)
                    .speed(0.01)
                    .prefix("exposure "),
            );
            if n != *c {
                cmds.push(C::SetCamera {
                    entity: me.clone(),
                    camera: n,
                });
            }
            if removed {
                cmds.push(C::RemoveComponent {
                    entity: me.clone(),
                    component: ComponentKind::Camera,
                });
            }
        }
        ui.separator();
        ui.menu_button("Add Component", |ui| {
            for (label, kind, has) in [
                (
                    "Mesh Renderer",
                    ComponentKind::MeshRenderer,
                    w.get::<&MeshRenderer>(e).is_ok(),
                ),
                ("Light", ComponentKind::Light, w.get::<&Light>(e).is_ok()),
                ("Camera", ComponentKind::Camera, w.get::<&Camera>(e).is_ok()),
            ] {
                if !has && ui.button(label).clicked() {
                    cmds.push(C::AddComponent {
                        entity: me.clone(),
                        component: kind,
                    });
                    ui.close();
                }
            }
        });
    }

    // ---- material editor ------------------------------------------------------------------

    fn material_editor(&self, ui: &mut egui::Ui, infos: &[AssetInfo], cmds: &mut Vec<C>) {
        let Some(m) = &self.material else {
            ui.label("Pick a material in the asset browser (Edit).");
            return;
        };
        ui.label(format!(
            "{}{}",
            m.path,
            if m.draft != m.saved {
                "  (unsaved)"
            } else {
                ""
            }
        ));
        let mut d: MaterialAsset = m.draft.clone();
        let mut rgb = [d.base_color[0], d.base_color[1], d.base_color[2]];
        ui.horizontal(|ui| {
            ui.label("Base color");
            ui.color_edit_button_rgb(&mut rgb);
        });
        d.base_color[..3].copy_from_slice(&rgb);
        ui.add(egui::Slider::new(&mut d.base_color[3], 0.0..=1.0).text("alpha"));
        ui.add(egui::Slider::new(&mut d.metallic, 0.0..=1.0).text("metallic"));
        ui.add(egui::Slider::new(&mut d.roughness, 0.0..=1.0).text("roughness"));
        ui.add(egui::Slider::new(&mut d.normal_scale, 0.0..=4.0).text("normal scale"));
        ui.add(egui::Slider::new(&mut d.occlusion_strength, 0.0..=1.0).text("occlusion"));
        ui.horizontal(|ui| {
            ui.label("Emissive");
            ui.color_edit_button_rgb(&mut d.emissive);
        });
        ui.checkbox(&mut d.double_sided, "double sided");
        let mut mode = match d.alpha_mode {
            assets::AlphaModeAsset::Opaque => 0,
            assets::AlphaModeAsset::Mask(_) => 1,
            assets::AlphaModeAsset::Blend => 2,
        };
        egui::ComboBox::from_label("alpha mode")
            .selected_text(["Opaque", "Mask", "Blend"][mode])
            .show_ui(ui, |ui| {
                for (i, n) in ["Opaque", "Mask", "Blend"].into_iter().enumerate() {
                    ui.selectable_value(&mut mode, i, n);
                }
            });
        let cutoff = match d.alpha_mode {
            assets::AlphaModeAsset::Mask(c) => c,
            _ => 0.5,
        };
        d.alpha_mode = match mode {
            0 => assets::AlphaModeAsset::Opaque,
            1 => {
                let mut c = cutoff;
                ui.add(egui::Slider::new(&mut c, 0.0..=1.0).text("cutoff"));
                assets::AlphaModeAsset::Mask(c)
            }
            _ => assets::AlphaModeAsset::Blend,
        };
        let slots: [(&str, &mut Option<assets::AssetId>); 5] = [
            ("Base color tex", &mut d.base_color_texture),
            ("Metal/rough tex", &mut d.metallic_roughness_texture),
            ("Normal tex", &mut d.normal_texture),
            ("Occlusion tex", &mut d.occlusion_texture),
            ("Emissive tex", &mut d.emissive_texture),
        ];
        for (label, slot) in slots {
            let cur = slot.unwrap_or(assets::AssetId::NIL);
            if let Some(n) = asset_combo(ui, label, infos, AssetKind::Texture, cur) {
                *slot = (!n.is_nil()).then_some(n);
            }
        }
        if d != m.draft {
            cmds.push(C::EditMaterial {
                path: m.path.clone(),
                material: d,
            });
        }
        ui.horizontal(|ui| {
            if ui
                .add_enabled(m.draft != m.saved, egui::Button::new("Save"))
                .clicked()
            {
                cmds.push(C::SaveMaterial {
                    path: m.path.clone(),
                });
            }
            if ui
                .add_enabled(m.draft != m.saved, egui::Button::new("Revert"))
                .clicked()
            {
                cmds.push(C::EditMaterial {
                    path: m.path.clone(),
                    material: m.saved.clone(),
                });
            }
        });
    }

    // ---- asset browser --------------------------------------------------------------------

    fn asset_browser(
        &self,
        ui: &mut egui::Ui,
        infos: &[AssetInfo],
        st: &mut UiState,
        cmds: &mut Vec<C>,
    ) {
        if self.project.is_none() {
            ui.label("No project open.");
            return;
        }
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut st.new_material)
                    .hint_text("material name")
                    .desired_width(100.0),
            );
            if ui.button("Create material").clicked() && !st.new_material.is_empty() {
                cmds.push(C::CreateMaterial {
                    path: format!("assets/materials/{}.material", st.new_material),
                });
                st.new_material.clear();
            }
            ui.add(
                egui::TextEdit::singleline(&mut st.new_scene)
                    .hint_text("scene name")
                    .desired_width(100.0),
            );
            if ui.button("Create scene").clicked() && !st.new_scene.is_empty() {
                cmds.push(C::CreateScene {
                    path: format!("assets/scenes/{}.scene", st.new_scene),
                });
                st.new_scene.clear();
            }
        });
        let stats = self.core.assets.stats();
        ui.label(format_stats(&stats));
        let files: Vec<&AssetInfo> = infos.iter().filter(|a| a.parent.is_none()).collect();
        self.folder(ui, "assets", &files, cmds);
    }

    /// One folder level: files directly below `dir` first, then sub-folders.
    fn folder(&self, ui: &mut egui::Ui, dir: &str, files: &[&AssetInfo], cmds: &mut Vec<C>) {
        let prefix = format!("{dir}/");
        let mut subs: Vec<&str> = files
            .iter()
            .filter_map(|a| {
                a.path
                    .strip_prefix(&prefix)?
                    .split_once('/')
                    .map(|(d, _)| d)
            })
            .collect();
        subs.sort_unstable();
        subs.dedup();
        for s in subs {
            egui::CollapsingHeader::new(s)
                .id_salt(format!("{prefix}{s}"))
                .default_open(true)
                .show(ui, |ui| {
                    self.folder(ui, &format!("{prefix}{s}"), files, cmds)
                });
        }
        for a in files {
            let Some(name) = a.path.strip_prefix(&prefix).filter(|n| !n.contains('/')) else {
                continue;
            };
            ui.horizontal(|ui| {
                ui.label(name);
                ui.weak(format!("{:?}", a.kind));
                match self.core.assets.state(a.id) {
                    Some(LoadState::Loaded) => ui.colored_label(Color32::LIGHT_GREEN, "loaded"),
                    Some(LoadState::Loading) => ui.colored_label(Color32::YELLOW, "loading"),
                    Some(LoadState::Failed(e)) => {
                        ui.colored_label(Color32::LIGHT_RED, format!("FAILED: {e}"))
                    }
                    None => ui.weak("not loaded"),
                };
                match a.kind {
                    AssetKind::Prefab if ui.small_button("Instantiate").clicked() => {
                        cmds.push(C::InstantiateModel {
                            model: AssetRef::Id(a.id),
                            name: name.split('.').next().unwrap_or(name).to_owned(),
                            translation: (self.camera.position
                                + self.camera.rotation() * Vec3::NEG_Z * 5.0)
                                .to_array(),
                        });
                    }
                    AssetKind::Material if ui.small_button("Edit").clicked() => {
                        cmds.push(C::OpenMaterial {
                            path: a.path.clone(),
                        });
                    }
                    AssetKind::Scene if ui.small_button("Open").clicked() => {
                        cmds.push(C::OpenScene {
                            path: a.path.clone(),
                        });
                    }
                    _ => {}
                }
            });
        }
    }

    // ---- debug ----------------------------------------------------------------------------

    fn debug_panels(&mut self, ui: &mut egui::Ui, diag: &DeviceDiagnostics) {
        let stats = self.core.renderer.stats();
        egui::CollapsingHeader::new("Frame")
            .default_open(true)
            .show(ui, |ui| panels::frame_stats(ui, &self.history, &stats));
        egui::CollapsingHeader::new("Renderer")
            .default_open(true)
            .show(ui, |ui| {
                panels::renderer_settings(ui, &mut self.core.renderer)
            });
        egui::CollapsingHeader::new("GPU passes")
            .show(ui, |ui| panels::gpu_timings(ui, &stats.gpu));
        egui::CollapsingHeader::new("Render graph")
            .show(ui, |ui| panels::render_graph(ui, &stats.graph, &stats.gpu));
        egui::CollapsingHeader::new("Memory & device")
            .show(ui, |ui| panels::memory(ui, diag, &stats));
    }

    // ---- viewport -------------------------------------------------------------------------

    fn viewport(&mut self, ui: &mut egui::Ui, cmds: &mut Vec<C>) {
        let rect = ui.available_rect_before_wrap();
        self.viewport_rect = Some(rect);
        let resp = ui.allocate_rect(rect, Sense::click_and_drag());
        if let Some(tex) = self.viewport_tex {
            ui.painter().image(
                tex,
                rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        }
        if self.playing() {
            ui.painter().text(
                rect.left_top() + egui::vec2(8.0, 8.0),
                egui::Align2::LEFT_TOP,
                "PLAYING (Stop restores the scene)",
                egui::FontId::proportional(14.0),
                Color32::LIGHT_GREEN,
            );
            return;
        }
        let aspect = rect.width() / rect.height().max(1.0);
        let uv_of = |p: egui::Pos2| {
            (
                (p.x - rect.left()) / rect.width().max(1.0),
                (p.y - rect.top()) / rect.height().max(1.0),
            )
        };
        let cam = self.camera.camera();

        // Fly camera: RMB look, WASD/Space/Ctrl move while it is held, wheel dollies.
        if resp.dragged_by(PointerButton::Secondary) {
            let d = resp.drag_delta();
            self.camera.yaw -= d.x * 0.004;
            self.camera.pitch = (self.camera.pitch - d.y * 0.004).clamp(-1.5, 1.5);
            let (dt, keys, fast) = ui.input(|i| {
                let k = |key| f32::from(u8::from(i.key_down(key)));
                (
                    i.stable_dt.min(0.1),
                    Vec3::new(
                        k(Key::D) - k(Key::A),
                        k(Key::Space) - k(Key::Q),
                        k(Key::S) - k(Key::W),
                    ),
                    i.modifiers.shift,
                )
            });
            let speed = 4.0 * if fast { 3.0 } else { 1.0 };
            self.camera.position += self.camera.rotation() * keys * speed * dt;
        }
        if resp.hovered() {
            let scroll = ui.input(|i| i.raw_scroll_delta.y);
            self.camera.position += self.camera.rotation() * Vec3::NEG_Z * scroll * 0.01;
        }

        // Gizmo grab, drag and release.
        let selected = self.selected_entity();
        if resp.drag_started_by(PointerButton::Primary)
            && let (Some(e), Some(p)) = (selected, ui.input(|i| i.pointer.press_origin()))
        {
            let origin = self.world_position(e);
            let len = (origin - cam.position).length().max(0.5) * GIZMO_SCALE;
            let start = pick::project(&cam, origin, aspect)
                .map(|(x, y)| (x * rect.width(), y * rect.height()));
            let pos = (p.x - rect.left(), p.y - rect.top());
            let hit = start.and_then(|s| {
                [Vec3::X, Vec3::Y, Vec3::Z]
                    .into_iter()
                    .enumerate()
                    .filter_map(|(i, axis)| {
                        let (x, y) = pick::project(&cam, origin + axis * len, aspect)?;
                        let d =
                            pick::segment_distance(pos, s, (x * rect.width(), y * rect.height()));
                        (d < GRAB_PIXELS).then_some((d, i, axis))
                    })
                    .min_by(|a, b| a.0.total_cmp(&b.0))
            });
            if let Some((_, i, axis)) = hit {
                let ray = pick::camera_ray(&cam, uv_of(p), aspect);
                if let Some(t) = pick::axis_param(&ray, origin, axis) {
                    self.gizmo = Some(GizmoDrag {
                        axis: i,
                        start_t: t,
                        start_pos: origin,
                    });
                }
            }
        }
        if let Some(g) = self.gizmo {
            if resp.dragged_by(PointerButton::Primary)
                && let (Some(e), Some(p)) = (selected, resp.interact_pointer_pos())
            {
                let axis = [Vec3::X, Vec3::Y, Vec3::Z][g.axis];
                let ray = pick::camera_ray(&cam, uv_of(p), aspect);
                if let Some(t) = pick::axis_param(&ray, g.start_pos, axis) {
                    self.move_to(e, g.start_pos + axis * (t - g.start_t), cmds);
                }
            } else {
                self.gizmo = None;
            }
        }
        if resp.drag_stopped() {
            self.gizmo = None;
        }

        // Click selects (a click on empty space clears the selection).
        if resp.clicked_by(PointerButton::Primary)
            && let Some(p) = resp.interact_pointer_pos()
        {
            let ray = pick::camera_ray(&cam, uv_of(p), aspect);
            cmds.push(C::Select {
                entity: self.pick(&ray).and_then(|e| uuid_ref(self, e)),
            });
        }
    }

    /// Queues a transform that puts `e` at `world` (its parent's space accounted for).
    fn move_to(&self, e: Entity, world: Vec3, cmds: &mut Vec<C>) {
        let Some(me) = uuid_ref(self, e) else { return };
        let parent = self
            .core
            .world
            .parent_of(e)
            .map_or(foundation::math::Mat4::IDENTITY, |p| {
                self.core
                    .world
                    .get::<&scene::GlobalTransform>(p)
                    .map_or(foundation::math::Mat4::IDENTITY, |g| g.matrix)
            });
        let Ok(t) = self.core.world.get::<&scene::Transform>(e) else {
            return;
        };
        let (y, x, z) = t.rotation.to_euler(EulerRot::YXZ);
        cmds.push(C::SetTransform {
            entity: me,
            translation: parent.inverse().transform_point3(world).to_array(),
            rotation_deg: [x, y, z].map(f32::to_degrees),
            scale: t.scale.to_array(),
        });
    }
}

fn format_stats(s: &AssetStats) -> String {
    format!(
        "{} loaded, {} loading, {} failed; cache {}/{} hits",
        s.loaded,
        s.pending,
        s.failed,
        s.cache_hits,
        s.cache_hits + s.cache_misses
    )
}

fn vec3_row(ui: &mut egui::Ui, label: &str, v: &mut [f32; 3], speed: f64) -> bool {
    ui.label(label);
    let mut changed = false;
    ui.horizontal(|ui| {
        for c in v.iter_mut() {
            changed |= ui.add(egui::DragValue::new(c).speed(speed)).changed();
        }
    });
    ui.end_row();
    changed
}

/// Header row of a component; true when its remove button was clicked.
fn component_header(ui: &mut egui::Ui, title: &str) -> bool {
    ui.horizontal(|ui| {
        ui.strong(title);
        ui.small_button("Remove").clicked()
    })
    .inner
}

/// Combo over the assets of `kind` (plus "(default)"); `Some(new)` when the choice changed.
fn asset_combo(
    ui: &mut egui::Ui,
    label: &str,
    infos: &[AssetInfo],
    kind: AssetKind,
    current: assets::AssetId,
) -> Option<assets::AssetId> {
    let title = |i: &AssetInfo| {
        let file = i.path.rsplit('/').next().unwrap_or(&i.path);
        let id = i.id.to_string();
        if i.parent.is_some() {
            format!("{file} [{}]", &id[..8])
        } else {
            file.to_owned()
        }
    };
    let shown = infos
        .iter()
        .find(|i| i.id == current)
        .map_or_else(|| "(default)".to_owned(), title);
    let mut chosen = current;
    egui::ComboBox::from_label(label)
        .selected_text(shown)
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut chosen, assets::AssetId::NIL, "(default)");
            for i in infos.iter().filter(|i| i.kind == kind) {
                ui.selectable_value(&mut chosen, i.id, title(i));
            }
        });
    (chosen != current).then_some(chosen)
}
