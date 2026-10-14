//! Follower-arm calibration records: joint range-of-motion tracking, sanity checks and
//! named profiles.
//!
//! The arm's zero stays in the servos (centre calibration and position offsets); a profile
//! only records what was measured so it can be reviewed and the offsets restored.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const JOINTS: usize = 6;
/// Raw servo position of a centred joint.
pub const CENTER: u16 = 2048;
/// A swept joint moves at least this many servo ticks (about 4°).
pub const MIN_SPAN: u16 = 50;

/// Min/max raw position seen per joint while the user moves each joint by hand.
#[derive(Clone, Debug, PartialEq)]
pub struct RangeTracker {
    min: [u16; JOINTS],
    max: [u16; JOINTS],
    samples: u32,
}

impl Default for RangeTracker {
    fn default() -> Self {
        Self { min: [u16::MAX; JOINTS], max: [0; JOINTS], samples: 0 }
    }
}

impl RangeTracker {
    pub fn record(&mut self, positions: &[u16; JOINTS]) {
        for (i, p) in positions.iter().enumerate() {
            self.min[i] = self.min[i].min(*p);
            self.max[i] = self.max[i].max(*p);
        }
        self.samples += 1;
    }

    pub fn samples(&self) -> u32 {
        self.samples
    }

    /// `(min, max)` per joint; `None` until a sample arrives.
    pub fn ranges(&self) -> Option<[(u16, u16); JOINTS]> {
        (self.samples > 0).then(|| std::array::from_fn(|i| (self.min[i], self.max[i])))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JointCheck {
    Ok,
    /// The joint barely moved, so the range says nothing about it.
    NotSwept,
    /// The centre position lies outside the swept range: the zero is off.
    CentreOutside,
}

pub fn check_joint((min, max): (u16, u16)) -> JointCheck {
    if max.saturating_sub(min) < MIN_SPAN {
        JointCheck::NotSwept
    } else if !(min..=max).contains(&CENTER) {
        JointCheck::CentreOutside
    } else {
        JointCheck::Ok
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    pub name: String,
    /// Seconds since the Unix epoch.
    pub saved_at: u64,
    /// Servo position offsets as read back from the arm; `None` when not read.
    pub offsets: [Option<i16>; JOINTS],
    pub ranges: [(u16, u16); JOINTS],
}

pub fn profiles_dir() -> PathBuf {
    crate::settings::config_dir().join("calibration")
}

/// File-name-safe form of a profile name; `None` when nothing usable remains.
pub fn slug(name: &str) -> Option<String> {
    let s: String = name
        .trim()
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    (!s.trim_matches('_').is_empty()).then_some(s)
}

fn path_of(dir: &Path, name: &str) -> anyhow::Result<PathBuf> {
    let slug = slug(name).ok_or_else(|| anyhow::anyhow!("profile name is empty"))?;
    Ok(dir.join(format!("{slug}.json")))
}

pub fn save(dir: &Path, profile: &Profile) -> anyhow::Result<()> {
    std::fs::create_dir_all(dir)?;
    std::fs::write(path_of(dir, &profile.name)?, serde_json::to_vec_pretty(profile)?)?;
    Ok(())
}

pub fn load(dir: &Path, name: &str) -> anyhow::Result<Profile> {
    Ok(serde_json::from_slice(&std::fs::read(path_of(dir, name)?)?)?)
}

pub fn delete(dir: &Path, name: &str) -> anyhow::Result<()> {
    Ok(std::fs::remove_file(path_of(dir, name)?)?)
}

/// Saved profile names, sorted. A missing directory is an empty list.
pub fn list(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            (p.extension()? == "json").then(|| p.file_stem()?.to_str().map(str::to_owned))?
        })
        .collect();
    names.sort();
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracker_keeps_min_and_max_per_joint() {
        let mut t = RangeTracker::default();
        assert_eq!(t.ranges(), None);
        t.record(&[2048, 1000, 3000, 2048, 10, 4000]);
        t.record(&[2100, 1500, 2900, 2000, 4000, 4000]);
        let r = t.ranges().unwrap();
        assert_eq!(r[0], (2048, 2100));
        assert_eq!(r[1], (1000, 1500));
        assert_eq!(r[4], (10, 4000));
        assert_eq!(t.samples(), 2);
    }

    #[test]
    fn checks_flag_unswept_and_off_centre_joints() {
        assert_eq!(check_joint((2040, 2060)), JointCheck::NotSwept);
        assert_eq!(check_joint((500, 1800)), JointCheck::CentreOutside);
        assert_eq!(check_joint((500, 3500)), JointCheck::Ok);
    }

    #[test]
    fn slug_rejects_empty_and_replaces_separators() {
        assert_eq!(slug("  my arm/1 "), Some("my_arm_1".into()));
        assert_eq!(slug("../.."), None);
        assert_eq!(slug("   "), None);
    }

    #[test]
    fn profiles_round_trip_and_list() {
        let dir = std::env::temp_dir().join(format!("nexarm-calib-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(list(&dir).is_empty());
        let p = Profile {
            name: "bench arm".into(),
            saved_at: 7,
            offsets: [Some(-12), None, Some(5), Some(0), Some(2047), Some(-2047)],
            ranges: [(1, 2); JOINTS],
        };
        save(&dir, &p).unwrap();
        assert_eq!(list(&dir), ["bench_arm"]);
        assert_eq!(load(&dir, "bench arm").unwrap(), p);
        delete(&dir, "bench arm").unwrap();
        assert!(list(&dir).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
