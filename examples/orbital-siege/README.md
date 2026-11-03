# Orbital Siege

A small original 3D arcade fixed-shooter built on the Anvil game API (`runtime`, `scene`, `assets`).
Your ship slides along the near edge of a neon deck and fires at a marching formation of three
enemy designs (spinner, gunship, crawler). They step down at the walls, shoot back, and speed up as
the grid thins and on every wave. Four shield blocks absorb enemy fire (your own shots pass over them).

```sh
cargo run -p orbital-siege
```

| Key | Action |
|---|---|
| Left / Right, A / D | Move |
| Space | Fire |
| R | Restart after game over |
| Esc | Quit |

Scoring: spinner 50, gunship 30, crawler 10, wave clear 100 x wave. Three lives. HUD (no text): ship
models are lives, violet pips are the wave, one gold block per 100 points. Score, lives and wave are
also logged (`tracing::info!`, once a second of game time and on every hit, wave and game over).

## AI-observable run

```sh
ANVIL_WINDOW=0 ANVIL_VSYNC=0 ANVIL_EXIT_AFTER_FRAMES=600 ANVIL_SCREENSHOT=out.png ORBITAL_AUTOPILOT=1 cargo run -p orbital-siege
```

`ORBITAL_AUTOPILOT=1` plays itself (and restarts after game over). Autopilot and headless runs use
one fixed 1/60 s step per frame, so they are deterministic. The standard `ANVIL_*` run options of the
player apply.

## Layout

- `src/game.rs`: the rules as pure Rust (formation, AABB collisions, cooldowns, waves, autopilot), unit tested.
- `src/main.rs`: engine glue; mirrors the game state onto entities every frame.
- `project/`: the generated asset project (primitive meshes in `shapes.glb` and the materials).
  Regenerate with `cargo test -p orbital-siege --test generate_project -- --ignored`.
  `ORBITAL_PROJECT=dir` runs another project directory.
