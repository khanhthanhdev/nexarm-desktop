//! JSON types exchanged with the engine. They mirror `docs/service/openapi.json`.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Health {
    pub status: String,
    pub version: String,
    pub api_version: String,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub pid: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct CameraProfile {
    #[serde(rename = "type", default = "default_camera_type")]
    pub kind: String,
    pub source: String,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
}

fn default_camera_type() -> String {
    "opencv".into()
}

impl Default for CameraProfile {
    fn default() -> Self {
        Self { kind: default_camera_type(), source: "0".into(), width: 640, height: 480, fps: 30 }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct RobotProfile {
    pub name: String,
    pub follower_port: String,
    #[serde(default)]
    pub leader_port: Option<String>,
    #[serde(default = "default_baud")]
    pub baudrate: u32,
    #[serde(default)]
    pub cameras: BTreeMap<String, CameraProfile>,
}

fn default_baud() -> u32 {
    1_000_000
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct PortInfo {
    pub device: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct SessionInfo {
    pub id: String,
    pub kind: String,
    pub profile: String,
    pub state: String,
    #[serde(default)]
    pub started_at: f64,
    #[serde(default)]
    pub params: Value,
    #[serde(default)]
    pub stop_reason: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub status: Value,
}

impl SessionInfo {
    /// A numeric field of the worker's live status (`status.<key>`).
    pub fn status_f64(&self, key: &str) -> Option<f64> {
        self.status.get(key).and_then(Value::as_f64)
    }

    pub fn status_str(&self, key: &str) -> Option<&str> {
        self.status.get(key).and_then(Value::as_str)
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct JointDelta {
    pub joint: String,
    pub leader_raw: i64,
    pub leader_mapped: i64,
    pub follower: i64,
    pub delta: i64,
    pub ok: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct CalibrationReport {
    pub tolerance_ticks: i64,
    pub joints: Vec<JointDelta>,
    pub ok: bool,
    #[serde(default)]
    pub reset_files: Vec<String>,
    #[serde(default)]
    pub hint: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct DatasetSummary {
    pub repo_id: String,
    pub fps: u32,
    #[serde(default)]
    pub robot_type: Option<String>,
    pub total_episodes: u32,
    pub total_frames: u64,
    #[serde(default)]
    pub cameras: Vec<String>,
    #[serde(default)]
    pub excluded_episodes: u32,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct EpisodeSummary {
    pub episode_index: u32,
    pub length: u32,
    #[serde(default)]
    pub tasks: Vec<String>,
    #[serde(default)]
    pub excluded: bool,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct JointsData {
    pub names: Vec<String>,
    pub timestamps: Vec<f64>,
    pub action: Vec<Vec<f64>>,
    pub state: Vec<Vec<f64>>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Operation {
    pub id: String,
    pub label: String,
    pub state: String,
    #[serde(default)]
    pub result: Option<Value>,
    #[serde(default)]
    pub error: Option<String>,
}

impl Operation {
    pub fn is_running(&self) -> bool {
        self.state == "running"
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct TrainRequest {
    pub dataset_repo_id: String,
    pub policy_type: String,
    pub steps: u32,
    pub batch_size: u32,
    pub log_freq: u32,
    #[serde(default)]
    pub save_freq: Option<u32>,
    #[serde(default)]
    pub job_name: Option<String>,
    #[serde(default = "yes")]
    pub use_excluded: bool,
    #[serde(default)]
    pub extra_args: BTreeMap<String, String>,
    #[serde(default = "local")]
    pub runner: String,
    #[serde(default)]
    pub server: Option<String>,
}

fn yes() -> bool {
    true
}

fn local() -> String {
    "local".into()
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Job {
    pub id: String,
    pub runner: String,
    pub state: String,
    pub request: TrainRequest,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub step: u32,
    pub total_steps: u32,
    #[serde(default)]
    pub loss: Option<f64>,
    #[serde(default)]
    pub lr: Option<f64>,
}

impl Job {
    pub fn is_active(&self) -> bool {
        matches!(self.state.as_str(), "queued" | "running")
    }

    pub fn progress(&self) -> f32 {
        if self.total_steps == 0 { 0.0 } else { (self.step as f32 / self.total_steps as f32).clamp(0.0, 1.0) }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct MetricPoint {
    pub step: u32,
    pub values: BTreeMap<String, f64>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Checkpoint {
    #[serde(default)]
    pub step: Option<u32>,
    pub path: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct ModelInfo {
    pub id: String,
    pub source: String,
    #[serde(default)]
    pub policy_type: Option<String>,
    #[serde(default)]
    pub step: Option<u32>,
    #[serde(default)]
    pub dataset_repo_id: Option<String>,
    #[serde(default)]
    pub job_id: Option<String>,
}

impl ModelInfo {
    /// One-line label for pickers.
    pub fn label(&self) -> String {
        let mut parts = vec![self.policy_type.clone().unwrap_or_else(|| "policy".into())];
        if let Some(s) = self.step {
            parts.push(format!("step {s}"));
        }
        if let Some(d) = &self.dataset_repo_id {
            parts.push(d.clone());
        }
        format!("{} [{}]", parts.join(" · "), self.source)
    }
}
