//! The genre-neutral core shared by every engine built on it (the Classic engine and the 3D engine): integer maths, the seeded random generator, the
//! canonical state hash and the command log that replays are made of. Nothing here knows about tiles, units or
//! any game's rules, so a 2D tile engine and a 3D engine can both build on it.

#![deny(clippy::float_arithmetic, clippy::disallowed_types)]

pub mod hash;
pub mod imath;
pub mod replay;
pub mod rng;
