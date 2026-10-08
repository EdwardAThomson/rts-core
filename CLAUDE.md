# rts-core

The genre-neutral core shared by the Classic engine (`rts-engine`) and the 3D engine (`rts-3d-engine`). Read
`README.md` first.

- Two crates: `rts-core` (`crates/core`, the deterministic core) and `rts-platform` (`crates/platform`, the
  renderers' GPU, sound, files and browser layer; floats and the clock are fine there, game state is not).
- Determinism rules are in README.md and enforced by clippy; keep `crates/core/clippy.toml` and the crate's
  `#![deny(...)]` in place.
- Anything that changes a state hash is a breaking change for both engines: bump the crate version and say so in the commit.
- Nothing here may name a tile, unit, faction or setting. If it only makes sense for one engine, it belongs in that
  engine's repository.
- Clean room: never copy code from other RTS projects or remakes; ideas only, credited.
- Reports end with Verified and Not verified lists.
