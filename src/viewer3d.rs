//! Pop-out 3D view: spawns the Rerun viewer and streams the arm pose to it.

use std::sync::atomic::{AtomicI64, Ordering};

use anyhow::{anyhow, Result};

use crate::kinematics::{ArmModel, SCALE};

pub const VIEWER_INSTALL_HINT: &str = "The 3D view needs the Rerun Viewer 0.38.1 on PATH. Install it with one of:\n\
    * cargo binstall --force rerun-cli@0.38.1\n\
    * pip install rerun-sdk==0.38.1\n\
    * download from https://github.com/rerun-io/rerun/releases/0.38.1/";

const APP_ID: &str = "nexarm";
const ARM_PATH: &str = "world/arm";

pub struct Viewer3d {
    rec: rerun::RecordingStream,
    frame: AtomicI64,
}

impl Viewer3d {
    /// Spawns the viewer process and logs the static scene (axes + link meshes).
    pub fn open(model: &ArmModel) -> Result<Viewer3d> {
        let rec = rerun::RecordingStreamBuilder::new(APP_ID)
            .spawn()
            .map_err(|e| anyhow!("{e}\n\n{VIEWER_INSTALL_HINT}"))?;
        rec.log_static("world", &rerun::ViewCoordinates::RIGHT_HAND_Z_UP())?;
        for link in &model.links {
            let Some(path) = &link.stl_path else { continue };
            let bytes = match std::fs::read(path) {
                Ok(b) => b,
                Err(e) => {
                    eprintln!("viewer3d: skipping {}: {e}", path.display());
                    continue;
                }
            };
            let [r, g, b] = link.color;
            let mesh = rerun::Asset3D::from_file_contents(bytes, Some("model/stl"))
                .with_albedo_factor(rerun::Rgba32::from_rgb(r, g, b));
            rec.log_static(format!("{ARM_PATH}/{}", link.name), &mesh)?;
        }
        Ok(Viewer3d { rec, frame: AtomicI64::new(0) })
    }

    /// Logs one pose as per-link transforms on the `frame` timeline.
    pub fn update(&self, model: &ArmModel, servos: [i16; 6]) {
        let frame = self.frame.fetch_add(1, Ordering::Relaxed);
        self.rec.set_time_sequence("frame", frame);
        for (name, m) in model.link_transforms(servos) {
            // Meshes are in metres; FK output is in millimetres.
            let t = m.w_axis.truncate() / SCALE as f32;
            let rot = glam::Mat3::from_mat4(m).to_cols_array_2d();
            let xf = rerun::Transform3D::from_translation_mat3x3([t.x, t.y, t.z], rerun::Mat3x3::from(rot));
            if let Err(e) = self.rec.log(format!("{ARM_PATH}/{name}"), &xf) {
                eprintln!("viewer3d: log {name}: {e}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::VIEWER_INSTALL_HINT;

    #[test]
    fn viewer3d_install_hint_names_all_routes() {
        for s in [
            "cargo binstall --force rerun-cli@0.38.1",
            "pip install rerun-sdk==0.38.1",
            "https://github.com/rerun-io/rerun/releases/0.38.1/",
        ] {
            assert!(VIEWER_INSTALL_HINT.contains(s), "{s}");
        }
    }
}
