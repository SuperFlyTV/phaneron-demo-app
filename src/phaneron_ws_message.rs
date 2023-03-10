// TODO: This should not be a copy-paste job

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct RegisterRequest {
    #[serde(rename = "userId")]
    pub user_id: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RegisterResponse {
    pub url: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(tag = "event")]
pub enum ClientEvent {
    Topics(TopicsRequest),
    NodeState(NodeStateRequest),
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TopicsRequest {
    pub topics: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct NodeStateRequest {
    pub node_id: String,
    pub state: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FlipperState {
    pub flipped: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum ServerEvent {
    PhaneronState(PhaneronState),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhaneronState {
    pub nodes: HashMap<String, PhaneronNode>,
    pub video_outputs: HashMap<String, Vec<String>>,
    pub video_inputs: HashMap<String, Vec<String>>,
    pub connections: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhaneronNode {
    pub name: Option<String>,
    pub state: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TraditionalMixerEmulatorState {
    pub active_input: Option<String>,
    pub next_input: Option<String>,
    pub transition: Option<TraditionalMixerEmulatorTransition>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(tag = "transition")]
pub enum TraditionalMixerEmulatorTransition {
    Mix { position: f32 },
}
