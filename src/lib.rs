pub mod engine;
pub mod i18n;
pub mod kinematics;
pub mod link;
pub mod protocol;
pub mod settings;
pub mod sync;
pub mod ui;
pub mod viewer3d;
pub mod wifi;

use std::path::PathBuf;

/// Directory holding `STL/`, `nexarm.urdf` and images.
///
/// Looks at `$NEXARM_ASSETS`, then `assets/` next to the executable (and the macOS
/// bundle `Resources/assets`), then the source tree for `cargo run`.
pub fn assets_dir() -> PathBuf {
    let mut candidates = Vec::new();
    if let Some(p) = std::env::var_os("NEXARM_ASSETS") {
        candidates.push(PathBuf::from(p));
    }
    if let Some(exe_dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(PathBuf::from)) {
        candidates.push(exe_dir.join("assets"));
        candidates.push(exe_dir.join("../Resources/assets"));
    }
    candidates.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets"));
    candidates
        .iter()
        .find(|p| p.join("nexarm.urdf").is_file())
        .cloned()
        .unwrap_or_else(|| candidates.last().cloned().unwrap())
}
