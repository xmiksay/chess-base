//! Ensures `frontend/dist` exists at compile time so `rust-embed` always
//! compiles, even before the Vue app has been built. The real assets are
//! produced by `make frontend` (vite build) and embedded in release builds.

use std::path::Path;

fn main() {
    let dist = Path::new(env!("CARGO_MANIFEST_DIR")).join("frontend/dist");
    if !dist.exists() {
        let _ = std::fs::create_dir_all(&dist);
    }
    // A placeholder so the embedded asset set is never empty.
    let index = dist.join("index.html");
    if !index.exists() {
        let _ = std::fs::write(
            &index,
            "<!doctype html><meta charset=utf-8><title>chess-base</title>\
             <body>Frontend not built. Run <code>make frontend</code>.</body>",
        );
    }
    println!("cargo:rerun-if-changed=frontend/dist");
}
