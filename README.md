# RTS core

The genre-neutral core shared by the RTS engines: the Classic engine (2D tile grid, `rts-engine`) and the 3D
engine (`rts-3d-engine`). Nothing here knows about tiles, units or any game's rules.

| Crate | Path | Has |
|---|---|---|
| `rts-core` | `crates/core` | `imath` (integer square root), `rng` (seeded xorshift held in the game state), `hash` (streaming canonical-JSON FNV-1a state hash with a `Canon` trait), `replay` (command queue and log), `json` (a whole-number JSON reader for rules data and setting packs), `mission` (declarative mission triggers: shared conditions and actions, objectives, win and lose, with engine-specific ones plugged in; its state is hashed with the game), `campaign` (a campaign as data: a text territory map, levels of territory choices each with its mission, and the player's progress as a small text file), `packs` (a pack's `pack.json`, the `packs/` folders each engine searches, and the browser's `packs.json` list) |
| `rts-platform` | `crates/platform` | The renderers' platform layer, never part of a simulation: `gpu` (opening wgpu, with or without a window), `batch` (textures and a sprite batcher), `text` (a pixel font), `skin` (a setting pack's UI skin from `theme/theme.json`: nine-slice frames, glyphs, emblems, cursors and baked fonts, read own pack first), `audio` (a mixer with buses, loops, music with crossfades, and the sound device), `wav` (PCM and IMA ADPCM), `music` (a pack's music by pool, menu, calm, battle, won and lost, turning to battle and back with the local player's fighting, for both engines), `clock`, `files` (a folder or files in memory) and, in the browser, `web` (the page, its address and fetching files) |

Planned here as the engines need them: lockstep networking, the AI framework and setting-pack loading.

## The repositories

The project is five repositories under [EdwardAThomson](https://github.com/EdwardAThomson). Only the two engines
are built and run; the others are pulled in by them or read alongside them.

| Repository | Visibility | What it holds | How the others use it |
|---|---|---|---|
| **rts-core** (this one) | public | The genre-neutral deterministic core (`rts-core`: integer maths, the seeded generator, the state hash, replays, the JSON reader, missions, campaigns) and the renderers' platform layer (`rts-platform`: GPU, sprites, text, UI skin, sound, music, files, browser page) | Both engines pin it by commit in `Cargo.toml`; `cargo` fetches it, so there is nothing to clone. A change to it reaches an engine only when that engine moves its pin and its golden tests pass |
| [rts-engine](https://github.com/EdwardAThomson/rts-engine) | private | The Classic engine: 2D tile RTS simulation, computer opponent, tools, the wgpu player (desktop and browser), the public `generic` setting pack, and the Blender art studio | Built on rts-core. Its art studio also exports the 3D engine's glTF models |
| [rts-3d-engine](https://github.com/EdwardAThomson/rts-3d-engine) | private | The 3D engine: heightmap terrain, continuous movement, real projectiles, its computer opponent, the wgpu player (desktop and browser) and its own `generic` pack | Built on rts-core, and shares the platform layer, HUD layout and save format with the Classic engine |
| [rts-setting-private](https://github.com/EdwardAThomson/rts-setting-private) | private | Private, data-only setting packs for the Classic engine (every setting-specific name lives only here), plus the scripts that make their art, maps, campaigns, sounds and voices | Cloned into the Classic engine's git-ignored `settings-private/` folder and picked with `--setting`. The engines run without it, on their `generic` packs |
| [ai-gamedev-playbooks](https://github.com/EdwardAThomson/ai-gamedev-playbooks) | private | The design docs (`plans/rts/` for both engines, `plans/rts-3d/` for the 3D one, `plans/rts/engine-parity.md`), guides and reviews | Read, never built: no code depends on it |

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
  arithmetic; clippy runs with `-D warnings` before every push (CI runs only when started by hand).
- **A change that alters any hash is a breaking change.** Both engines' golden tests pin hashes made with this
  crate, so bump the crate version and say so.
- **No protected names, no setting-specific words.** Generic ids only.
- **No third-party dependencies** unless one clearly pays for itself. `rts-core` has none. `rts-platform` has wgpu,
  pollster, png (the skin's pictures) and cpal (sound; on Linux it builds against ALSA's headers, `libasound2-dev`, or build it without the
  `device` feature), plus wasm-bindgen, wasm-bindgen-futures, js-sys and web-sys for the browser.

## Building and testing

You need [rustup](https://rustup.rs); the first `cargo` command here installs the toolchain pinned in
`rust-toolchain.toml` (Rust 1.97 with clippy, rustfmt and the WebAssembly target). `rts-core` needs nothing else.
`rts-platform`'s sound device builds against ALSA on Linux:

```bash
sudo apt-get install build-essential pkg-config libasound2-dev   # Debian and Ubuntu, for rts-platform's `device` feature
cargo test
cargo clippy --all-targets -- -D warnings
cargo clippy --target wasm32-unknown-unknown -- -D warnings
cargo build --no-default-features -p rts-platform                # without the sound device, so without ALSA
```

Once per clone, turn on the pre-push hook. It runs `.githooks/spot-check` on what each push sends: `cargo fmt --check`,
clippy, the tests for the changed files, and the goldens when simulation code changed. CI runs only when started by
hand, so this is the gate; the whole `cargo test` runs about once a day (`CHECKS=full .githooks/spot-check`), and
`git push --no-verify` skips the hook once:

```bash
git config core.hooksPath .githooks
```

There is nothing to run here: the engines' repositories have the games, players and tools.

The code moved here from `rts-engine`: `rts-core` from `crates/engine-core` (as merged in rts-engine PR #5), and
`rts-platform` from `crates/classic-render/src/platform`, and its `skin` from `crates/classic-render/src/skin.rs`.
