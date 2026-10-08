# RTS core

The genre-neutral core shared by the RTS engines: the Classic engine (2D tile grid, `rts-engine`) and the 3D
engine (`rts-3d-engine`). Nothing here knows about tiles, units or any game's rules.

| Crate | Path | Has |
|---|---|---|
| `rts-core` | `crates/core` | `imath` (integer square root), `rng` (seeded xorshift held in the game state), `hash` (streaming canonical-JSON FNV-1a state hash with a `Canon` trait), `replay` (command queue and log) |
| `rts-platform` | `crates/platform` | The renderers' platform layer, never part of a simulation: `gpu` (opening wgpu, with or without a window), `batch` (textures and a sprite batcher), `text` (a pixel font), `audio` (a mixer, buses and the sound device), `wav`, `clock`, `files` (a folder or files in memory) and, in the browser, `web` (the page, its address and fetching files) |

Planned here as the engines need them: lockstep networking, the AI framework and setting-pack loading.

## Using it

```toml
[dependencies]
rts-core = { git = "https://github.com/EdwardAThomson/rts-core", rev = "<commit>" }
rts-platform = { git = "https://github.com/EdwardAThomson/rts-core", rev = "<commit>" }
```

Pin a commit with `rev`, so a change here never alters an engine's hashes until that engine moves on to it and
its golden tests pass.

## Rules

- **Determinism** (`rts-core`). Integer maths only, the one seeded generator held in the game state, no clock, no
  threads and no iteration over `HashMap`/`HashSet`. `crates/core/clippy.toml` bans the types and the crate denies float
  arithmetic; CI runs clippy with `-D warnings`.
- **A change that alters any hash is a breaking change.** Both engines' golden tests pin hashes made with this
  crate, so bump the crate version and say so.
- **No protected names, no setting-specific words.** Generic ids only.
- **No third-party dependencies** unless one clearly pays for itself. `rts-core` has none. `rts-platform` has wgpu,
  pollster and cpal (sound; on Linux it builds against ALSA's headers, `libasound2-dev`, or build it without the
  `device` feature), plus wasm-bindgen, wasm-bindgen-futures, js-sys and web-sys for the browser.

```bash
cargo test
cargo clippy --all-targets -- -D warnings
cargo clippy --target wasm32-unknown-unknown -- -D warnings
```

The code moved here from `rts-engine`: `rts-core` from `crates/engine-core` (as merged in rts-engine PR #5), and
`rts-platform` from `crates/classic-render/src/platform`.
