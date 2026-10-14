//! URDF forward kinematics for the NexArm, ported from `urdf_parser.py` and
//! `arm_3d_widget._compute_link_transforms`.
//!
//! Lengths are in millimetres (URDF metres x [`SCALE`]); STL meshes stay in metres.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use glam::{DMat3, DMat4, DVec3, Mat4};

/// URDF metres -> millimetres.
pub(crate) const SCALE: f64 = 1000.0;
const SKIP_LINKS: &[&str] = &["camera_link"];
const BASE_LINK: &str = "base_link";

const SERVO_CENTER: [f64; 6] = [2048.0, 2048.0, 1198.0, 1238.0, 2048.0, 2048.0];
const SERVO_RATIO: [f64; 6] = [0.05859375, 0.087890625, 0.087890625, 0.087890625, 0.05859375, 0.05859375];
const SERVO_DIR: [f64; 6] = [1.0, 1.0, -1.0, -1.0, -1.0, -1.0];
const URDF_DIR: [f64; 6] = [1.0, -1.0, 1.0, -1.0, 1.0, 1.0];

const LINK_COLORS: [[f32; 3]; 10] = [
    [0.55, 0.55, 0.6],
    [0.3, 0.6, 0.95],
    [0.3, 0.8, 0.45],
    [0.95, 0.6, 0.25],
    [0.75, 0.35, 0.8],
    [0.8, 0.6, 0.1],
    [0.55, 0.1, 0.55],
    [0.02, 0.02, 0.02],
    [0.02, 0.02, 0.02],
    [0.02, 0.02, 0.02],
];

#[derive(Debug, Clone)]
pub struct LinkInfo {
    pub name: String,
    pub stl_path: Option<PathBuf>,
    pub color: [u8; 3],
}

#[derive(Debug, Clone)]
struct Joint {
    name: String,
    kind: String,
    xyz: DVec3,
    rpy: DVec3,
    parent: String,
    child: String,
    axis: DVec3,
}

#[derive(Debug, Clone)]
pub struct ArmModel {
    /// Rendered links in draw order (`camera_link` excluded).
    pub links: Vec<LinkInfo>,
    /// Revolute chain from `base_link`, driven by servos 0..n.
    chain: Vec<Joint>,
    /// Remaining joints whose child is off the chain (gripper, jaws).
    fixed: Vec<Joint>,
}

fn parse_vec3(s: Option<&str>, default: DVec3) -> Result<DVec3> {
    let Some(s) = s else { return Ok(default) };
    let v: Vec<f64> = s
        .split_whitespace()
        .map(str::parse)
        .collect::<std::result::Result<_, _>>()
        .with_context(|| format!("bad vector {s:?}"))?;
    anyhow::ensure!(v.len() == 3, "expected 3 components in {s:?}");
    Ok(DVec3::new(v[0], v[1], v[2]))
}

fn rpy_to_matrix(rpy: DVec3) -> DMat3 {
    DMat3::from_rotation_z(rpy.z) * DMat3::from_rotation_y(rpy.y) * DMat3::from_rotation_x(rpy.x)
}

fn axis_angle_matrix(axis: DVec3, angle: f64) -> DMat3 {
    let n = axis.length();
    if n < 1e-9 {
        return DMat3::IDENTITY;
    }
    DMat3::from_axis_angle(axis / n, angle)
}

fn make_transform(xyz: DVec3, rpy: DVec3) -> DMat4 {
    DMat4::from_translation(xyz) * DMat4::from_mat3(rpy_to_matrix(rpy))
}

impl ArmModel {
    /// Parses `assets_dir/nexarm.urdf`; mesh basenames resolve to `assets_dir/STL/<file>`.
    pub fn load(assets_dir: &Path) -> Result<ArmModel> {
        let urdf_path = assets_dir.join("nexarm.urdf");
        let text = std::fs::read_to_string(&urdf_path)
            .with_context(|| format!("reading {}", urdf_path.display()))?;
        Self::parse(&text, &assets_dir.join("STL"))
    }

    fn parse(text: &str, stl_dir: &Path) -> Result<ArmModel> {
        let doc = roxmltree::Document::parse(text).context("parsing URDF")?;
        let root = doc.root_element();

        let mut meshes: HashMap<String, Option<PathBuf>> = HashMap::new();
        for link in root.children().filter(|n| n.has_tag_name("link")) {
            let name = link.attribute("name").unwrap_or_default().to_string();
            let stl = link
                .children()
                .find(|n| n.has_tag_name("visual"))
                .and_then(|v| v.descendants().find(|n| n.has_tag_name("mesh")))
                .and_then(|m| m.attribute("filename"))
                .and_then(|f| f.rsplit(['/', '\\']).next())
                .map(|base| stl_dir.join(base))
                .filter(|p| p.is_file());
            meshes.insert(name, stl);
        }

        let mut joints = Vec::new();
        for j in root.children().filter(|n| n.has_tag_name("joint")) {
            let attr_of = |tag: &str, attr: &str| {
                j.children().find(|n| n.has_tag_name(tag)).and_then(|n| n.attribute(attr))
            };
            let name = j.attribute("name").unwrap_or_default().to_string();
            joints.push(Joint {
                kind: j.attribute("type").unwrap_or("fixed").to_string(),
                xyz: parse_vec3(attr_of("origin", "xyz"), DVec3::ZERO)
                    .with_context(|| format!("joint {name} origin"))?,
                rpy: parse_vec3(attr_of("origin", "rpy"), DVec3::ZERO)
                    .with_context(|| format!("joint {name} rpy"))?,
                parent: attr_of("parent", "link").unwrap_or_default().to_string(),
                child: attr_of("child", "link").unwrap_or_default().to_string(),
                axis: parse_vec3(attr_of("axis", "xyz"), DVec3::Z)
                    .with_context(|| format!("joint {name} axis"))?,
                name,
            });
        }

        // Movable chain from base_link; last joint per parent wins, as in the original.
        let mut child_map: HashMap<&str, &Joint> = HashMap::new();
        for j in joints.iter().filter(|j| j.kind == "revolute" || j.kind == "continuous") {
            child_map.insert(j.parent.as_str(), j);
        }
        let mut chain = Vec::new();
        let mut seen = HashSet::from([BASE_LINK]);
        let mut current = BASE_LINK;
        while let Some(j) = child_map.get(current) {
            if !seen.insert(j.child.as_str()) {
                break;
            }
            chain.push((*j).clone());
            current = j.child.as_str();
        }

        let chain_children: HashSet<&str> = chain.iter().map(|j| j.child.as_str()).collect();
        let fixed: Vec<Joint> = joints
            .iter()
            .filter(|j| {
                j.child != BASE_LINK
                    && !chain_children.contains(j.child.as_str())
                    && !SKIP_LINKS.contains(&j.child.as_str())
            })
            .cloned()
            .collect();

        let mut order: Vec<String> = vec![BASE_LINK.to_string()];
        order.extend(chain.iter().map(|j| j.child.clone()));
        for j in &fixed {
            if !order.contains(&j.child) {
                order.push(j.child.clone());
            }
        }

        let links = order
            .iter()
            .enumerate()
            .filter(|(_, name)| !SKIP_LINKS.contains(&name.as_str()))
            .map(|(ci, name)| LinkInfo {
                name: name.clone(),
                stl_path: meshes.get(name).cloned().flatten(),
                color: LINK_COLORS[ci % LINK_COLORS.len()].map(|c| (c * 255.0).round() as u8),
            })
            .collect();

        Ok(ArmModel { links, chain, fixed })
    }

    /// Joint angles (radians) for the given raw servo positions.
    pub(crate) fn servo_angles(servos: [i16; 6]) -> [f64; 6] {
        std::array::from_fn(|i| {
            ((servos[i] as f64 - SERVO_CENTER[i]) * SERVO_RATIO[i] * SERVO_DIR[i] * URDF_DIR[i]).to_radians()
        })
    }

    /// World transform (millimetres) of each entry in [`Self::links`], in the same order.
    pub fn link_transforms(&self, servos: [i16; 6]) -> Vec<(String, Mat4)> {
        let angles = Self::servo_angles(servos);
        let mut xf: HashMap<&str, DMat4> = HashMap::new();
        xf.insert(BASE_LINK, DMat4::IDENTITY);

        let mut t = DMat4::IDENTITY;
        for (i, j) in self.chain.iter().enumerate() {
            let angle = angles.get(i).copied().unwrap_or(0.0);
            t = t * make_transform(j.xyz * SCALE, j.rpy) * DMat4::from_mat3(axis_angle_matrix(j.axis, angle));
            xf.insert(j.child.as_str(), t);
        }

        let claw = angles[5];
        let jaw_travel = -claw * 0.02;
        let mut remaining: Vec<&Joint> = self.fixed.iter().collect();
        for _ in 0..5 {
            if remaining.is_empty() {
                break;
            }
            remaining.retain(|j| {
                let Some(&parent_t) = xf.get(j.parent.as_str()) else { return true };
                let t_origin = make_transform(j.xyz * SCALE, j.rpy);
                let t = if j.kind == "revolute" && j.name.contains("gripper_base") {
                    parent_t * t_origin * DMat4::from_mat3(axis_angle_matrix(j.axis, claw))
                } else if j.kind == "prismatic" && j.name.contains("jaw") {
                    let mut disp = jaw_travel * SCALE;
                    if j.name.contains("left") {
                        disp = -disp;
                    }
                    parent_t * t_origin * DMat4::from_translation(j.axis * disp)
                } else {
                    parent_t * t_origin
                };
                xf.insert(j.child.as_str(), t);
                false
            });
        }

        self.links
            .iter()
            .map(|l| (l.name.clone(), xf.get(l.name.as_str()).copied().unwrap_or(DMat4::IDENTITY).as_mat4()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;

    const CENTER: [i16; 6] = [2048, 2048, 1198, 1238, 2048, 2048];

    fn model() -> ArmModel {
        ArmModel::load(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets")).unwrap()
    }

    fn get(xf: &[(String, Mat4)], name: &str) -> Mat4 {
        xf.iter().find(|(n, _)| n == name).unwrap().1
    }

    fn close(a: Vec3, b: Vec3) -> bool {
        (a - b).length() < 1e-3
    }

    #[test]
    fn kinematics_link_order_skips_camera() {
        let m = model();
        let names: Vec<&str> = m.links.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "base_link", "link1", "link2", "link3", "link4", "link5", "gripper_link",
                "gripper_base_link", "right_jaw_link", "left_jaw_link"
            ]
        );
        assert!(m.links.iter().all(|l| l.stl_path.as_ref().is_some_and(|p| p.is_file())));
        assert_eq!(m.links[1].color, [77, 153, 242]);
        assert_eq!(m.chain.len(), 5);
    }

    #[test]
    fn kinematics_center_pose_is_pure_translation() {
        let xf = model().link_transforms(CENTER);
        assert_eq!(xf.len(), 10);
        let (_, rot, tr) = get(&xf, "link2").to_scale_rotation_translation();
        assert!(close(tr, Vec3::new(0.42383 - 0.41844, 37.28, 71.003 + 39.446)), "{tr}");
        assert!(rot.angle_between(glam::Quat::IDENTITY) < 1e-5);
        assert_eq!(get(&xf, "base_link"), Mat4::IDENTITY);
    }

    #[test]
    fn kinematics_joint1_rotates_downstream() {
        let mut s = CENTER;
        s[0] = 2048 + 1536; // 1536 * 0.05859375 = 90 degrees
        let xf = model().link_transforms(s);
        let l1 = get(&xf, "link1");
        let o1 = Vec3::new(0.42383, 0.0, 71.003);
        assert!(close(l1.w_axis.truncate(), o1));
        // 90 degrees about -Z maps +X to -Y.
        assert!(close(l1.transform_vector3(Vec3::X), -Vec3::Y));
        let o2 = Vec3::new(-0.41844, 37.28, 39.446);
        let rotated = Vec3::new(o2.y, -o2.x, o2.z);
        assert!(close(get(&xf, "link2").w_axis.truncate(), o1 + rotated));
    }

    #[test]
    fn kinematics_claw_moves_jaws_apart() {
        let m = model();
        let base = m.link_transforms(CENTER);
        let mut s = CENTER;
        s[5] = 2048 + 500;
        let open = m.link_transforms(s);
        let pos = |xf: &[(String, Mat4)], n: &str| get(xf, n).w_axis.truncate();
        let dr = pos(&open, "right_jaw_link") - pos(&base, "right_jaw_link");
        let dl = pos(&open, "left_jaw_link") - pos(&base, "left_jaw_link");
        assert!(dr.length() > 0.1);
        assert!(close(dr, -dl), "{dr} vs {dl}");
        assert_eq!(get(&open, "gripper_link"), get(&base, "gripper_link"));
    }
}
