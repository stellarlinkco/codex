use codex_core::runtime_owner::RuntimeLiveRegistration;
use codex_core::runtime_owner::RuntimeOwner;
use codex_core::runtime_owner::ThreadOwnerLease;
use codex_core::runtime_owner::runtime_owner_lease_path;
use codex_core::runtime_owner::unix_timestamp_now;
use codex_protocol::ThreadId;
use codex_protocol::models::PermissionProfile;
use codex_protocol::protocol::Event;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::Op;
use codex_protocol::protocol::ReviewDecision;
use codex_protocol::request_permissions::PermissionGrantScope;
use codex_protocol::request_user_input::RequestUserInputResponse;
use codex_protocol::user_input::UserInput;
use serde::Deserialize;
use serde::Serialize;
use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::VecDeque;
use std::path::PathBuf;

const LIVE_BRIDGE_EVENT_CAPACITY: usize = 32_768;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LiveBridgeLiveState {
    #[default]
    Unavailable,
    Idle,
    Generating,
    WaitingApproval,
    WaitingUserInput,
    Stopped,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LiveBridgeSessionState {
    pub window_id: String,
    pub thread_id: Option<ThreadId>,
    pub pid: u32,
    pub cwd: Option<PathBuf>,
    pub live_state: LiveBridgeLiveState,
    pub current_turn_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LiveBridgeSnapshot {
    pub session_state: LiveBridgeSessionState,
    pub session_configured: Option<Event>,
    pub buffered_events: Vec<Event>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum LiveBridgeCommand {
    Snapshot,
    Subscribe,
    SubmitInput {
        items: Vec<UserInput>,
    },
    SteerInput {
        items: Vec<UserInput>,
        expected_turn_id: Option<String>,
    },
    Interrupt,
    Approve {
        action: LiveBridgeApprovalAction,
    },
    Deny {
        action: LiveBridgeDenialAction,
    },
    AnswerUserInput {
        turn_id: String,
        response: RequestUserInputResponse,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum LiveBridgeApprovalAction {
    Exec {
        id: String,
        turn_id: Option<String>,
        decision: Option<ReviewDecision>,
    },
    Patch {
        id: String,
        decision: Option<ReviewDecision>,
    },
    Permissions {
        id: String,
        permissions: PermissionProfile,
        scope: Option<PermissionGrantScope>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum LiveBridgeDenialAction {
    Exec {
        id: String,
        turn_id: Option<String>,
        abort: bool,
    },
    Patch {
        id: String,
        abort: bool,
    },
    Permissions {
        id: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum LiveBridgeResponse {
    Ack,
    Snapshot { snapshot: LiveBridgeSnapshot },
    Submitted { submission_id: String },
    Steered { turn_id: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LiveBridgeClientFrame {
    pub id: String,
    pub command: LiveBridgeCommand,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum LiveBridgeServerFrame {
    Response {
        id: String,
        response: LiveBridgeResponse,
    },
    Error {
        id: Option<String>,
        message: String,
    },
    Event {
        event: LiveBridgeEvent,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum LiveBridgeEvent {
    SessionState { state: LiveBridgeSessionState },
    TurnStarted { event: Event },
    TurnCompleted { event: Event },
    MessageDelta { event: Event },
    MessageFinalized { event: Event },
    ApprovalRequested { event: Event },
    RequestUserInputRequested { event: Event },
    OwnerClosed,
}

#[derive(Debug)]
struct LiveBridgeState {
    codex_home: PathBuf,
    window_id: String,
    pid: u32,
    socket_path: PathBuf,
    registry_path: PathBuf,
    lease_path: Option<PathBuf>,
    stale_lease_paths: Vec<PathBuf>,
    lease_created_at: Option<i64>,
    thread_id: Option<ThreadId>,
    cwd: Option<PathBuf>,
    live_state: LiveBridgeLiveState,
    current_turn_id: Option<String>,
    last_heartbeat_at: i64,
    session_configured: Option<Event>,
    buffered_events: VecDeque<Event>,
    next_connection_id: usize,
    connections: HashMap<usize, ConnectionState>,
    exec_approval_ids: HashSet<String>,
    exec_approval_ids_by_turn_id: HashMap<String, Vec<String>>,
    patch_approval_ids: HashSet<String>,
    patch_approval_ids_by_turn_id: HashMap<String, Vec<String>>,
    request_permissions_ids: HashSet<String>,
    request_permissions_ids_by_turn_id: HashMap<String, Vec<String>>,
    request_user_input_ids: HashSet<String>,
    request_user_input_ids_by_turn_id: HashMap<String, Vec<String>>,
}

#[derive(Debug)]
struct ConnectionState {
    subscribed: bool,
    sender: std::sync::mpsc::Sender<LiveBridgeServerFrame>,
}

impl LiveBridgeState {
    fn new(
        codex_home: PathBuf,
        window_id: String,
        socket_path: PathBuf,
        registry_path: PathBuf,
    ) -> Self {
        Self {
            codex_home,
            window_id,
            pid: std::process::id(),
            socket_path,
            registry_path,
            lease_path: None,
            stale_lease_paths: Vec::new(),
            lease_created_at: None,
            thread_id: None,
            cwd: None,
            live_state: LiveBridgeLiveState::Unavailable,
            current_turn_id: None,
            last_heartbeat_at: unix_timestamp_now(),
            session_configured: None,
            buffered_events: VecDeque::with_capacity(LIVE_BRIDGE_EVENT_CAPACITY),
            next_connection_id: 0,
            connections: HashMap::new(),
            exec_approval_ids: HashSet::new(),
            exec_approval_ids_by_turn_id: HashMap::new(),
            patch_approval_ids: HashSet::new(),
            patch_approval_ids_by_turn_id: HashMap::new(),
            request_permissions_ids: HashSet::new(),
            request_permissions_ids_by_turn_id: HashMap::new(),
            request_user_input_ids: HashSet::new(),
            request_user_input_ids_by_turn_id: HashMap::new(),
        }
    }

    fn snapshot(&self) -> LiveBridgeSnapshot {
        LiveBridgeSnapshot {
            session_state: self.session_state(),
            session_configured: self.session_configured.clone(),
            buffered_events: self
                .buffered_events
                .iter()
                .filter(|event| self.should_include_snapshot_event(event))
                .cloned()
                .collect(),
        }
    }

    fn session_state(&self) -> LiveBridgeSessionState {
        LiveBridgeSessionState {
            window_id: self.window_id.clone(),
            thread_id: self.thread_id,
            pid: self.pid,
            cwd: self.cwd.clone(),
            live_state: self.live_state,
            current_turn_id: self.current_turn_id.clone(),
        }
    }

    fn registration(&self) -> Option<RuntimeLiveRegistration> {
        Some(RuntimeLiveRegistration {
            window_id: self.window_id.clone(),
            thread_id: self.thread_id?,
            pid: self.pid,
            cwd: self.cwd.clone()?,
            socket_path: self.socket_path.clone(),
            last_heartbeat_at: self.last_heartbeat_at,
            runtime_owner: RuntimeOwner::Tui,
        })
    }

    fn owner_lease(&self) -> Option<ThreadOwnerLease> {
        Some(ThreadOwnerLease {
            thread_id: self.thread_id?,
            runtime_owner: RuntimeOwner::Tui,
            pid: self.pid,
            window_id: Some(self.window_id.clone()),
            created_at: self.lease_created_at?,
            last_heartbeat_at: self.last_heartbeat_at,
        })
    }

    fn add_connection(&mut self, sender: std::sync::mpsc::Sender<LiveBridgeServerFrame>) -> usize {
        let connection_id = self.next_connection_id;
        self.next_connection_id += 1;
        self.connections.insert(
            connection_id,
            ConnectionState {
                subscribed: false,
                sender,
            },
        );
        connection_id
    }

    fn remove_connection(&mut self, connection_id: usize) {
        self.connections.remove(&connection_id);
    }

    fn subscribe(&mut self, connection_id: usize) {
        if let Some(connection) = self.connections.get_mut(&connection_id) {
            connection.subscribed = true;
        }
    }

    fn observe_event(&mut self, event: Event) -> Vec<LiveBridgeServerFrame> {
        if let EventMsg::SessionConfigured(session) = &event.msg {
            self.clear_runtime_state();
            self.thread_id = Some(session.session_id);
            self.lease_path = Some(runtime_owner_lease_path(
                &self.codex_home,
                &session.session_id,
            ));
            self.lease_created_at = Some(unix_timestamp_now());
            self.cwd = Some(session.cwd.clone());
            self.session_configured = Some(event.clone());
            self.live_state = LiveBridgeLiveState::Idle;
        } else {
            self.push_buffered_event(event.clone());
        }

        let previous_state = self.live_state;
        let mut frames = Vec::new();
        match &event.msg {
            EventMsg::SessionConfigured(_) => {}
            EventMsg::TurnStarted(turn) => {
                self.current_turn_id = Some(turn.turn_id.clone());
                frames.push(LiveBridgeServerFrame::Event {
                    event: LiveBridgeEvent::TurnStarted {
                        event: event.clone(),
                    },
                });
            }
            EventMsg::TurnComplete(turn) => {
                self.clear_turn(turn.turn_id.as_str());
                if self.current_turn_id.as_deref() == Some(turn.turn_id.as_str()) {
                    self.current_turn_id = None;
                }
                frames.push(LiveBridgeServerFrame::Event {
                    event: LiveBridgeEvent::TurnCompleted {
                        event: event.clone(),
                    },
                });
            }
            EventMsg::TurnAborted(turn) => {
                if let Some(turn_id) = &turn.turn_id {
                    self.clear_turn(turn_id);
                    if self.current_turn_id.as_deref() == Some(turn_id.as_str()) {
                        self.current_turn_id = None;
                    }
                }
                frames.push(LiveBridgeServerFrame::Event {
                    event: LiveBridgeEvent::TurnCompleted {
                        event: event.clone(),
                    },
                });
            }
            EventMsg::ExecApprovalRequest(ev) => {
                let approval_id = ev.effective_approval_id();
                self.exec_approval_ids.insert(approval_id.clone());
                self.exec_approval_ids_by_turn_id
                    .entry(ev.turn_id.clone())
                    .or_default()
                    .push(approval_id);
                frames.push(LiveBridgeServerFrame::Event {
                    event: LiveBridgeEvent::ApprovalRequested {
                        event: event.clone(),
                    },
                });
            }
            EventMsg::ApplyPatchApprovalRequest(ev) => {
                self.patch_approval_ids.insert(ev.call_id.clone());
                self.patch_approval_ids_by_turn_id
                    .entry(ev.turn_id.clone())
                    .or_default()
                    .push(ev.call_id.clone());
                frames.push(LiveBridgeServerFrame::Event {
                    event: LiveBridgeEvent::ApprovalRequested {
                        event: event.clone(),
                    },
                });
            }
            EventMsg::RequestPermissions(ev) => {
                self.request_permissions_ids.insert(ev.call_id.clone());
                self.request_permissions_ids_by_turn_id
                    .entry(ev.turn_id.clone())
                    .or_default()
                    .push(ev.call_id.clone());
                frames.push(LiveBridgeServerFrame::Event {
                    event: LiveBridgeEvent::ApprovalRequested {
                        event: event.clone(),
                    },
                });
            }
            EventMsg::RequestUserInput(ev) => {
                self.request_user_input_ids.insert(ev.call_id.clone());
                self.request_user_input_ids_by_turn_id
                    .entry(ev.turn_id.clone())
                    .or_default()
                    .push(ev.call_id.clone());
                frames.push(LiveBridgeServerFrame::Event {
                    event: LiveBridgeEvent::RequestUserInputRequested {
                        event: event.clone(),
                    },
                });
            }
            EventMsg::ExecCommandBegin(ev) => {
                self.exec_approval_ids.remove(&ev.call_id);
                Self::remove_call_id_from_turn_map(
                    &mut self.exec_approval_ids_by_turn_id,
                    &ev.call_id,
                );
            }
            EventMsg::PatchApplyBegin(ev) => {
                self.patch_approval_ids.remove(&ev.call_id);
                Self::remove_call_id_from_turn_map(
                    &mut self.patch_approval_ids_by_turn_id,
                    &ev.call_id,
                );
            }
            EventMsg::AgentMessageContentDelta(_)
            | EventMsg::ReasoningContentDelta(_)
            | EventMsg::ReasoningRawContentDelta(_)
            | EventMsg::PlanDelta(_) => {
                frames.push(LiveBridgeServerFrame::Event {
                    event: LiveBridgeEvent::MessageDelta {
                        event: event.clone(),
                    },
                });
            }
            EventMsg::ItemCompleted(completed)
                if matches!(
                    completed.item,
                    codex_protocol::items::TurnItem::AgentMessage(_)
                        | codex_protocol::items::TurnItem::Reasoning(_)
                ) =>
            {
                frames.push(LiveBridgeServerFrame::Event {
                    event: LiveBridgeEvent::MessageFinalized {
                        event: event.clone(),
                    },
                });
            }
            EventMsg::ShutdownComplete => {
                self.current_turn_id = None;
                self.clear_pending_requests();
                self.live_state = LiveBridgeLiveState::Stopped;
                frames.push(LiveBridgeServerFrame::Event {
                    event: LiveBridgeEvent::OwnerClosed,
                });
            }
            _ => {}
        }

        let recalculated_state = self.derive_live_state();
        self.live_state = recalculated_state;
        if self.live_state != previous_state || matches!(&event.msg, EventMsg::SessionConfigured(_))
        {
            frames.push(LiveBridgeServerFrame::Event {
                event: LiveBridgeEvent::SessionState {
                    state: self.session_state(),
                },
            });
        }
        frames
    }

    fn note_outbound_op(&mut self, op: &Op) -> Vec<LiveBridgeServerFrame> {
        let previous_state = self.live_state;
        match op {
            Op::ExecApproval { id, turn_id, .. } => {
                self.exec_approval_ids.remove(id);
                if let Some(turn_id) = turn_id {
                    Self::remove_call_id_from_turn_map_entry(
                        &mut self.exec_approval_ids_by_turn_id,
                        turn_id,
                        id,
                    );
                }
            }
            Op::PatchApproval { id, .. } => {
                self.patch_approval_ids.remove(id);
                Self::remove_call_id_from_turn_map(&mut self.patch_approval_ids_by_turn_id, id);
            }
            Op::RequestPermissionsResponse { id, .. } => {
                self.request_permissions_ids.remove(id);
                Self::remove_call_id_from_turn_map(
                    &mut self.request_permissions_ids_by_turn_id,
                    id,
                );
            }
            Op::UserInputAnswer { id, .. } => {
                let mut remove_turn_entry = false;
                if let Some(call_ids) = self.request_user_input_ids_by_turn_id.get_mut(id) {
                    if !call_ids.is_empty() {
                        let call_id = call_ids.remove(0);
                        self.request_user_input_ids.remove(&call_id);
                    }
                    if call_ids.is_empty() {
                        remove_turn_entry = true;
                    }
                }
                if remove_turn_entry {
                    self.request_user_input_ids_by_turn_id.remove(id);
                }
            }
            Op::Shutdown => {
                self.current_turn_id = None;
                self.clear_pending_requests();
                self.live_state = LiveBridgeLiveState::Stopped;
            }
            _ => {}
        }

        self.live_state = self.derive_live_state();
        if self.live_state != previous_state {
            vec![LiveBridgeServerFrame::Event {
                event: LiveBridgeEvent::SessionState {
                    state: self.session_state(),
                },
            }]
        } else {
            Vec::new()
        }
    }

    fn detach(&mut self) -> Vec<LiveBridgeServerFrame> {
        if self.thread_id.is_none() && self.live_state == LiveBridgeLiveState::Unavailable {
            return Vec::new();
        }

        self.clear_runtime_state();
        vec![
            LiveBridgeServerFrame::Event {
                event: LiveBridgeEvent::OwnerClosed,
            },
            LiveBridgeServerFrame::Event {
                event: LiveBridgeEvent::SessionState {
                    state: self.session_state(),
                },
            },
        ]
    }

    fn clear_runtime_state(&mut self) {
        if let Some(lease_path) = self.lease_path.take() {
            self.stale_lease_paths.push(lease_path);
        }
        self.thread_id = None;
        self.cwd = None;
        self.current_turn_id = None;
        self.live_state = LiveBridgeLiveState::Unavailable;
        self.lease_created_at = None;
        self.session_configured = None;
        self.buffered_events.clear();
        self.clear_pending_requests();
    }

    fn clear_pending_requests(&mut self) {
        self.exec_approval_ids.clear();
        self.exec_approval_ids_by_turn_id.clear();
        self.patch_approval_ids.clear();
        self.patch_approval_ids_by_turn_id.clear();
        self.request_permissions_ids.clear();
        self.request_permissions_ids_by_turn_id.clear();
        self.request_user_input_ids.clear();
        self.request_user_input_ids_by_turn_id.clear();
    }

    fn clear_turn(&mut self, turn_id: &str) {
        Self::clear_call_ids_for_turn(
            &mut self.exec_approval_ids,
            &mut self.exec_approval_ids_by_turn_id,
            turn_id,
        );
        Self::clear_call_ids_for_turn(
            &mut self.patch_approval_ids,
            &mut self.patch_approval_ids_by_turn_id,
            turn_id,
        );
        Self::clear_call_ids_for_turn(
            &mut self.request_permissions_ids,
            &mut self.request_permissions_ids_by_turn_id,
            turn_id,
        );
        Self::clear_call_ids_for_turn(
            &mut self.request_user_input_ids,
            &mut self.request_user_input_ids_by_turn_id,
            turn_id,
        );
    }

    fn clear_call_ids_for_turn(
        ids: &mut HashSet<String>,
        ids_by_turn: &mut HashMap<String, Vec<String>>,
        turn_id: &str,
    ) {
        if let Some(call_ids) = ids_by_turn.remove(turn_id) {
            for call_id in call_ids {
                ids.remove(&call_id);
            }
        }
    }

    fn derive_live_state(&self) -> LiveBridgeLiveState {
        if self.thread_id.is_none() {
            return LiveBridgeLiveState::Unavailable;
        }
        if self.live_state == LiveBridgeLiveState::Stopped {
            return LiveBridgeLiveState::Stopped;
        }
        if !self.request_user_input_ids.is_empty() {
            return LiveBridgeLiveState::WaitingUserInput;
        }
        if !self.exec_approval_ids.is_empty()
            || !self.patch_approval_ids.is_empty()
            || !self.request_permissions_ids.is_empty()
        {
            return LiveBridgeLiveState::WaitingApproval;
        }
        if self.current_turn_id.is_some() {
            return LiveBridgeLiveState::Generating;
        }
        LiveBridgeLiveState::Idle
    }

    fn push_buffered_event(&mut self, event: Event) {
        if matches!(event.msg, EventMsg::SessionConfigured(_)) {
            return;
        }
        self.buffered_events.push_back(event);
        if self.buffered_events.len() > LIVE_BRIDGE_EVENT_CAPACITY {
            self.buffered_events.pop_front();
        }
    }

    fn should_include_snapshot_event(&self, event: &Event) -> bool {
        match &event.msg {
            EventMsg::ExecApprovalRequest(ev) => {
                self.exec_approval_ids.contains(&ev.effective_approval_id())
            }
            EventMsg::ApplyPatchApprovalRequest(ev) => {
                self.patch_approval_ids.contains(&ev.call_id)
            }
            EventMsg::RequestPermissions(ev) => self.request_permissions_ids.contains(&ev.call_id),
            EventMsg::RequestUserInput(ev) => self.request_user_input_ids.contains(&ev.call_id),
            _ => true,
        }
    }

    fn publish(&mut self, frames: Vec<LiveBridgeServerFrame>) {
        if frames.is_empty() {
            return;
        }

        self.connections.retain(|_, connection| {
            if !connection.subscribed {
                return true;
            }
            frames
                .iter()
                .all(|frame| connection.sender.send(frame.clone()).is_ok())
        });
    }

    fn remove_call_id_from_turn_map(
        call_ids_by_turn_id: &mut HashMap<String, Vec<String>>,
        call_id: &str,
    ) {
        call_ids_by_turn_id.retain(|_, call_ids| {
            call_ids.retain(|queued_call_id| queued_call_id != call_id);
            !call_ids.is_empty()
        });
    }

    fn remove_call_id_from_turn_map_entry(
        call_ids_by_turn_id: &mut HashMap<String, Vec<String>>,
        turn_id: &str,
        call_id: &str,
    ) {
        let mut remove_turn_entry = false;
        if let Some(call_ids) = call_ids_by_turn_id.get_mut(turn_id) {
            call_ids.retain(|queued_call_id| queued_call_id != call_id);
            if call_ids.is_empty() {
                remove_turn_entry = true;
            }
        }
        if remove_turn_entry {
            call_ids_by_turn_id.remove(turn_id);
        }
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use super::LiveBridgeApprovalAction;
    use super::LiveBridgeClientFrame;
    use super::LiveBridgeCommand;
    use super::LiveBridgeDenialAction;
    use super::LiveBridgeResponse;
    use super::LiveBridgeServerFrame;
    use super::LiveBridgeState;
    use super::unix_timestamp_now;
    use crate::app_event::AppEvent;
    use crate::app_event_sender::AppEventSender;
    use codex_core::SteerInputError;
    use codex_core::ThreadManager;
    use codex_protocol::protocol::Event;
    use codex_protocol::protocol::Op;
    use codex_protocol::protocol::ReviewDecision;
    use std::fs;
    use std::io;
    use std::io::BufRead;
    use std::io::BufReader;
    use std::io::Write;
    use std::os::unix::net::UnixListener;
    use std::os::unix::net::UnixStream;
    use std::path::Path;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::atomic::AtomicBool;
    use std::sync::atomic::Ordering;
    use std::thread::JoinHandle;
    use std::time::Duration;
    use uuid::Uuid;

    const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);

    pub(crate) struct LiveBridgeHandle {
        state: Arc<Mutex<LiveBridgeState>>,
        stop_flag: Arc<AtomicBool>,
        listener_thread: Option<JoinHandle<()>>,
        heartbeat_thread: Option<JoinHandle<()>>,
        startup_warning: Option<String>,
    }

    struct CommandExecutor {
        runtime_handle: tokio::runtime::Handle,
        server: Arc<ThreadManager>,
        app_event_tx: AppEventSender,
        state: Arc<Mutex<LiveBridgeState>>,
    }

    impl CommandExecutor {
        fn execute(&self, command: LiveBridgeCommand) -> Result<ExecutionResult, String> {
            let thread_id = {
                let state = self
                    .state
                    .lock()
                    .map_err(|_| "live bridge lock poisoned".to_string())?;
                state.thread_id.ok_or_else(|| {
                    "no active primary thread is attached to this live bridge".to_string()
                })?
            };

            match command {
                LiveBridgeCommand::SubmitInput { items } => {
                    let op = Op::UserInput {
                        items,
                        final_output_json_schema: None,
                    };
                    let submission_id = self.runtime_handle.block_on(async {
                        let thread = self
                            .server
                            .get_thread(thread_id)
                            .await
                            .map_err(|err| err.to_string())?;
                        thread
                            .submit(op.clone())
                            .await
                            .map_err(|err| err.to_string())
                    })?;
                    self.note_thread_op(thread_id, &op);
                    Ok(ExecutionResult {
                        response: LiveBridgeResponse::Submitted { submission_id },
                        noted_op: Some(op),
                    })
                }
                LiveBridgeCommand::SteerInput {
                    items,
                    expected_turn_id,
                } => {
                    let turn_id = self.runtime_handle.block_on(async {
                        let thread = self
                            .server
                            .get_thread(thread_id)
                            .await
                            .map_err(|err| err.to_string())?;
                        thread
                            .steer_input(items, expected_turn_id.as_deref())
                            .await
                            .map_err(|err| match err {
                                SteerInputError::NoActiveTurn(_) => {
                                    "cannot steer input without an active turn".to_string()
                                }
                                SteerInputError::ExpectedTurnMismatch { expected, actual } => {
                                    format!("expected active turn {expected}, found {actual}")
                                }
                                SteerInputError::EmptyInput => {
                                    "cannot steer an empty input payload".to_string()
                                }
                            })
                    })?;
                    Ok(ExecutionResult {
                        response: LiveBridgeResponse::Steered { turn_id },
                        noted_op: None,
                    })
                }
                LiveBridgeCommand::Interrupt => {
                    let op = Op::Interrupt;
                    self.runtime_handle.block_on(async {
                        let thread = self
                            .server
                            .get_thread(thread_id)
                            .await
                            .map_err(|err| err.to_string())?;
                        thread
                            .submit(op.clone())
                            .await
                            .map_err(|err| err.to_string())
                    })?;
                    Ok(ExecutionResult {
                        response: LiveBridgeResponse::Ack,
                        noted_op: None,
                    })
                }
                LiveBridgeCommand::Approve { action } => {
                    let op = match action {
                        LiveBridgeApprovalAction::Exec {
                            id,
                            turn_id,
                            decision,
                        } => Op::ExecApproval {
                            id,
                            turn_id,
                            decision: decision.unwrap_or(ReviewDecision::Approved),
                        },
                        LiveBridgeApprovalAction::Patch { id, decision } => Op::PatchApproval {
                            id,
                            decision: decision.unwrap_or(ReviewDecision::Approved),
                        },
                        LiveBridgeApprovalAction::Permissions {
                            id,
                            permissions,
                            scope,
                        } => Op::RequestPermissionsResponse {
                            id,
                            response:
                                codex_protocol::request_permissions::RequestPermissionsResponse {
                                    permissions,
                                    scope: scope.unwrap_or_default(),
                                },
                        },
                    };
                    self.runtime_handle.block_on(async {
                        let thread = self
                            .server
                            .get_thread(thread_id)
                            .await
                            .map_err(|err| err.to_string())?;
                        thread
                            .submit(op.clone())
                            .await
                            .map_err(|err| err.to_string())
                    })?;
                    self.note_thread_op(thread_id, &op);
                    Ok(ExecutionResult {
                        response: LiveBridgeResponse::Ack,
                        noted_op: Some(op),
                    })
                }
                LiveBridgeCommand::Deny { action } => {
                    let op = match action {
                        LiveBridgeDenialAction::Exec { id, turn_id, abort } => Op::ExecApproval {
                            id,
                            turn_id,
                            decision: if abort {
                                ReviewDecision::Abort
                            } else {
                                ReviewDecision::Denied
                            },
                        },
                        LiveBridgeDenialAction::Patch { id, abort } => Op::PatchApproval {
                            id,
                            decision: if abort {
                                ReviewDecision::Abort
                            } else {
                                ReviewDecision::Denied
                            },
                        },
                        LiveBridgeDenialAction::Permissions { id } => Op::RequestPermissionsResponse {
                            id,
                            response: codex_protocol::request_permissions::RequestPermissionsResponse {
                                permissions: Default::default(),
                                scope: Default::default(),
                            },
                        },
                    };
                    self.runtime_handle.block_on(async {
                        let thread = self
                            .server
                            .get_thread(thread_id)
                            .await
                            .map_err(|err| err.to_string())?;
                        thread
                            .submit(op.clone())
                            .await
                            .map_err(|err| err.to_string())
                    })?;
                    self.note_thread_op(thread_id, &op);
                    Ok(ExecutionResult {
                        response: LiveBridgeResponse::Ack,
                        noted_op: Some(op),
                    })
                }
                LiveBridgeCommand::AnswerUserInput { turn_id, response } => {
                    let op = Op::UserInputAnswer {
                        id: turn_id,
                        response,
                    };
                    self.runtime_handle.block_on(async {
                        let thread = self
                            .server
                            .get_thread(thread_id)
                            .await
                            .map_err(|err| err.to_string())?;
                        thread
                            .submit(op.clone())
                            .await
                            .map_err(|err| err.to_string())
                    })?;
                    self.note_thread_op(thread_id, &op);
                    Ok(ExecutionResult {
                        response: LiveBridgeResponse::Ack,
                        noted_op: Some(op),
                    })
                }
                LiveBridgeCommand::Snapshot | LiveBridgeCommand::Subscribe => {
                    Err("command is handled by the transport layer".to_string())
                }
            }
        }

        fn note_thread_op(&self, thread_id: codex_protocol::ThreadId, op: &Op) {
            self.app_event_tx.send(AppEvent::NoteThreadOp {
                thread_id,
                op: op.clone(),
            });
        }
    }

    struct ExecutionResult {
        response: LiveBridgeResponse,
        noted_op: Option<Op>,
    }

    impl LiveBridgeHandle {
        pub(crate) fn new(
            codex_home: &Path,
            server: Arc<ThreadManager>,
            app_event_tx: AppEventSender,
        ) -> Self {
            match Self::try_new(codex_home, server, app_event_tx) {
                Ok(handle) => handle,
                Err(err) => Self {
                    state: Arc::new(Mutex::new(LiveBridgeState::new(
                        PathBuf::new(),
                        Uuid::new_v4().to_string(),
                        PathBuf::new(),
                        PathBuf::new(),
                    ))),
                    stop_flag: Arc::new(AtomicBool::new(true)),
                    listener_thread: None,
                    heartbeat_thread: None,
                    startup_warning: Some(format!("Failed to start macOS live bridge: {err}")),
                },
            }
        }

        fn try_new(
            codex_home: &Path,
            server: Arc<ThreadManager>,
            app_event_tx: AppEventSender,
        ) -> io::Result<Self> {
            let runtime_handle = tokio::runtime::Handle::try_current().map_err(|err| {
                io::Error::other(format!("tokio runtime unavailable for live bridge: {err}"))
            })?;
            let runtime_dir = codex_core::runtime_owner::runtime_live_dir(codex_home);
            fs::create_dir_all(&runtime_dir)?;
            let window_id = Uuid::new_v4().to_string();
            let socket_path =
                codex_core::runtime_owner::runtime_live_socket_path(codex_home, &window_id);
            let registry_path =
                codex_core::runtime_owner::runtime_live_registration_path(codex_home, &window_id);
            if socket_path.exists() {
                let _ = fs::remove_file(&socket_path);
            }
            let listener = UnixListener::bind(&socket_path)?;
            let state = Arc::new(Mutex::new(LiveBridgeState::new(
                codex_home.to_path_buf(),
                window_id,
                socket_path,
                registry_path,
            )));
            let stop_flag = Arc::new(AtomicBool::new(false));
            let executor = Arc::new(CommandExecutor {
                runtime_handle,
                server,
                app_event_tx,
                state: Arc::clone(&state),
            });
            let listener_thread = Some(spawn_listener_thread(
                listener,
                Arc::clone(&state),
                executor,
                Arc::clone(&stop_flag),
            ));
            let heartbeat_thread = Some(spawn_heartbeat_thread(
                Arc::clone(&state),
                Arc::clone(&stop_flag),
            ));
            Ok(Self {
                state,
                stop_flag,
                listener_thread,
                heartbeat_thread,
                startup_warning: None,
            })
        }

        pub(crate) fn take_startup_warning(&mut self) -> Option<String> {
            self.startup_warning.take()
        }

        pub(crate) fn observe_primary_event(&self, event: Event) {
            let mut state = match self.state.lock() {
                Ok(state) => state,
                Err(_) => return,
            };
            let frames = state.observe_event(event);
            persist_runtime_files(&mut state);
            state.publish(frames);
        }

        pub(crate) fn observe_primary_op(&self, op: &Op) {
            let mut state = match self.state.lock() {
                Ok(state) => state,
                Err(_) => return,
            };
            let frames = state.note_outbound_op(op);
            persist_runtime_files(&mut state);
            state.publish(frames);
        }

        pub(crate) fn detach_primary_thread(&self) {
            let mut state = match self.state.lock() {
                Ok(state) => state,
                Err(_) => return,
            };
            let frames = state.detach();
            persist_runtime_files(&mut state);
            state.publish(frames);
        }

        pub(crate) fn shutdown(&mut self) {
            self.stop_flag.store(true, Ordering::Release);
            let socket_path = match self.state.lock() {
                Ok(state) => state.socket_path.clone(),
                Err(_) => PathBuf::new(),
            };
            if !socket_path.as_os_str().is_empty() {
                let _ = UnixStream::connect(&socket_path);
            }
            if let Some(handle) = self.listener_thread.take() {
                let _ = handle.join();
            }
            if let Some(handle) = self.heartbeat_thread.take() {
                let _ = handle.join();
            }
            if let Ok(state) = self.state.lock() {
                let _ = fs::remove_file(&state.socket_path);
                let _ = fs::remove_file(&state.registry_path);
            }
        }
    }

    impl Drop for LiveBridgeHandle {
        fn drop(&mut self) {
            self.shutdown();
        }
    }

    fn spawn_listener_thread(
        listener: UnixListener,
        state: Arc<Mutex<LiveBridgeState>>,
        executor: Arc<CommandExecutor>,
        stop_flag: Arc<AtomicBool>,
    ) -> JoinHandle<()> {
        std::thread::spawn(move || {
            while !stop_flag.load(Ordering::Acquire) {
                let stream = match listener.accept() {
                    Ok((stream, _)) => stream,
                    Err(err) => {
                        if stop_flag.load(Ordering::Acquire) {
                            break;
                        }
                        tracing::warn!(error = %err, "live bridge accept failed");
                        continue;
                    }
                };
                let state = Arc::clone(&state);
                let executor = Arc::clone(&executor);
                let stop_flag = Arc::clone(&stop_flag);
                std::thread::spawn(move || handle_connection(stream, state, executor, stop_flag));
            }
        })
    }

    fn spawn_heartbeat_thread(
        state: Arc<Mutex<LiveBridgeState>>,
        stop_flag: Arc<AtomicBool>,
    ) -> JoinHandle<()> {
        std::thread::spawn(move || {
            while !stop_flag.load(Ordering::Acquire) {
                std::thread::sleep(HEARTBEAT_INTERVAL);
                if stop_flag.load(Ordering::Acquire) {
                    break;
                }
                let mut state = match state.lock() {
                    Ok(state) => state,
                    Err(_) => break,
                };
                state.last_heartbeat_at = unix_timestamp_now();
                persist_runtime_files(&mut state);
            }
        })
    }

    fn handle_connection(
        stream: UnixStream,
        state: Arc<Mutex<LiveBridgeState>>,
        executor: Arc<CommandExecutor>,
        stop_flag: Arc<AtomicBool>,
    ) {
        let write_stream = match stream.try_clone() {
            Ok(stream) => stream,
            Err(err) => {
                tracing::warn!(error = %err, "failed to clone live bridge stream");
                return;
            }
        };
        let (tx, rx) = std::sync::mpsc::channel();
        let connection_id = {
            let mut state = match state.lock() {
                Ok(state) => state,
                Err(_) => return,
            };
            state.add_connection(tx)
        };

        let writer_handle = std::thread::spawn(move || writer_loop(write_stream, rx));
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) => break,
                Ok(_) => {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    let frame = match serde_json::from_str::<LiveBridgeClientFrame>(trimmed) {
                        Ok(frame) => frame,
                        Err(err) => {
                            send_one_off_error(&state, connection_id, None, err.to_string());
                            continue;
                        }
                    };
                    let response = handle_frame(&state, connection_id, &executor, frame);
                    if let Some(response) = response {
                        send_one_off_frame(&state, connection_id, response);
                    }
                }
                Err(err) => {
                    if !stop_flag.load(Ordering::Acquire) {
                        tracing::warn!(error = %err, "live bridge read failed");
                    }
                    break;
                }
            }
        }

        if let Ok(mut state) = state.lock() {
            state.remove_connection(connection_id);
        }
        let _ = writer_handle.join();
    }

    fn writer_loop(mut stream: UnixStream, rx: std::sync::mpsc::Receiver<LiveBridgeServerFrame>) {
        for frame in rx {
            match serde_json::to_vec(&frame) {
                Ok(mut payload) => {
                    payload.push(b'\n');
                    if let Err(err) = stream.write_all(&payload) {
                        tracing::warn!(error = %err, "live bridge write failed");
                        break;
                    }
                }
                Err(err) => {
                    tracing::warn!(error = %err, "failed to serialize live bridge frame");
                    break;
                }
            }
        }
    }

    fn handle_frame(
        state: &Arc<Mutex<LiveBridgeState>>,
        connection_id: usize,
        executor: &Arc<CommandExecutor>,
        frame: LiveBridgeClientFrame,
    ) -> Option<LiveBridgeServerFrame> {
        match frame.command {
            LiveBridgeCommand::Snapshot => {
                let snapshot = match state.lock() {
                    Ok(state) => state.snapshot(),
                    Err(_) => {
                        return Some(LiveBridgeServerFrame::Error {
                            id: Some(frame.id),
                            message: "live bridge lock poisoned".to_string(),
                        });
                    }
                };
                Some(LiveBridgeServerFrame::Response {
                    id: frame.id,
                    response: LiveBridgeResponse::Snapshot { snapshot },
                })
            }
            LiveBridgeCommand::Subscribe => {
                if let Ok(mut state) = state.lock() {
                    state.subscribe(connection_id);
                }
                Some(LiveBridgeServerFrame::Response {
                    id: frame.id,
                    response: LiveBridgeResponse::Ack,
                })
            }
            command => match executor.execute(command) {
                Ok(result) => {
                    if let Some(op) = result.noted_op.as_ref()
                        && let Ok(mut state) = state.lock()
                    {
                        let frames = state.note_outbound_op(op);
                        persist_runtime_files(&mut state);
                        state.publish(frames);
                    }
                    Some(LiveBridgeServerFrame::Response {
                        id: frame.id,
                        response: result.response,
                    })
                }
                Err(message) => Some(LiveBridgeServerFrame::Error {
                    id: Some(frame.id),
                    message,
                }),
            },
        }
    }

    fn send_one_off_frame(
        state: &Arc<Mutex<LiveBridgeState>>,
        connection_id: usize,
        frame: LiveBridgeServerFrame,
    ) {
        if let Ok(state) = state.lock()
            && let Some(connection) = state.connections.get(&connection_id)
        {
            let _ = connection.sender.send(frame);
        }
    }

    fn send_one_off_error(
        state: &Arc<Mutex<LiveBridgeState>>,
        connection_id: usize,
        id: Option<String>,
        message: String,
    ) {
        send_one_off_frame(
            state,
            connection_id,
            LiveBridgeServerFrame::Error { id, message },
        );
    }

    fn persist_runtime_files(state: &mut LiveBridgeState) {
        if state.registry_path.as_os_str().is_empty() {
            return;
        }

        for path in std::mem::take(&mut state.stale_lease_paths) {
            let _ = fs::remove_file(path);
        }

        if let Some(registration) = state.registration() {
            if let Ok(serialized) = serde_json::to_vec_pretty(&registration) {
                if let Some(parent) = state.registry_path.parent() {
                    let _ = fs::create_dir_all(parent);
                }
                let _ = fs::write(&state.registry_path, serialized);
            }
        } else {
            let _ = fs::remove_file(&state.registry_path);
        }

        if let Some(lease) = state.owner_lease() {
            if let Some(path) = state.lease_path.as_ref()
                && let Ok(serialized) = serde_json::to_vec_pretty(&lease)
            {
                if let Some(parent) = path.parent() {
                    let _ = fs::create_dir_all(parent);
                }
                let _ = fs::write(path, serialized);
            }
        } else if let Some(path) = state.lease_path.as_ref() {
            let _ = fs::remove_file(path);
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use crate::app_event_sender::AppEventSender;
    use codex_core::ThreadManager;
    use codex_protocol::protocol::Event;
    use codex_protocol::protocol::Op;
    use std::path::Path;
    use std::sync::Arc;

    pub(crate) struct LiveBridgeHandle {
        startup_warning: Option<String>,
    }

    impl LiveBridgeHandle {
        pub(crate) fn new(
            _codex_home: &Path,
            _server: Arc<ThreadManager>,
            _app_event_tx: AppEventSender,
        ) -> Self {
            Self {
                startup_warning: None,
            }
        }

        pub(crate) fn take_startup_warning(&mut self) -> Option<String> {
            self.startup_warning.take()
        }

        pub(crate) fn observe_primary_event(&self, _event: Event) {}

        pub(crate) fn observe_primary_op(&self, _op: &Op) {}

        pub(crate) fn detach_primary_thread(&self) {}

        pub(crate) fn shutdown(&mut self) {}
    }
}

pub(crate) use imp::LiveBridgeHandle;

#[cfg(test)]
mod tests {
    use super::*;
    use codex_protocol::protocol::SessionConfiguredEvent;
    use codex_protocol::protocol::TurnAbortReason;
    use codex_protocol::protocol::TurnAbortedEvent;
    use codex_protocol::protocol::TurnStartedEvent;
    use pretty_assertions::assert_eq;
    use std::path::PathBuf;

    fn sample_session_configured(thread_id: ThreadId) -> Event {
        Event {
            id: "session-configured".to_string(),
            msg: EventMsg::SessionConfigured(SessionConfiguredEvent {
                session_id: thread_id,
                forked_from_id: None,
                thread_name: None,
                model: "gpt-5".to_string(),
                model_provider_id: "openai".to_string(),
                service_tier: None,
                approval_policy: codex_protocol::protocol::AskForApproval::Never,
                sandbox_policy: codex_protocol::protocol::SandboxPolicy::new_read_only_policy(),
                cwd: PathBuf::from("/tmp/project"),
                reasoning_effort: None,
                history_log_id: 0,
                history_entry_count: 0,
                initial_messages: None,
                network_proxy: None,
                rollout_path: None,
            }),
        }
    }

    #[test]
    fn observe_session_configured_sets_idle_session_state() {
        let thread_id = ThreadId::new();
        let mut state = LiveBridgeState::new(
            PathBuf::from("/tmp/codex-home"),
            "window-1".to_string(),
            PathBuf::from("/tmp/window-1.sock"),
            PathBuf::from("/tmp/window-1.json"),
        );

        let frames = state.observe_event(sample_session_configured(thread_id));

        assert_eq!(state.thread_id, Some(thread_id));
        assert_eq!(state.live_state, LiveBridgeLiveState::Idle);
        assert!(frames.iter().any(|frame| matches!(
            frame,
            LiveBridgeServerFrame::Event {
                event: LiveBridgeEvent::SessionState { .. }
            }
        )));
    }

    #[test]
    fn approval_and_user_input_events_drive_live_state() {
        let thread_id = ThreadId::new();
        let mut state = LiveBridgeState::new(
            PathBuf::from("/tmp/codex-home"),
            "window-1".to_string(),
            PathBuf::from("/tmp/window-1.sock"),
            PathBuf::from("/tmp/window-1.json"),
        );
        state.observe_event(sample_session_configured(thread_id));
        state.observe_event(Event {
            id: "turn-started".to_string(),
            msg: EventMsg::TurnStarted(TurnStartedEvent {
                turn_id: "turn-1".to_string(),
                model_context_window: None,
                collaboration_mode_kind: Default::default(),
            }),
        });
        assert_eq!(state.live_state, LiveBridgeLiveState::Generating);

        state.observe_event(Event {
            id: "approval".to_string(),
            msg: EventMsg::ExecApprovalRequest(
                codex_protocol::protocol::ExecApprovalRequestEvent {
                    call_id: "call-1".to_string(),
                    approval_id: None,
                    turn_id: "turn-1".to_string(),
                    command: vec!["echo".to_string(), "hi".to_string()],
                    cwd: PathBuf::from("/tmp/project"),
                    reason: None,
                    network_approval_context: None,
                    proposed_execpolicy_amendment: None,
                    proposed_network_policy_amendments: None,
                    additional_permissions: None,
                    skill_metadata: None,
                    available_decisions: None,
                    parsed_cmd: Vec::new(),
                },
            ),
        });
        assert_eq!(state.live_state, LiveBridgeLiveState::WaitingApproval);

        let frames = state.note_outbound_op(&Op::ExecApproval {
            id: "call-1".to_string(),
            turn_id: Some("turn-1".to_string()),
            decision: ReviewDecision::Approved,
        });
        assert_eq!(state.live_state, LiveBridgeLiveState::Generating);
        assert_eq!(frames.len(), 1);

        state.observe_event(Event {
            id: "user-input".to_string(),
            msg: EventMsg::RequestUserInput(
                codex_protocol::request_user_input::RequestUserInputEvent {
                    call_id: "ui-1".to_string(),
                    turn_id: "turn-1".to_string(),
                    questions: Vec::new(),
                },
            ),
        });
        assert_eq!(state.live_state, LiveBridgeLiveState::WaitingUserInput);

        state.note_outbound_op(&Op::UserInputAnswer {
            id: "turn-1".to_string(),
            response: RequestUserInputResponse {
                answers: HashMap::new(),
            },
        });
        assert_eq!(state.live_state, LiveBridgeLiveState::Generating);
    }

    #[test]
    fn shutdown_or_detach_marks_owner_closed() {
        let thread_id = ThreadId::new();
        let mut state = LiveBridgeState::new(
            PathBuf::from("/tmp/codex-home"),
            "window-1".to_string(),
            PathBuf::from("/tmp/window-1.sock"),
            PathBuf::from("/tmp/window-1.json"),
        );
        state.observe_event(sample_session_configured(thread_id));
        let shutdown_frames = state.observe_event(Event {
            id: "shutdown".to_string(),
            msg: EventMsg::ShutdownComplete,
        });
        assert_eq!(state.live_state, LiveBridgeLiveState::Stopped);
        assert!(shutdown_frames.iter().any(|frame| matches!(
            frame,
            LiveBridgeServerFrame::Event {
                event: LiveBridgeEvent::OwnerClosed
            }
        )));

        let detach_frames = state.detach();
        assert_eq!(state.live_state, LiveBridgeLiveState::Unavailable);
        assert_eq!(state.thread_id, None);
        assert!(detach_frames.iter().any(|frame| matches!(
            frame,
            LiveBridgeServerFrame::Event {
                event: LiveBridgeEvent::SessionState { .. }
            }
        )));
    }

    #[test]
    fn turn_abort_clears_pending_state_for_turn() {
        let thread_id = ThreadId::new();
        let mut state = LiveBridgeState::new(
            PathBuf::from("/tmp/codex-home"),
            "window-1".to_string(),
            PathBuf::from("/tmp/window-1.sock"),
            PathBuf::from("/tmp/window-1.json"),
        );
        state.observe_event(sample_session_configured(thread_id));
        state.observe_event(Event {
            id: "turn-started".to_string(),
            msg: EventMsg::TurnStarted(TurnStartedEvent {
                turn_id: "turn-1".to_string(),
                model_context_window: None,
                collaboration_mode_kind: Default::default(),
            }),
        });
        state.observe_event(Event {
            id: "approval".to_string(),
            msg: EventMsg::RequestPermissions(
                codex_protocol::request_permissions::RequestPermissionsEvent {
                    call_id: "perm-1".to_string(),
                    turn_id: "turn-1".to_string(),
                    reason: None,
                    permissions: Default::default(),
                },
            ),
        });

        state.observe_event(Event {
            id: "abort".to_string(),
            msg: EventMsg::TurnAborted(TurnAbortedEvent {
                turn_id: Some("turn-1".to_string()),
                reason: TurnAbortReason::Interrupted,
            }),
        });

        assert_eq!(state.live_state, LiveBridgeLiveState::Idle);
        assert!(state.request_permissions_ids.is_empty());
    }
}
