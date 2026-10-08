//! The platform layer both engines' renderers stand on: the GPU, textures, a sprite batcher, a pixel font, sound (a
//! mixer, the sound device and WAV files), a clock, file access and (in the browser) the page. Nothing here knows about
//! tiles, units, games or setting packs, and nothing here is part of a simulation: it may use floating point and the
//! clock, and it never touches game state.
//!
//! Moved from the Classic engine's renderer (`rts-engine`, `crates/classic-render/src/platform`).

pub mod audio;
pub mod batch;
pub mod clock;
pub mod files;
pub mod gpu;
pub mod text;
pub mod wav;
#[cfg(target_arch = "wasm32")]
pub mod web;

#[cfg(feature = "device")]
pub use audio::Speaker;
pub use audio::{Bus, ClipId, Mixer, Played, Sound};
pub use batch::{Rect, SpriteBatch, TexId};
pub use clock::Instant;
pub use files::Files;
pub use gpu::Gpu;
pub use text::Font;
pub use wav::Clip;
