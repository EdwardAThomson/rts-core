//! The genre-neutral core shared by every engine built on it (the Classic engine and the 3D engine): integer maths,
//! the seeded random generator, the canonical state hash, the command log that replays are made of, a JSON reader for
//! data files, and finding the setting packs players pick from. Nothing here knows about tiles, units or any game's
//! rules, so a 2D tile engine and a 3D engine can both build on it.

#![deny(clippy::float_arithmetic, clippy::disallowed_types)]

pub mod campaign;
pub mod hash;
pub mod imath;
pub mod json;
pub mod mission;
pub mod packs;
pub mod replay;
pub mod rng;
