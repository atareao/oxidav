//! Build script: guarantee a usable `web/dist` before `rust-embed` runs.
//!
//! `rust-embed` (see `src/assets.rs`) embeds `web/dist` **at compile time** and
//! fails the build when the folder is missing.
//!
//! * In **debug** (development) a missing/placeholder `web/dist` is tolerated:
//!   this script writes a minimal placeholder and warns about the required
//!   build order, so a backend-only checkout still compiles and the developer
//!   can run the API/DAV surfaces without the SPA bundle.
//! * In **release** the placeholder must never be embedded: a shippable binary
//!   that answers `200` with the placeholder page instead of the real SPA is a
//!   bug. The build **aborts** when there is no real `web/dist/index.html`.
//!
//! # Build order
//!
//! The SPA must be built **before** the backend:
//!
//! ```text
//! cd web && npm ci && npm run build   # produces web/dist
//! cargo build --release               # embeds web/dist
//! ```

use std::env;
use std::fs;
use std::path::Path;

/// Minimal, dependency-free page written when `web/dist` was never built. Only a
/// development fallback: it is never embedded in a release build.
const PLACEHOLDER_INDEX: &str = "<!doctype html>\n\
<html lang=\"es\">\n\
  <head><meta charset=\"UTF-8\" /><title>OxiDav</title></head>\n\
  <body><p>OxiDav: el frontend no se ha construido. \
Ejecuta <code>cd web &amp;&amp; npm run build</code> y vuelve a compilar.</p></body>\n\
</html>\n";

fn main() {
    // Re-run whenever the embedded assets change.
    println!("cargo:rerun-if-changed=web/dist");

    let dist = Path::new("web/dist");
    let index = dist.join("index.html");

    if is_real_build(&index) {
        return;
    }

    // A release binary must embed the real SPA; a placeholder (or nothing at
    // all) is a hard error so it can never be shipped by accident.
    if env::var("PROFILE").as_deref() == Ok("release") {
        panic!(
            "web/dist/index.html is missing or is the development placeholder, \
             so the release build cannot embed the SPA.\n\
             Build the frontend FIRST, then rebuild:\n\
             \n    cd web && npm ci && npm run build\n    cargo build --release\n"
        );
    }

    println!(
        "cargo:warning=web/dist looks empty: building a placeholder. \
         Build the frontend FIRST (cd web && npm run build), then `cargo build`, \
         so the SPA is embedded into the binary."
    );

    // Best-effort: never panic a debug build here; if this fails, `rust-embed`
    // will report the missing folder with its own actionable error.
    let _ = fs::create_dir_all(dist.join("assets"));
    let _ = fs::write(index, PLACEHOLDER_INDEX);
}

/// Whether `index.html` exists and is a real build (not our own placeholder).
fn is_real_build(index: &Path) -> bool {
    fs::read_to_string(index)
        .map(|content| content != PLACEHOLDER_INDEX)
        .unwrap_or(false)
}
