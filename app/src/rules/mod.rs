//! The game's rules as plain Rust, independent of rendering: the campaign save, tomb generation and
//! exploration, battles, and the Ark between missions. Presentation (`flow`) drives these and shows
//! their results; tests can play them headless.

pub mod ai;
pub mod ark;
pub mod battle;
pub mod explore;
pub mod rng;
pub mod save;
pub mod tomb;
