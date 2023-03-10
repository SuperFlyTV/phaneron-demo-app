use std::{collections::HashMap, sync::Arc, time::Duration};

use axum::{
    extract::{
        ws::{Message, WebSocket},
        State, WebSocketUpgrade,
    },
    http::Response,
    response::{Html, IntoResponse},
    routing::get,
    Router, Server,
};
use futures_util::{future::try_join_all, SinkExt, StreamExt};
use futures_util::{
    pin_mut,
    stream::{SplitSink, SplitStream},
};
use phaneron_ws_message::{PhaneronState, TraditionalMixerEmulatorTransition};
use reqwest::header::{ACCEPT, CONNECTION, HOST, USER_AGENT};
use serde::{Deserialize, Serialize};
use tokio::{
    select,
    sync::{
        broadcast::{self, error::RecvError},
        Mutex,
    },
};
use tokio_tungstenite::connect_async;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

use crate::phaneron_ws_message::{
    ClientEvent, NodeStateRequest, ServerEvent, TraditionalMixerEmulatorState,
};

mod phaneron_ws_message;

#[tokio::main]
async fn main() {
    if std::env::var("RUST_LIB_BACKTRACE").is_err() {
        std::env::set_var("RUST_LIB_BACKTRACE", "1")
    }
    color_eyre::install().unwrap();

    if std::env::var("RUST_LOG").is_err() {
        std::env::set_var("RUST_LOG", "phaneron=info")
    }
    tracing_subscriber::fmt::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let (state_tx, _) = broadcast::channel::<Snapshot>(1);
    let (commands_tx, _) = broadcast::channel::<AppCommand>(1);

    let app_state = AppState {
        state_tx: state_tx.clone(),
        commands_tx: commands_tx.clone(),
        last_snapshot: Default::default(),
    };

    let router = Router::new()
        .route("/", get(root_get))
        .route("/index.mjs", get(indexmjs_get))
        .route("/index.css", get(indexcss_get))
        .route("/state", get(state_ws))
        .with_state(app_state.clone());

    tokio::task::spawn(talk_to_phaneron(
        app_state.clone(),
        state_tx.clone(),
        commands_tx.subscribe(),
    ));

    let mut snapshot_rx = state_tx.subscribe();
    tokio::task::spawn(async move {
        loop {
            match snapshot_rx.recv().await {
                Ok(msg) => *app_state.last_snapshot.lock().await = Some(msg),
                Err(err) => match err {
                    RecvError::Lagged(msgs) => warn!("Lagged by {} messages", msgs),
                    RecvError::Closed => todo!("Handle closed snapshot_rx"),
                },
            }
        }
    });

    let server = Server::bind(&"0.0.0.0:7032".parse().unwrap()).serve(router.into_make_service());
    let addr = server.local_addr();
    info!("Listening on {addr}");

    server.await.unwrap();
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(tag = "command")]
enum AppCommand {
    SetActiveInput(SetActiveInputCommand),
    SetNextInput(SetNextInputCommand),
    DoCut,
    DoMix(DoMixCommand),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetActiveInputCommand {
    input: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetNextInputCommand {
    input: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DoMixCommand {
    position: f32,
}

#[derive(Clone, Debug)]
struct AppState {
    state_tx: broadcast::Sender<Snapshot>,
    commands_tx: broadcast::Sender<AppCommand>,
    last_snapshot: Arc<Mutex<Option<Snapshot>>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Snapshot {
    active_video_source_id: Option<String>,
    next_video_source_id: Option<String>,
    video_sources: Vec<VideoSource>,
    // audio_sources: Vec<AudioSource>,
    transition: Option<Transition>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VideoSource {
    id: String,
    name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AudioSource {
    id: String,
    level: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(tag = "transition")]
enum Transition {
    Mix { position: f32 },
}

impl From<TraditionalMixerEmulatorTransition> for Transition {
    fn from(transition: TraditionalMixerEmulatorTransition) -> Self {
        match transition {
            TraditionalMixerEmulatorTransition::Mix { position } => Transition::Mix { position },
        }
    }
}

impl From<Transition> for TraditionalMixerEmulatorTransition {
    fn from(transition: Transition) -> Self {
        match transition {
            Transition::Mix { position } => TraditionalMixerEmulatorTransition::Mix { position },
        }
    }
}

#[axum::debug_handler]
async fn root_get() -> impl IntoResponse {
    let markup = tokio::fs::read_to_string("src/index.html").await.unwrap();

    Html(markup)
}

#[axum::debug_handler]
async fn indexmjs_get() -> impl IntoResponse {
    let markup = tokio::fs::read_to_string("src/index.mjs").await.unwrap();

    Response::builder()
        .header("content-type", "application/javascript;charset=utf-8")
        .body(markup)
        .unwrap()
}

#[axum::debug_handler]
async fn indexcss_get() -> impl IntoResponse {
    let markup = tokio::fs::read_to_string("src/index.css").await.unwrap();

    Response::builder()
        .header("content-type", "text/css;charset=utf-8")
        .body(markup)
        .unwrap()
}

#[axum::debug_handler]
async fn state_ws(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    ws.on_upgrade(|ws: WebSocket| async {
        let (tx, rx) = ws.split();
        let state_handle = tokio::spawn(state_stream(state.clone(), tx));
        let commands_handle = tokio::spawn(handle_commands(state, rx));
        try_join_all([state_handle, commands_handle]).await.unwrap();
    })
}

async fn state_stream(app_state: AppState, mut tx: SplitSink<WebSocket, Message>) {
    let mut rx = app_state.state_tx.subscribe();
    if let Some(snapshot) = &*app_state.last_snapshot.lock().await {
        tx.send(axum::extract::ws::Message::Text(
            serde_json::to_string(&snapshot).unwrap(),
        ))
        .await
        .ok();
    }

    loop {
        match rx.recv().await {
            Ok(msg) => {
                tx.send(axum::extract::ws::Message::Text(
                    serde_json::to_string(&msg).unwrap(),
                ))
                .await
                .ok();
            }
            Err(err) => match err {
                RecvError::Closed => todo!("Handle state_stream rx closed"),
                RecvError::Lagged(msgs) => warn!("state_stream rx lagged by {} messages", msgs),
            },
        }
    }
}

async fn handle_commands(app_state: AppState, mut rx: SplitStream<WebSocket>) {
    let tx = app_state.commands_tx.clone();
    while let Some(msg) = rx.next().await {
        if let Ok(Message::Text(command)) = msg {
            let command: AppCommand = serde_json::from_str(&command).unwrap();
            info!("{:?}", command);
            tx.send(command).ok();
        }
    }
}

async fn talk_to_phaneron(
    state: AppState,
    snapshot_tx: tokio::sync::broadcast::Sender<Snapshot>,
    mut commands_rx: tokio::sync::broadcast::Receiver<AppCommand>,
) {
    let user_id = uuid::Uuid::new_v4().to_string();
    let request = &crate::phaneron_ws_message::RegisterRequest {
        user_id: user_id.clone(),
    };

    let resp: crate::phaneron_ws_message::RegisterResponse = reqwest::Client::new()
        .post("http://127.0.0.1:8080/register")
        .header(USER_AGENT, "PhaneronTestApp/1.0.0")
        .header(ACCEPT, "*/*")
        .header(HOST, "localhost:8080")
        .header(CONNECTION, "keep-alive")
        .json(&request)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let url = url::Url::parse(&resp.url).unwrap();
    let (ws_stream, _) = connect_async(url).await.expect("Failed to connect");
    info!("WebSocket connected");

    let (mut write, read) = ws_stream.split();

    let command_state_tx = snapshot_tx.clone();
    tokio::spawn(async move {
        loop {
            match commands_rx.recv().await {
                Ok(msg) => {
                    let state = { state.last_snapshot.clone() };
                    match msg {
                        AppCommand::SetActiveInput(cmd) => {
                            let current_state = state.lock().await.as_ref().unwrap().clone();
                            let new_state = TraditionalMixerEmulatorState {
                                active_input: Some(cmd.input),
                                next_input: current_state.next_video_source_id.clone(),
                                transition: None,
                            };
                            let event = ClientEvent::NodeState(NodeStateRequest {
                                node_id: "switcher".to_string(),
                                state: serde_json::to_string(&new_state).unwrap(),
                            });
                            write
                                .send(tokio_tungstenite::tungstenite::Message::Text(
                                    serde_json::to_string(&event).unwrap(),
                                ))
                                .await
                                .unwrap()
                        }
                        AppCommand::SetNextInput(cmd) => {
                            let mut transition: Option<TraditionalMixerEmulatorTransition> = None;
                            if let Some(switcher_transition) =
                                &state.lock().await.as_ref().unwrap().transition
                            {
                                transition = Some(switcher_transition.clone().into())
                            }
                            let current_state = state.lock().await.as_ref().unwrap().clone();
                            let new_state = TraditionalMixerEmulatorState {
                                active_input: current_state.active_video_source_id.clone(),
                                next_input: Some(cmd.input),
                                transition,
                            };
                            let event = ClientEvent::NodeState(NodeStateRequest {
                                node_id: "switcher".to_string(),
                                state: serde_json::to_string(&new_state).unwrap(),
                            });
                            write
                                .send(tokio_tungstenite::tungstenite::Message::Text(
                                    serde_json::to_string(&event).unwrap(),
                                ))
                                .await
                                .unwrap()
                        }
                        AppCommand::DoCut => {
                            let current_state = state.lock().await.as_ref().unwrap().clone();
                            let new_state = TraditionalMixerEmulatorState {
                                active_input: current_state.next_video_source_id.clone(),
                                next_input: current_state.active_video_source_id.clone(),
                                transition: None,
                            };
                            let event = ClientEvent::NodeState(NodeStateRequest {
                                node_id: "switcher".to_string(),
                                state: serde_json::to_string(&new_state).unwrap(),
                            });
                            write
                                .send(tokio_tungstenite::tungstenite::Message::Text(
                                    serde_json::to_string(&event).unwrap(),
                                ))
                                .await
                                .unwrap()
                        }
                        AppCommand::DoMix(command) => {
                            let current_state = state.lock().await.as_ref().unwrap().clone();
                            let new_state = TraditionalMixerEmulatorState {
                                active_input: current_state.active_video_source_id.clone(),
                                next_input: current_state.next_video_source_id.clone(),
                                transition: Some(TraditionalMixerEmulatorTransition::Mix {
                                    position: 1.0 - command.position,
                                }),
                            };
                            let event = ClientEvent::NodeState(NodeStateRequest {
                                node_id: "switcher".to_string(),
                                state: serde_json::to_string(&new_state).unwrap(),
                            });
                            write
                                .send(tokio_tungstenite::tungstenite::Message::Text(
                                    serde_json::to_string(&event).unwrap(),
                                ))
                                .await
                                .unwrap()
                        }
                    }
                }
                Err(err) => match err {
                    RecvError::Closed => todo!("Handle talk_to_phaneron rx closed"),
                    RecvError::Lagged(msgs) => warn!("commands_rx lagged by {} messages", msgs),
                },
            }
        }
    });

    let ws_to_app = {
        read.for_each(|message| async {
            if let tokio_tungstenite::tungstenite::Message::Text(text) = message.unwrap() {
                info!("{text}");
                let event: ServerEvent = serde_json::from_str(&text).unwrap();
                match event {
                    ServerEvent::PhaneronState(state) => handle_state(state, snapshot_tx.clone()),
                }
            }
        })
    };

    pin_mut!(ws_to_app);
    ws_to_app.await;
}

fn handle_state(phaneron_state: PhaneronState, tx: tokio::sync::broadcast::Sender<Snapshot>) {
    let switcher_node = phaneron_state.nodes.get("switcher").unwrap();
    if let Some(state) = &switcher_node.state {
        let switcher_state: TraditionalMixerEmulatorState = serde_json::from_str(state).unwrap();
        let mut node_output_to_node_id: HashMap<String, String> = HashMap::new();
        for (node_id, _) in phaneron_state.nodes.iter() {
            if let Some(outputs) = phaneron_state.video_outputs.get(node_id) {
                for output in outputs {
                    node_output_to_node_id.insert(output.to_string(), node_id.to_string());
                }
            }
        }

        let mut video_sources: Vec<VideoSource> = vec![];
        let switcher_inputs = phaneron_state.video_inputs.get("switcher").unwrap();
        for input in switcher_inputs.iter() {
            if let Some(output) = phaneron_state.connections.get(input) {
                let node_id = node_output_to_node_id.get(output).unwrap();
                let node = phaneron_state.nodes.get(node_id).unwrap().clone();
                video_sources.push(VideoSource {
                    id: input.clone(), // output.clone(),
                    name: node.name.unwrap_or_else(|| node_id.clone()),
                })
            }
        }

        let mut transition: Option<Transition> = None;
        if let Some(switcher_transition) = switcher_state.transition {
            transition = Some(switcher_transition.into());
        }

        let snapshot = Snapshot {
            active_video_source_id: switcher_state.active_input,
            next_video_source_id: switcher_state.next_input,
            video_sources,
            transition,
        };
        tx.send(snapshot).ok();
    }
}
