//! Pure game rules: formation, bullets, shields, waves, score. No engine types.
//!
//! Coordinates: x across the playfield, y up the playfield (the player is at `PLAYER_Y`).

/// Playfield half width.
pub const HALF_W: f32 = 9.0;
/// Height of the player's row.
pub const PLAYER_Y: f32 = 0.0;
/// Top of the playfield.
pub const TOP: f32 = 13.0;
/// Seconds per simulation step.
pub const DT: f32 = 1.0 / 60.0;
/// Formation columns.
pub const COLS: usize = 9;
/// Formation rows.
pub const ROWS: usize = 5;
const SPACING: (f32, f32) = (1.6, 1.3);
const ENEMY_HALF: f32 = 0.5;
const PLAYER_SPEED: f32 = 9.0;
const PLAYER_HALF: (f32, f32) = (0.7, 0.45);
const FIRE_COOLDOWN: f32 = 0.3;
const INVULN: f32 = 2.0;
const SHIELD_Y: f32 = 3.2;
/// Shield blocks per shield (columns, rows).
pub const SHIELD_GRID: (usize, usize) = (6, 3);
/// Edge of a shield block.
pub const BLOCK: f32 = 0.38;
/// Number of shields.
pub const SHIELDS: usize = 4;
/// Starting lives.
pub const LIVES: u32 = 3;

/// Axis-aligned box, centre and half extents.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aabb {
    /// Centre x.
    pub x: f32,
    /// Centre y.
    pub y: f32,
    /// Half width.
    pub hw: f32,
    /// Half height.
    pub hh: f32,
}

impl Aabb {
    /// Do the boxes overlap?
    pub fn overlaps(&self, o: &Aabb) -> bool {
        (self.x - o.x).abs() < self.hw + o.hw && (self.y - o.y).abs() < self.hh + o.hh
    }
}

/// Enemy design, chosen by row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Glowing core with an orbiting ring (top row).
    Spinner,
    /// Armoured wedge (middle rows).
    Gunship,
    /// Domed walker (bottom rows).
    Crawler,
}

impl Kind {
    /// Score for destroying one.
    pub fn points(self) -> u32 {
        match self {
            Kind::Spinner => 50,
            Kind::Gunship => 30,
            Kind::Crawler => 10,
        }
    }

    fn of_row(row: usize) -> Kind {
        match row {
            0 => Kind::Spinner,
            1 | 2 => Kind::Gunship,
            _ => Kind::Crawler,
        }
    }
}

/// One enemy of the grid; its position derives from the formation.
#[derive(Clone, Copy, Debug)]
pub struct Enemy {
    /// Grid column.
    pub col: usize,
    /// Grid row (0 = top).
    pub row: usize,
    /// Design.
    pub kind: Kind,
    /// Still flying.
    pub alive: bool,
}

/// The marching grid.
#[derive(Clone, Debug)]
pub struct Formation {
    /// All enemies, row-major.
    pub enemies: Vec<Enemy>,
    /// Centre x of the grid.
    pub x: f32,
    /// y of the top row.
    pub y: f32,
    /// +1 right, -1 left.
    pub dir: f32,
}

impl Formation {
    /// A full grid for `wave` (1-based); later waves start lower.
    pub fn new(wave: u32) -> Self {
        let enemies = (0..ROWS * COLS)
            .map(|i| Enemy {
                col: i % COLS,
                row: i / COLS,
                kind: Kind::of_row(i / COLS),
                alive: true,
            })
            .collect();
        Self {
            enemies,
            x: 0.0,
            y: TOP - 0.5 - 0.7 * (wave.saturating_sub(1)).min(3) as f32,
            dir: 1.0,
        }
    }

    /// Centre of enemy `i`.
    pub fn pos(&self, i: usize) -> (f32, f32) {
        let e = &self.enemies[i];
        (
            self.x + (e.col as f32 - (COLS - 1) as f32 * 0.5) * SPACING.0,
            self.y - e.row as f32 * SPACING.1,
        )
    }

    /// Box of enemy `i`.
    pub fn aabb(&self, i: usize) -> Aabb {
        let (x, y) = self.pos(i);
        Aabb {
            x,
            y,
            hw: ENEMY_HALF,
            hh: ENEMY_HALF,
        }
    }

    /// Enemies still flying.
    pub fn alive(&self) -> usize {
        self.enemies.iter().filter(|e| e.alive).count()
    }

    /// Marches sideways; at a wall reverses and steps down. Speed grows as the grid thins.
    pub fn step(&mut self, dt: f32, wave: u32) {
        let alive = self.alive();
        if alive == 0 {
            return;
        }
        let thinned = 1.0 - alive as f32 / (ROWS * COLS) as f32;
        let speed = (1.2 + 0.25 * wave as f32) * (1.0 + 2.0 * thinned);
        self.x += self.dir * speed * dt;
        let (mut lo, mut hi) = (f32::MAX, f32::MIN);
        for (i, e) in self.enemies.iter().enumerate() {
            if e.alive {
                let x = self.pos(i).0;
                lo = lo.min(x);
                hi = hi.max(x);
            }
        }
        let wall = HALF_W - ENEMY_HALF;
        if (self.dir > 0.0 && hi > wall) || (self.dir < 0.0 && lo < -wall) {
            self.x -= if self.dir > 0.0 { hi - wall } else { lo + wall };
            self.dir = -self.dir;
            self.y -= 0.4;
        }
    }

    /// Lowest alive enemy of `col`.
    fn bottom_of(&self, col: usize) -> Option<usize> {
        (0..self.enemies.len())
            .filter(|&i| self.enemies[i].alive && self.enemies[i].col == col)
            .max_by_key(|&i| self.enemies[i].row)
    }
}

/// A shot in flight.
#[derive(Clone, Copy, Debug)]
pub struct Bullet {
    /// x.
    pub x: f32,
    /// y.
    pub y: f32,
    /// Vertical speed (positive = up, the player's).
    pub vy: f32,
}

impl Bullet {
    /// True for the player's shots.
    pub fn friendly(&self) -> bool {
        self.vy > 0.0
    }

    fn aabb(&self) -> Aabb {
        Aabb {
            x: self.x,
            y: self.y,
            hw: 0.12,
            hh: 0.4,
        }
    }
}

/// One destructible shield block.
#[derive(Clone, Copy, Debug)]
pub struct Block {
    /// Centre x.
    pub x: f32,
    /// Centre y.
    pub y: f32,
    /// Hits left (0 = destroyed or never built).
    pub hp: u8,
}

impl Block {
    fn aabb(&self) -> Aabb {
        Aabb {
            x: self.x,
            y: self.y,
            hw: BLOCK * 0.5,
            hh: BLOCK * 0.5,
        }
    }
}

/// What the player asks for this step.
#[derive(Clone, Copy, Debug, Default)]
pub struct Controls {
    /// -1 left .. +1 right.
    pub dir: f32,
    /// Fire held.
    pub fire: bool,
}

/// Something worth showing or logging.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Event {
    /// An enemy blew up at (x, y).
    EnemyKilled(f32, f32),
    /// The player lost a life at x.
    PlayerHit(f32),
    /// The grid was cleared; the next wave starts.
    WaveCleared(u32),
    /// Out of lives.
    GameOver,
}

/// Playing or finished.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Running.
    Playing,
    /// Waiting for a restart.
    GameOver,
}

/// The whole game state.
#[derive(Clone, Debug)]
pub struct Game {
    /// Phase.
    pub phase: Phase,
    /// Ship x.
    pub player_x: f32,
    /// Seconds of invulnerability left.
    pub invuln: f32,
    /// Lives left.
    pub lives: u32,
    /// Score.
    pub score: u32,
    /// Wave number, from 1.
    pub wave: u32,
    /// The grid.
    pub formation: Formation,
    /// Shots in flight.
    pub bullets: Vec<Bullet>,
    /// Shield blocks (destroyed ones stay with hp 0 so indices are stable within a wave).
    pub blocks: Vec<Block>,
    /// Increments whenever formation and shields are rebuilt (wave start, restart).
    pub epoch: u32,
    fire_cd: f32,
    enemy_cd: f32,
    rng: u32,
}

impl Default for Game {
    fn default() -> Self {
        Self::new()
    }
}

impl Game {
    /// A fresh game at wave 1.
    pub fn new() -> Self {
        let mut g = Self {
            phase: Phase::Playing,
            player_x: 0.0,
            invuln: 0.0,
            lives: LIVES,
            score: 0,
            wave: 1,
            formation: Formation::new(1),
            bullets: Vec::new(),
            blocks: Vec::new(),
            epoch: 0,
            fire_cd: 0.0,
            enemy_cd: 1.5,
            rng: 0x9E37_79B9,
        };
        g.reset_wave();
        g
    }

    /// Restart from wave 1 (the epoch keeps counting up so views rebuild).
    pub fn restart(&mut self) {
        let epoch = self.epoch;
        *self = Self::new();
        self.epoch += epoch;
    }

    fn reset_wave(&mut self) {
        self.formation = Formation::new(self.wave);
        self.bullets.clear();
        self.blocks.clear();
        let w = SHIELD_GRID.0 as f32 * BLOCK;
        for s in 0..SHIELDS {
            let cx = (s as f32 - (SHIELDS - 1) as f32 * 0.5) * (2.0 * HALF_W / SHIELDS as f32);
            for r in 0..SHIELD_GRID.1 {
                for c in 0..SHIELD_GRID.0 {
                    // Notch the bottom middle so the shield reads as an arch.
                    let hp = if r == 0 && (2..4).contains(&c) { 0 } else { 2 };
                    self.blocks.push(Block {
                        x: cx - w * 0.5 + (c as f32 + 0.5) * BLOCK,
                        y: SHIELD_Y + r as f32 * BLOCK,
                        hp,
                    });
                }
            }
        }
        self.epoch += 1;
    }

    fn rand(&mut self) -> f32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        (self.rng >> 8) as f32 / (1u32 << 24) as f32
    }

    fn player_box(&self) -> Aabb {
        Aabb {
            x: self.player_x,
            y: PLAYER_Y,
            hw: PLAYER_HALF.0,
            hh: PLAYER_HALF.1,
        }
    }

    /// Advances one step of `dt` seconds.
    pub fn step(&mut self, c: Controls, dt: f32) -> Vec<Event> {
        let mut ev = Vec::new();
        if self.phase != Phase::Playing {
            return ev;
        }
        let limit = HALF_W - PLAYER_HALF.0;
        self.player_x =
            (self.player_x + c.dir.clamp(-1.0, 1.0) * PLAYER_SPEED * dt).clamp(-limit, limit);
        self.invuln = (self.invuln - dt).max(0.0);
        self.fire_cd -= dt;
        if c.fire && self.fire_cd <= 0.0 {
            self.fire_cd = FIRE_COOLDOWN;
            self.bullets.push(Bullet {
                x: self.player_x,
                y: PLAYER_Y + 0.8,
                vy: 20.0,
            });
        }

        self.formation.step(dt, self.wave);
        self.enemy_fire(dt);
        for b in &mut self.bullets {
            b.y += b.vy * dt;
        }
        self.bullets.retain(|b| (-1.0..=TOP + 2.0).contains(&b.y));
        self.collide(&mut ev);
        if self.phase == Phase::GameOver {
            return ev;
        }

        // Enemies chew through shields they touch and win by reaching the player's row.
        let f = &self.formation;
        for i in (0..f.enemies.len()).filter(|&i| f.enemies[i].alive) {
            let a = f.aabb(i);
            for blk in self.blocks.iter_mut().filter(|b| b.hp > 0) {
                if a.overlaps(&blk.aabb()) {
                    blk.hp = 0;
                }
            }
            if a.y - a.hh <= PLAYER_Y + PLAYER_HALF.1 {
                self.lives = 0;
                self.phase = Phase::GameOver;
                ev.push(Event::GameOver);
                return ev;
            }
        }

        if self.formation.alive() == 0 {
            self.score += 100 * self.wave;
            ev.push(Event::WaveCleared(self.wave));
            self.wave += 1;
            self.reset_wave();
        }
        ev
    }

    fn enemy_fire(&mut self, dt: f32) {
        self.enemy_cd -= dt;
        if self.enemy_cd > 0.0 {
            return;
        }
        let interval = (1.1 - 0.1 * self.wave as f32).max(0.35);
        self.enemy_cd = interval * (0.5 + self.rand());
        let enemy_shots = self.bullets.iter().filter(|b| !b.friendly()).count();
        if enemy_shots >= 3 + self.wave as usize {
            return;
        }
        let start = (self.rand() * COLS as f32) as usize;
        // First column from a random start that still has a shooter.
        if let Some(i) = (0..COLS).find_map(|k| self.formation.bottom_of((start + k) % COLS)) {
            let (x, y) = self.formation.pos(i);
            self.bullets.push(Bullet {
                x,
                y: y - 0.6,
                vy: -(6.0 + 0.6 * self.wave.min(8) as f32),
            });
        }
    }

    fn collide(&mut self, ev: &mut Vec<Event>) {
        let mut spent = vec![false; self.bullets.len()];
        for (bi, b) in self.bullets.iter().enumerate() {
            let a = b.aabb();
            if b.friendly() {
                let f = &mut self.formation;
                // Own shots fly over the player's shields.
                if let Some(i) =
                    (0..f.enemies.len()).find(|&i| f.enemies[i].alive && f.aabb(i).overlaps(&a))
                {
                    let (x, y) = f.pos(i);
                    f.enemies[i].alive = false;
                    self.score += f.enemies[i].kind.points();
                    ev.push(Event::EnemyKilled(x, y));
                    spent[bi] = true;
                }
            } else if let Some(blk) = self
                .blocks
                .iter_mut()
                .find(|k| k.hp > 0 && k.aabb().overlaps(&a))
            {
                blk.hp -= 1;
                spent[bi] = true;
            } else if self.invuln <= 0.0 && self.player_box().overlaps(&a) {
                spent[bi] = true;
                self.lives -= 1;
                self.invuln = INVULN;
                ev.push(Event::PlayerHit(self.player_x));
                if self.lives == 0 {
                    self.phase = Phase::GameOver;
                    ev.push(Event::GameOver);
                    return;
                }
            }
        }
        let mut i = 0;
        self.bullets.retain(|_| {
            i += 1;
            !spent[i - 1]
        });
        if ev.iter().any(|e| matches!(e, Event::PlayerHit(_))) {
            self.bullets.retain(|b| b.friendly());
        }
    }
}

/// Crude autopilot: chase the nearest enemy column (aiming slightly ahead of the march), sidestep falling shots, fire.
pub fn autopilot(g: &Game) -> Controls {
    let px = g.player_x;
    let f = &g.formation;
    let target = (0..f.enemies.len())
        .filter(|&i| f.enemies[i].alive)
        .map(|i| f.pos(i).0 + f.dir * 0.6)
        .min_by(|a, b| (a - px).abs().total_cmp(&(b - px).abs()));
    let mut dir = match target {
        Some(x) if (x - px).abs() > 0.15 => (x - px).signum(),
        _ => 0.0,
    };
    // Falling shot close above us: step away from it (towards the middle if cornered).
    if let Some(b) = g
        .bullets
        .iter()
        .filter(|b| !b.friendly() && b.y < PLAYER_Y + 4.5 && (b.x - px).abs() < 0.9)
        .min_by(|a, b| a.y.total_cmp(&b.y))
    {
        dir = if b.x >= px { -1.0 } else { 1.0 };
        if (px + dir).abs() > HALF_W - 1.0 {
            dir = -px.signum();
        }
    }
    Controls { dir, fire: true }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quiet() -> Game {
        let mut g = Game::new();
        g.enemy_cd = f32::MAX;
        g
    }

    fn bx(x: f32) -> Aabb {
        Aabb {
            x,
            y: 0.0,
            hw: 1.0,
            hh: 1.0,
        }
    }

    #[test]
    fn aabb_overlap() {
        assert!(bx(0.0).overlaps(&bx(1.5)));
        assert!(!bx(0.0).overlaps(&bx(2.5)));
    }

    #[test]
    fn formation_reverses_and_descends_at_wall() {
        let mut f = Formation::new(1);
        let y0 = f.y;
        for _ in 0..600 {
            f.step(DT, 1);
        }
        assert!(f.y < y0, "stepped down");
        let hi = (0..f.enemies.len())
            .map(|i| f.pos(i).0)
            .fold(f32::MIN, f32::max);
        assert!(hi <= HALF_W, "stays inside the playfield");
    }

    #[test]
    fn formation_speeds_up_when_thinned() {
        let (mut full, mut thin) = (Formation::new(1), Formation::new(1));
        for e in thin.enemies.iter_mut().skip(5) {
            e.alive = false;
        }
        full.step(0.1, 1);
        thin.step(0.1, 1);
        assert!(thin.x > full.x);
    }

    #[test]
    fn fire_respects_cooldown() {
        let mut g = quiet();
        let fire = Controls {
            dir: 0.0,
            fire: true,
        };
        g.step(fire, DT);
        g.step(fire, DT);
        assert_eq!(g.bullets.len(), 1);
        for _ in 0..30 {
            g.step(fire, DT);
        }
        // The first shot has meanwhile hit a crawler (10 points).
        assert!(g.bullets.len() as u32 + g.score / 10 >= 2);
    }

    #[test]
    fn player_bullet_kills_enemy_and_scores() {
        let mut g = quiet();
        let (x, y) = g.formation.pos(0);
        g.bullets.push(Bullet {
            x,
            y: y - 0.3,
            vy: 20.0,
        });
        let ev = g.step(Controls::default(), DT);
        assert!(ev.iter().any(|e| matches!(e, Event::EnemyKilled(..))));
        assert_eq!(g.score, Kind::Spinner.points());
        assert!(!g.formation.enemies[0].alive);
    }

    #[test]
    fn enemy_bullet_damages_shield_then_destroys() {
        let mut g = quiet();
        let b = g.blocks.iter().position(|b| b.hp > 0).unwrap();
        let (x, y) = (g.blocks[b].x, g.blocks[b].y);
        for expected in [1, 0] {
            g.bullets.push(Bullet { x, y, vy: -1.0 });
            g.step(Controls::default(), DT);
            assert_eq!(g.blocks[b].hp, expected);
        }
    }

    #[test]
    fn player_hit_costs_life_then_game_over() {
        let mut g = quiet();
        for expect in [2, 1, 0] {
            g.invuln = 0.0;
            g.bullets.push(Bullet {
                x: g.player_x,
                y: PLAYER_Y,
                vy: -1.0,
            });
            g.step(Controls::default(), DT);
            assert_eq!(g.lives, expect);
        }
        assert_eq!(g.phase, Phase::GameOver);
        let (score, x) = (g.score, g.player_x);
        g.step(
            Controls {
                dir: 1.0,
                fire: true,
            },
            DT,
        );
        assert_eq!((g.score, g.player_x), (score, x), "frozen after game over");
        g.restart();
        assert_eq!((g.phase, g.lives, g.wave), (Phase::Playing, LIVES, 1));
    }

    #[test]
    fn clearing_the_grid_starts_the_next_wave() {
        let mut g = quiet();
        let epoch = g.epoch;
        for e in &mut g.formation.enemies {
            e.alive = false;
        }
        let ev = g.step(Controls::default(), DT);
        assert!(ev.contains(&Event::WaveCleared(1)));
        assert_eq!(
            (g.wave, g.formation.alive(), g.score),
            (2, ROWS * COLS, 100)
        );
        assert!(g.epoch > epoch);
    }

    #[test]
    fn reaching_the_player_row_ends_the_game() {
        let mut g = quiet();
        g.formation.y = PLAYER_Y + 4.0 * SPACING.1 + 0.8;
        let ev = g.step(Controls::default(), DT);
        assert!(ev.contains(&Event::GameOver));
    }

    #[test]
    fn autopilot_scores() {
        let mut g = Game::new();
        for _ in 0..1800 {
            let c = autopilot(&g);
            g.step(c, DT);
        }
        assert!(g.score >= 200, "score {}", g.score);
    }
}
