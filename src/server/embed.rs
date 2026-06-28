//! The built Vue SPA, embedded into the binary at compile time.
//!
//! `build.rs` guarantees `frontend/dist` exists so this always compiles; the
//! real assets are produced by `make frontend`.

use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "frontend/dist"]
pub struct Assets;
