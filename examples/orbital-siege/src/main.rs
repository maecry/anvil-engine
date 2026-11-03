//! Orbital Siege: a 3D arcade fixed-shooter on the Anvil game API (`runtime`, `scene`, `assets`).
//!
//! `game.rs` holds the rules (pure Rust, unit tested); this file maps the game state onto
//! entities each frame. Env: `ORBITAL_AUTOPILOT=1` plays itself, `ORBITAL_PROJECT=dir` overrides
//! the generated project (`project/`, see `tests/generate_project.rs`), plus the `ANVIL_*` run
//! options of the player (`ANVIL_WINDOW=0`, `ANVIL_EXIT_AFTER_FRAMES`, `ANVIL_SCREENSHOT`...).

mod game;

use assets::AssetId;
use foundation::math::{Quat, Vec3};
use game::{Controls, DT, Event, Game, HALF_W, Kind, LIVES, Phase};
use platform::KeyCode;
use runtime::{Application, Context, RunOptions, Runtime, RuntimeError};
use scene::{Camera, Light, LightKind, MeshRenderer, Transform, World};

type Entity = hecs::Entity;

/// Meshes of `shapes.glb`, in file order.
#[derive(Clone, Copy)]
enum Shape {
    Cube,
    Sphere,
    Cone,
    Cylinder,
}

macro_rules! materials {
    ($($v:ident = $n:literal),*) => {
        /// Materials of the project, by file name.
        #[derive(Clone, Copy)]
        enum Mat { $($v),* }
        const MAT_NAMES: &[&str] = &[$($n),*];
    };
}
materials!(
    Hull = "hull",
    HullDark = "hull_dark",
    GlowCyan = "glow_cyan",
    Spinner = "spinner",
    Gunship = "gunship",
    Crawler = "crawler",
    Eye = "eye",
    Shield = "shield",
    ShieldHurt = "shield_hurt",
    BoltPlayer = "bolt_player",
    BoltEnemy = "bolt_enemy",
    Ground = "ground",
    Rail = "rail",
    Grid = "grid",
    Spark = "spark",
    HudScore = "hud_score",
    HudWave = "hud_wave"
);

/// Asset ids of the meshes and materials.
struct Look {
    meshes: Vec<AssetId>,
    mats: Vec<AssetId>,
}

/// Game (x, y, height) to world space: the player's row is z = 6 and enemies march towards +z.
fn at(x: f32, y: f32, h: f32) -> Vec3 {
    Vec3::new(x, h, 6.0 - y)
}

const HIDDEN: Vec3 = Vec3::new(0.0, -60.0, 0.0);
const SCORE_BLOCKS: usize = 24;
const WAVE_PIPS: usize = 8;

impl Look {
    fn load(ctx: &Context) -> Self {
        let glb = ctx
            .assets
            .id_for_path("assets/models/shapes.glb")
            .unwrap_or(AssetId::NIL);
        Self {
            meshes: (0..4)
                .map(|i| AssetId::derive(glb, &format!("mesh/{i}/0")))
                .collect(),
            mats: MAT_NAMES
                .iter()
                .map(|n| {
                    ctx.assets
                        .id_for_path(&format!("assets/materials/{n}.material"))
                        .unwrap_or_else(|| {
                            tracing::error!(material = n, "material missing, run the generator");
                            AssetId::NIL
                        })
                })
                .collect(),
        }
    }

    fn renderer(&self, shape: Shape, mat: Mat) -> MeshRenderer {
        MeshRenderer {
            mesh: self.meshes[shape as usize],
            material: self.mats[mat as usize],
        }
    }

    /// Spawns one primitive, optionally under `parent`.
    #[allow(clippy::too_many_arguments)] // a transform's worth of plain values reads best at call sites
    fn part(
        &self,
        w: &mut World,
        parent: Option<Entity>,
        shape: Shape,
        mat: Mat,
        pos: [f32; 3],
        scale: [f32; 3],
        rot: Quat,
    ) -> Entity {
        let e = w.spawn_entity("part");
        let _ = w.insert_one(
            e,
            Transform {
                translation: pos.into(),
                rotation: rot,
                scale: scale.into(),
            },
        );
        let _ = w.insert_one(e, self.renderer(shape, mat));
        if parent.is_some() {
            let _ = w.set_parent(e, parent);
        }
        e
    }

    /// An empty root entity at `pos`.
    fn root(w: &mut World, name: &str, pos: Vec3) -> Entity {
        let e = w.spawn_entity(name);
        let _ = w.insert_one(e, Transform::from_translation(pos));
        e
    }

    /// The player's ship, nose towards -z, about 2 units wide.
    fn ship(&self, w: &mut World, scale: f32) -> Entity {
        use Mat::*;
        use Shape::*;
        let r = Self::root(w, "ship", HIDDEN);
        place(
            w,
            r,
            Transform {
                scale: Vec3::splat(scale),
                translation: HIDDEN,
                ..Transform::IDENTITY
            },
        );
        let p = Some(r);
        let nose = Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2);
        self.part(w, p, Cone, Hull, [0.0, 0.0, -0.15], [0.55, 1.5, 0.55], nose);
        self.part(
            w,
            p,
            Cube,
            HullDark,
            [0.0, -0.05, 0.3],
            [1.0, 0.22, 0.7],
            Quat::IDENTITY,
        );
        for s in [-1.0, 1.0] {
            let sweep = Quat::from_rotation_y(s * 0.35) * Quat::from_rotation_z(s * -0.12);
            self.part(
                w,
                p,
                Cube,
                Hull,
                [s * 0.75, -0.02, 0.2],
                [0.9, 0.07, 0.55],
                sweep,
            );
            self.part(
                w,
                p,
                Cube,
                GlowCyan,
                [s * 1.15, 0.0, 0.15],
                [0.09, 0.14, 0.7],
                sweep,
            );
        }
        self.part(
            w,
            p,
            Sphere,
            GlowCyan,
            [0.0, 0.0, 0.7],
            [0.4, 0.3, 0.4],
            Quat::IDENTITY,
        );
        self.part(
            w,
            p,
            Sphere,
            Eye,
            [0.0, 0.2, 0.05],
            [0.28, 0.2, 0.5],
            Quat::IDENTITY,
        );
        r
    }

    /// One enemy of `kind`, facing +z (towards the player).
    fn enemy(&self, w: &mut World, kind: Kind) -> Entity {
        use Mat::*;
        use Shape::*;
        let r = Self::root(w, "enemy", HIDDEN);
        let p = Some(r);
        let q = Quat::IDENTITY;
        match kind {
            Kind::Spinner => {
                self.part(w, p, Sphere, Mat::Spinner, [0.0; 3], [0.62; 3], q);
                self.part(w, p, Sphere, Eye, [0.0, 0.0, 0.3], [0.22; 3], q);
                self.part(w, p, Cylinder, Hull, [0.0; 3], [1.15, 0.05, 1.15], q);
                for (x, z) in [(0.55, 0.0), (-0.55, 0.0), (0.0, 0.55), (0.0, -0.55)] {
                    self.part(w, p, Cube, HullDark, [x, 0.0, z], [0.2; 3], q);
                }
            }
            Kind::Gunship => {
                self.part(w, p, Cube, Mat::Gunship, [0.0; 3], [0.8, 0.32, 0.7], q);
                let fwd = Quat::from_rotation_x(std::f32::consts::FRAC_PI_2);
                self.part(
                    w,
                    p,
                    Cone,
                    HullDark,
                    [0.0, 0.0, 0.5],
                    [0.34, 0.5, 0.34],
                    fwd,
                );
                self.part(w, p, Sphere, Eye, [0.0, 0.2, 0.05], [0.2; 3], q);
                for s in [-1.0, 1.0] {
                    let tilt = Quat::from_rotation_z(s * 0.4);
                    self.part(
                        w,
                        p,
                        Cube,
                        HullDark,
                        [s * 0.6, 0.05, -0.05],
                        [0.55, 0.07, 0.5],
                        tilt,
                    );
                    self.part(
                        w,
                        p,
                        Cube,
                        Mat::Gunship,
                        [s * 0.9, 0.18, -0.05],
                        [0.08, 0.3, 0.5],
                        q,
                    );
                }
            }
            Kind::Crawler => {
                self.part(
                    w,
                    p,
                    Sphere,
                    Mat::Crawler,
                    [0.0, 0.08, 0.0],
                    [0.9, 0.62, 0.9],
                    q,
                );
                for s in [-1.0, 1.0] {
                    self.part(w, p, Sphere, Eye, [s * 0.2, 0.22, 0.32], [0.16; 3], q);
                    let leg = Quat::from_rotation_z(s * 0.5);
                    self.part(
                        w,
                        p,
                        Cylinder,
                        HullDark,
                        [s * 0.38, -0.3, 0.0],
                        [0.12, 0.42, 0.12],
                        leg,
                    );
                    self.part(
                        w,
                        p,
                        Cylinder,
                        HullDark,
                        [s * 0.15, -0.32, 0.3],
                        [0.1, 0.36, 0.1],
                        q,
                    );
                }
            }
        }
        r
    }
}

fn place(w: &mut World, e: Entity, t: Transform) {
    if let Ok(old) = w.query_one_mut::<&mut Transform>(e) {
        *old = t;
    }
}

fn despawn(w: &mut World, e: &mut Option<Entity>) {
    if let Some(e) = e.take() {
        w.despawn_recursive(e);
    }
}

/// A short-lived debris cube.
struct Spark {
    e: Entity,
    v: Vec3,
    life: f32,
}

/// Entities that mirror the game state.
struct View {
    epoch: u32,
    ship: Entity,
    bank: f32,
    enemies: Vec<Option<Entity>>,
    /// (entity, hp shown).
    blocks: Vec<Option<(Entity, u8)>>,
    bullets: Vec<Entity>,
    lives: Vec<Entity>,
    score: Vec<Entity>,
    waves: Vec<Entity>,
    sparks: Vec<Spark>,
}

struct Siege {
    game: Game,
    look: Option<Look>,
    view: Option<View>,
    auto: bool,
    /// One step per frame (autopilot and headless): deterministic.
    fixed: bool,
    acc: f32,
    tick: u64,
    over_ticks: u32,
    last_dir: f32,
}

impl Siege {
    fn new() -> Self {
        let auto = std::env::var("ORBITAL_AUTOPILOT").as_deref() == Ok("1");
        Self {
            game: Game::new(),
            look: None,
            view: None,
            auto,
            fixed: auto || std::env::var("ANVIL_WINDOW").as_deref() == Ok("0"),
            acc: 0.0,
            tick: 0,
            over_ticks: 0,
            last_dir: 0.0,
        }
    }

    fn controls(&self, ctx: &Context) -> Controls {
        if self.auto {
            return game::autopilot(&self.game);
        }
        let k = |c| ctx.input.key_down(c);
        let right = k(KeyCode::ArrowRight) || k(KeyCode::D);
        let left = k(KeyCode::ArrowLeft) || k(KeyCode::A);
        Controls {
            dir: f32::from(u8::from(right)) - f32::from(u8::from(left)),
            fire: k(KeyCode::Space),
        }
    }

    fn spawn_scene(look: &Look, w: &mut World) {
        use Mat::*;
        use Shape::*;
        let q = Quat::IDENTITY;
        // Camera above and behind the ship, looking up the playfield.
        let eye = Vec3::new(0.0, 14.5, 14.5);
        let (_, rotation, translation) = foundation::math::camera::rh::view::look_at_mat4(
            eye,
            Vec3::new(0.0, 0.0, 0.8),
            Vec3::Y,
        )
        .inverse()
        .to_scale_rotation_translation();
        let cam = w.spawn_entity("camera");
        let _ = w.insert_one(
            cam,
            Transform {
                translation,
                rotation,
                scale: Vec3::ONE,
            },
        );
        let _ = w.insert_one(
            cam,
            Camera {
                fov_y: 46f32.to_radians(),
                ..Camera::default()
            },
        );
        let light = |w: &mut World, name, t: Transform, l: Light| {
            let e = w.spawn_entity(name);
            let _ = w.insert_one(e, t);
            let _ = w.insert_one(e, l);
        };
        light(
            w,
            "sun",
            Transform {
                rotation: Quat::from_rotation_arc(
                    Vec3::NEG_Z,
                    Vec3::new(-0.35, -1.0, -0.45).normalize(),
                ),
                ..Transform::IDENTITY
            },
            Light {
                kind: LightKind::Directional,
                intensity: 2.5,
                color: [0.8, 0.88, 1.0],
                cast_shadows: true,
                ..Light::default()
            },
        );
        for (name, pos, color, intensity) in [
            ("ship glow", at(0.0, 0.0, 2.5), [0.2, 0.8, 1.0], 45.0),
            ("enemy glow", at(0.0, 9.0, 3.0), [1.0, 0.2, 0.6], 50.0),
        ] {
            light(
                w,
                name,
                Transform::from_translation(pos),
                Light {
                    color,
                    intensity,
                    range: 18.0,
                    ..Light::default()
                },
            );
        }
        // Deck, glowing rails and a floor line under the HUD.
        look.part(
            w,
            None,
            Cube,
            Ground,
            [0.0, -0.25, -2.0],
            [90.0, 0.4, 90.0],
            q,
        );
        for s in [-1.0, 1.0] {
            look.part(
                w,
                None,
                Cube,
                Rail,
                [s * (HALF_W + 0.4), 0.0, -2.0],
                [0.14, 0.14, 24.0],
                q,
            );
        }
        look.part(
            w,
            None,
            Cube,
            Rail,
            [0.0, 0.0, 7.2],
            [2.0 * HALF_W + 0.9, 0.12, 0.12],
            q,
        );
        for i in -10..=10 {
            let d = i as f32 * 3.0;
            look.part(w, None, Cube, Grid, [d, -0.02, -2.0], [0.05, 0.05, 80.0], q);
            look.part(
                w,
                None,
                Cube,
                Grid,
                [0.0, -0.02, d - 2.0],
                [80.0, 0.05, 0.05],
                q,
            );
        }
    }

    fn build_view(&self, look: &Look, w: &mut World) -> View {
        let ship = look.ship(w, 0.75);
        let mini = |w: &mut World, i: usize| {
            let e = look.ship(w, 0.3);
            place(
                w,
                e,
                Transform {
                    translation: at(-8.4 + i as f32 * 1.1, -2.1, 0.2),
                    scale: Vec3::splat(0.3),
                    ..Transform::IDENTITY
                },
            );
            e
        };
        let lives = (0..LIVES as usize).map(|i| mini(w, i)).collect();
        let pips = |w: &mut World, n, shape, mat, scale: f32| -> Vec<Entity> {
            (0..n)
                .map(|_| {
                    look.part(
                        w,
                        None,
                        shape,
                        mat,
                        HIDDEN.into(),
                        [scale; 3],
                        Quat::IDENTITY,
                    )
                })
                .collect()
        };
        View {
            epoch: u32::MAX,
            ship,
            bank: 0.0,
            enemies: Vec::new(),
            blocks: Vec::new(),
            bullets: Vec::new(),
            lives,
            score: pips(w, SCORE_BLOCKS, Shape::Cube, Mat::HudScore, 0.24),
            waves: pips(w, WAVE_PIPS, Shape::Sphere, Mat::HudWave, 0.3),
            sparks: Vec::new(),
        }
    }

    fn burst(look: &Look, v: &mut View, w: &mut World, x: f32, y: f32, n: usize, speed: f32) {
        for i in 0..n {
            let a = i as f32 / n as f32 * std::f32::consts::TAU + y;
            let dir = Vec3::new(a.cos(), 0.6 + (i % 3) as f32 * 0.4, a.sin());
            let e = look.part(
                w,
                None,
                Shape::Cube,
                Mat::Spark,
                at(x, y, 0.5).to_array(),
                [0.16; 3],
                Quat::from_rotation_y(a),
            );
            v.sparks.push(Spark {
                e,
                v: dir * speed,
                life: 0.7,
            });
        }
    }

    /// Mirrors the game state onto entities.
    fn sync(&mut self, w: &mut World, sim_dt: f32) {
        let (Some(look), Some(v)) = (&self.look, &mut self.view) else {
            return;
        };
        let g = &self.game;
        let t = self.tick as f32 * DT;

        if v.epoch != g.epoch {
            v.epoch = g.epoch;
            for e in &mut v.enemies {
                despawn(w, e);
            }
            for b in v.blocks.drain(..).flatten() {
                w.despawn_recursive(b.0);
            }
            v.enemies = g
                .formation
                .enemies
                .iter()
                .map(|e| Some(look.enemy(w, e.kind)))
                .collect();
            v.blocks = g
                .blocks
                .iter()
                .map(|b| {
                    (b.hp > 0).then(|| {
                        let e = look.part(
                            w,
                            None,
                            Shape::Cube,
                            Mat::Shield,
                            at(b.x, b.y, 0.3).to_array(),
                            [game::BLOCK * 0.94, game::BLOCK * 0.9, game::BLOCK * 0.94],
                            Quat::IDENTITY,
                        );
                        (e, b.hp)
                    })
                })
                .collect();
        }

        for (i, slot) in v.enemies.iter_mut().enumerate() {
            if !g.formation.enemies[i].alive {
                despawn(w, slot);
                continue;
            }
            let Some(e) = *slot else { continue };
            let (x, y) = g.formation.pos(i);
            let ph =
                g.formation.enemies[i].col as f32 * 0.9 + g.formation.enemies[i].row as f32 * 0.5;
            let mut tr = Transform::from_translation(at(x, y, 0.5));
            match g.formation.enemies[i].kind {
                Kind::Spinner => {
                    tr.translation.y += 0.1 * (t * 3.0 + ph).sin();
                    tr.rotation = Quat::from_rotation_y(t * 1.8 + ph) * Quat::from_rotation_x(0.35);
                }
                Kind::Gunship => tr.rotation = Quat::from_rotation_z(0.18 * (t * 2.0 + ph).sin()),
                Kind::Crawler => tr.scale.y = 1.0 + 0.1 * (t * 6.0 + ph).sin(),
            }
            place(w, e, tr);
        }

        for (b, slot) in g.blocks.iter().zip(v.blocks.iter_mut()) {
            match slot {
                Some((e, _)) if b.hp == 0 => {
                    w.despawn_recursive(*e);
                    *slot = None;
                }
                Some((e, shown)) if *shown != b.hp => {
                    *shown = b.hp;
                    let _ = w.insert_one(*e, look.renderer(Shape::Cube, Mat::ShieldHurt));
                }
                _ => {}
            }
        }

        while v.bullets.len() < g.bullets.len() {
            let e = look.part(
                w,
                None,
                Shape::Sphere,
                Mat::BoltPlayer,
                HIDDEN.into(),
                [0.2, 0.2, 0.9],
                Quat::IDENTITY,
            );
            v.bullets.push(e);
        }
        for (i, &e) in v.bullets.iter().enumerate() {
            match g.bullets.get(i) {
                Some(b) => {
                    let mat = if b.friendly() {
                        Mat::BoltPlayer
                    } else {
                        Mat::BoltEnemy
                    };
                    let _ = w.insert_one(e, look.renderer(Shape::Sphere, mat));
                    place(
                        w,
                        e,
                        Transform {
                            translation: at(b.x, b.y, 0.45),
                            scale: Vec3::new(0.2, 0.2, 0.9),
                            ..Transform::IDENTITY
                        },
                    );
                }
                None => place(w, e, Transform::from_translation(HIDDEN)),
            }
        }

        let blink = g.invuln > 0.0 && ((g.invuln * 10.0) as u32).is_multiple_of(2);
        let shown = g.phase == Phase::Playing && !blink;
        v.bank += (self.last_dir - v.bank) * 0.2;
        place(
            w,
            v.ship,
            Transform {
                translation: if shown {
                    at(g.player_x, 0.0, 0.35)
                } else {
                    HIDDEN
                },
                rotation: Quat::from_rotation_z(-v.bank * 0.35),
                scale: Vec3::splat(0.75),
            },
        );

        for (i, &e) in v.lives.iter().enumerate() {
            let pos = if i < g.lives as usize {
                at(-8.4 + i as f32 * 1.1, -2.1, 0.2)
            } else {
                HIDDEN
            };
            place(
                w,
                e,
                Transform {
                    translation: pos,
                    scale: Vec3::splat(0.3),
                    ..Transform::IDENTITY
                },
            );
        }
        let row = |w: &mut World, ents: &[Entity], n: usize, x0: f32, dx: f32, scale: f32| {
            for (i, &e) in ents.iter().enumerate() {
                let pos = if i < n {
                    at(x0 + i as f32 * dx, -2.1, 0.2)
                } else {
                    HIDDEN
                };
                place(
                    w,
                    e,
                    Transform {
                        translation: pos,
                        scale: Vec3::splat(scale),
                        ..Transform::IDENTITY
                    },
                );
            }
        };
        row(w, &v.score, (g.score / 100) as usize, 0.9, 0.33, 0.24);
        row(w, &v.waves, g.wave as usize, -4.6, 0.5, 0.3);

        for s in &mut v.sparks {
            s.life -= sim_dt;
            s.v.y -= 7.0 * sim_dt;
            if let Ok(tr) = w.query_one_mut::<&mut Transform>(s.e) {
                tr.translation += s.v * sim_dt;
                tr.scale = Vec3::splat(0.16 * (s.life / 0.7).max(0.0));
            }
        }
        for s in v.sparks.iter().filter(|s| s.life <= 0.0) {
            w.despawn_recursive(s.e);
        }
        v.sparks.retain(|s| s.life > 0.0);
    }
}

impl Application for Siege {
    fn start(&mut self, ctx: &mut Context) {
        let look = Look::load(ctx);
        Self::spawn_scene(&look, ctx.world);
        self.view = Some(self.build_view(&look, ctx.world));
        self.look = Some(look);
        tracing::info!(
            autopilot = self.auto,
            fixed_step = self.fixed,
            "Orbital Siege: Left/Right or A/D move, Space fires, R restarts, Esc quits"
        );
    }

    fn update(&mut self, ctx: &mut Context, dt: f32) {
        let steps = if self.fixed {
            1
        } else {
            self.acc += dt;
            let n = ((self.acc / DT) as u32).min(5);
            self.acc = (self.acc - n as f32 * DT).min(DT);
            n
        };
        let restart = ctx.input.key_down(KeyCode::R) || (self.auto && self.over_ticks > 90);
        if self.game.phase == Phase::GameOver {
            self.over_ticks += 1;
            if restart {
                self.game.restart();
                self.over_ticks = 0;
                tracing::info!("restart");
            }
        }
        for _ in 0..steps {
            let c = self.controls(ctx);
            self.last_dir = c.dir;
            self.tick += 1;
            for e in self.game.step(c, DT) {
                self.on_event(ctx.world, e);
            }
            if self.tick.is_multiple_of(60) {
                let g = &self.game;
                tracing::info!(
                    tick = self.tick,
                    score = g.score,
                    lives = g.lives,
                    wave = g.wave,
                    enemies = g.formation.alive(),
                    "status"
                );
            }
        }
        self.sync(ctx.world, steps as f32 * DT);
    }
}

impl Siege {
    fn on_event(&mut self, w: &mut World, e: Event) {
        let (Some(look), Some(v)) = (&self.look, &mut self.view) else {
            return;
        };
        let g = &self.game;
        match e {
            Event::EnemyKilled(x, y) => Self::burst(look, v, w, x, y, 8, 3.5),
            Event::PlayerHit(x) => {
                tracing::info!(lives = g.lives, score = g.score, "ship hit");
                Self::burst(look, v, w, x, 0.0, 16, 5.0);
            }
            Event::WaveCleared(n) => tracing::info!(
                cleared = n,
                score = g.score,
                "wave cleared, next wave is faster"
            ),
            Event::GameOver => {
                tracing::info!(score = g.score, wave = g.wave, "GAME OVER (R restarts)");
                Self::burst(look, v, w, g.player_x, 0.0, 24, 6.0);
            }
        }
    }
}

fn main() -> anyhow::Result<()> {
    foundation::init_logging()?;
    let dir = std::env::var_os("ORBITAL_PROJECT")
        .unwrap_or_else(|| concat!(env!("CARGO_MANIFEST_DIR"), "/project").into());
    let report = match Runtime::run(RunOptions::from_env(dir), Siege::new()) {
        Ok(r) => r,
        Err(RuntimeError::DeviceLost) => {
            tracing::error!("GPU device lost, exiting with code 3");
            std::process::exit(3);
        }
        Err(e) => return Err(e.into()),
    };
    tracing::info!(?report, "Orbital Siege finished");
    if report.validation_errors > 0 {
        std::process::exit(1);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_material_has_a_file() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("project/assets/materials");
        for n in MAT_NAMES {
            assert!(dir.join(format!("{n}.material")).exists(), "{n}");
        }
    }
}
