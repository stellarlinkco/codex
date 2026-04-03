use anyhow::Context;
use anyhow::Result;
use codex_core::runtime_owner::RuntimeLiveRegistration;
use codex_core::runtime_owner::RuntimeOwner;
use codex_core::runtime_owner::ThreadOwnerLease;
use codex_core::runtime_owner::runtime_leases_dir;
use codex_core::runtime_owner::runtime_live_dir;
use codex_core::runtime_owner::runtime_owner_lease_path;
use codex_core::runtime_owner::unix_timestamp_now;
use codex_protocol::ThreadId;
use codex_protocol::models::PermissionProfile;
use codex_protocol::protocol::Event;
use codex_protocol::protocol::ReviewDecision;
use codex_protocol::request_permissions::PermissionGrantScope;
use codex_protocol::request_user_input::RequestUserInputResponse;
use codex_protocol::user_input::UserInput;
use serde::Deserialize;
use serde::Serialize;
use std::collections::HashMap;
use std::io::ErrorKind;
use std::path::Path;
use std::path::PathBuf;
use tokio::fs;
use tokio::fs::OpenOptions;
use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncWriteExt;
use tokio::io::BufReader;
use tokio::net::UnixStream;
use uuid::Uuid;

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

pub(crate) async fn load_live_registrations(
    codex_home: &Path,
) -> HashMap<ThreadId, RuntimeLiveRegistration> {
    let mut registrations = HashMap::new();
    let mut entries = match fs::read_dir(runtime_live_dir(codex_home)).await {
        Ok(entries) => entries,
        Err(_) => return registrations,
    };

    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let Ok(bytes) = fs::read(&path).await else {
            continue;
        };
        let Ok(registration) = serde_json::from_slice::<RuntimeLiveRegistration>(&bytes) else {
            continue;
        };
        registrations.insert(registration.thread_id, registration);
    }

    registrations
}

pub(crate) async fn load_owner_lease(
    codex_home: &Path,
    thread_id: &ThreadId,
) -> Option<ThreadOwnerLease> {
    let path = runtime_owner_lease_path(codex_home, thread_id);
    let bytes = fs::read(&path).await.ok()?;
    let lease = serde_json::from_slice::<ThreadOwnerLease>(&bytes).ok()?;
    if lease.is_stale(unix_timestamp_now()) {
        let _ = fs::remove_file(path).await;
        return None;
    }
    Some(lease)
}

pub(crate) async fn load_live_registration_for_thread(
    codex_home: &Path,
    thread_id: &ThreadId,
) -> Option<RuntimeLiveRegistration> {
    let lease = load_owner_lease(codex_home, thread_id).await?;
    let window_id = lease.window_id.as_deref()?;
    let registration = load_live_registrations(codex_home)
        .await
        .remove(thread_id)?;
    if registration.window_id != window_id || registration.pid != lease.pid {
        return None;
    }
    Some(registration)
}

pub(crate) async fn write_headless_owner_lease(
    codex_home: &Path,
    thread_id: ThreadId,
    pid: u32,
    last_heartbeat_at: i64,
    created_at: i64,
) -> Result<()> {
    let lease = headless_owner_lease(thread_id, pid, last_heartbeat_at, created_at);
    ensure_owner_lease_dir(codex_home).await?;
    write_json_atomic(
        &runtime_owner_lease_path(codex_home, &lease.thread_id),
        &lease,
    )
    .await
}

pub(crate) async fn claim_headless_owner_lease(
    codex_home: &Path,
    thread_id: ThreadId,
    pid: u32,
    last_heartbeat_at: i64,
    created_at: i64,
) -> Result<Option<ThreadOwnerLease>> {
    let lease = headless_owner_lease(thread_id, pid, last_heartbeat_at, created_at);
    let path = runtime_owner_lease_path(codex_home, &thread_id);
    ensure_owner_lease_dir(codex_home).await?;

    loop {
        let mut body = serde_json::to_vec_pretty(&lease).context("serialize runtime json")?;
        body.push(b'\n');
        match OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
            .await
        {
            Ok(mut file) => {
                file.write_all(&body)
                    .await
                    .with_context(|| format!("write {}", path.display()))?;
                file.flush()
                    .await
                    .with_context(|| format!("flush {}", path.display()))?;
                return Ok(None);
            }
            Err(err) if err.kind() == ErrorKind::AlreadyExists => {
                let Some(existing) = load_owner_lease(codex_home, &thread_id).await else {
                    continue;
                };
                if existing.runtime_owner == RuntimeOwner::Serve && existing.pid == pid {
                    write_json_atomic(&path, &lease).await?;
                    return Ok(None);
                }
                return Ok(Some(existing));
            }
            Err(err) => {
                return Err(err).with_context(|| format!("create {}", path.display()));
            }
        }
    }
}

pub(crate) async fn remove_owner_lease(codex_home: &Path, thread_id: &ThreadId) {
    let _ = fs::remove_file(runtime_owner_lease_path(codex_home, thread_id)).await;
}

pub(crate) async fn bridge_snapshot(
    registration: &RuntimeLiveRegistration,
) -> Result<LiveBridgeSnapshot> {
    let response = bridge_request(registration, LiveBridgeCommand::Snapshot).await?;
    match response {
        LiveBridgeResponse::Snapshot { snapshot } => Ok(snapshot),
        _ => anyhow::bail!("live bridge returned unexpected snapshot response"),
    }
}

pub(crate) async fn bridge_command(
    registration: &RuntimeLiveRegistration,
    command: LiveBridgeCommand,
) -> Result<LiveBridgeResponse> {
    bridge_request(registration, command).await
}

pub(crate) async fn open_subscription(
    registration: &RuntimeLiveRegistration,
) -> Result<BufReader<tokio::net::unix::OwnedReadHalf>> {
    let stream = UnixStream::connect(&registration.socket_path)
        .await
        .with_context(|| format!("connect {}", registration.socket_path.display()))?;
    let (read_half, mut write_half) = stream.into_split();
    for command in [LiveBridgeCommand::Snapshot, LiveBridgeCommand::Subscribe] {
        let frame = LiveBridgeClientFrame {
            id: Uuid::new_v4().to_string(),
            command,
        };
        let mut payload = serde_json::to_vec(&frame).context("serialize live bridge request")?;
        payload.push(b'\n');
        write_half
            .write_all(&payload)
            .await
            .context("write live bridge request")?;
    }
    Ok(BufReader::new(read_half))
}

pub(crate) async fn next_subscription_frame(
    reader: &mut BufReader<tokio::net::unix::OwnedReadHalf>,
) -> Result<Option<LiveBridgeServerFrame>> {
    let mut line = String::new();
    let bytes = reader.read_line(&mut line).await?;
    if bytes == 0 {
        return Ok(None);
    }
    let frame = serde_json::from_str::<LiveBridgeServerFrame>(line.trim())
        .context("parse live bridge frame")?;
    Ok(Some(frame))
}

async fn bridge_request(
    registration: &RuntimeLiveRegistration,
    command: LiveBridgeCommand,
) -> Result<LiveBridgeResponse> {
    let mut stream = UnixStream::connect(&registration.socket_path)
        .await
        .with_context(|| format!("connect {}", registration.socket_path.display()))?;
    let request_id = Uuid::new_v4().to_string();
    let frame = LiveBridgeClientFrame {
        id: request_id.clone(),
        command,
    };
    let mut payload = serde_json::to_vec(&frame).context("serialize live bridge request")?;
    payload.push(b'\n');
    stream
        .write_all(&payload)
        .await
        .context("write live bridge request")?;

    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    loop {
        line.clear();
        let bytes = reader.read_line(&mut line).await?;
        if bytes == 0 {
            anyhow::bail!("live bridge closed without responding");
        }
        match serde_json::from_str::<LiveBridgeServerFrame>(line.trim())
            .context("parse live bridge frame")?
        {
            LiveBridgeServerFrame::Response { id, response } if id == request_id => {
                return Ok(response);
            }
            LiveBridgeServerFrame::Error { id, message }
                if id.as_deref() == Some(request_id.as_str()) =>
            {
                anyhow::bail!(message);
            }
            LiveBridgeServerFrame::Event { .. }
            | LiveBridgeServerFrame::Response { .. }
            | LiveBridgeServerFrame::Error { .. } => {}
        }
    }
}

async fn write_json_atomic<T: Serialize + ?Sized>(path: &Path, value: &T) -> Result<()> {
    let tmp_path = path.with_extension("json.tmp");
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .await
            .with_context(|| format!("create {}", parent.display()))?;
    }
    let mut body = serde_json::to_vec_pretty(value).context("serialize runtime json")?;
    body.push(b'\n');
    fs::write(&tmp_path, body)
        .await
        .with_context(|| format!("write {}", tmp_path.display()))?;
    if let Err(_err) = fs::rename(&tmp_path, path).await {
        let _ = fs::remove_file(path).await;
        fs::rename(&tmp_path, path)
            .await
            .with_context(|| format!("rename {} -> {}", tmp_path.display(), path.display()))?;
    }
    Ok(())
}

async fn ensure_owner_lease_dir(codex_home: &Path) -> Result<()> {
    let dir = runtime_leases_dir(codex_home);
    fs::create_dir_all(&dir)
        .await
        .with_context(|| format!("create owner lease dir {}", dir.display()))?;
    Ok(())
}

fn headless_owner_lease(
    thread_id: ThreadId,
    pid: u32,
    last_heartbeat_at: i64,
    created_at: i64,
) -> ThreadOwnerLease {
    ThreadOwnerLease {
        thread_id,
        runtime_owner: RuntimeOwner::Serve,
        pid,
        window_id: None,
        created_at,
        last_heartbeat_at,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_codex_home(test_name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("codex-serve-{test_name}-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&path).expect("create temp codex home");
        path
    }

    #[tokio::test]
    async fn owner_lease_load_reclaims_stale_file() {
        let codex_home = temp_codex_home("owner-lease-stale");
        let thread_id = ThreadId::new();
        let path = runtime_owner_lease_path(&codex_home, &thread_id);
        let stale_pid = i32::MAX as u32;
        let lease = ThreadOwnerLease {
            thread_id,
            runtime_owner: RuntimeOwner::Tui,
            pid: stale_pid,
            window_id: Some("window-1".to_string()),
            created_at: 1,
            last_heartbeat_at: 1,
        };
        ensure_owner_lease_dir(&codex_home)
            .await
            .expect("create owner lease dir");
        write_json_atomic(&path, &lease)
            .await
            .expect("write stale lease");

        let loaded = load_owner_lease(&codex_home, &thread_id).await;

        assert_eq!(loaded, None);
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(codex_home);
    }

    #[tokio::test]
    async fn owner_lease_claim_rejects_active_foreign_owner() {
        let codex_home = temp_codex_home("owner-lease-claim-conflict");
        let thread_id = ThreadId::new();
        let path = runtime_owner_lease_path(&codex_home, &thread_id);
        let existing = ThreadOwnerLease {
            thread_id,
            runtime_owner: RuntimeOwner::Tui,
            pid: std::process::id(),
            window_id: Some("window-1".to_string()),
            created_at: unix_timestamp_now(),
            last_heartbeat_at: unix_timestamp_now(),
        };
        ensure_owner_lease_dir(&codex_home)
            .await
            .expect("create owner lease dir");
        write_json_atomic(&path, &existing)
            .await
            .expect("write active lease");

        let claimed = claim_headless_owner_lease(
            &codex_home,
            thread_id,
            std::process::id(),
            unix_timestamp_now(),
            unix_timestamp_now(),
        )
        .await
        .expect("claim owner lease");

        assert_eq!(claimed, Some(existing));
        let _ = std::fs::remove_dir_all(codex_home);
    }

    #[tokio::test]
    async fn owner_lease_claim_refresh_and_remove_round_trip() {
        let codex_home = temp_codex_home("owner-lease-round-trip");
        let thread_id = ThreadId::new();
        let created_at = unix_timestamp_now();
        let first_heartbeat = created_at;

        let claimed = claim_headless_owner_lease(
            &codex_home,
            thread_id,
            std::process::id(),
            first_heartbeat,
            created_at,
        )
        .await
        .expect("claim owner lease");
        assert_eq!(claimed, None);

        let second_heartbeat = first_heartbeat + 5;
        write_headless_owner_lease(
            &codex_home,
            thread_id,
            std::process::id(),
            second_heartbeat,
            created_at,
        )
        .await
        .expect("refresh owner lease");

        let loaded = load_owner_lease(&codex_home, &thread_id)
            .await
            .expect("load refreshed owner lease");
        assert_eq!(
            loaded,
            ThreadOwnerLease {
                thread_id,
                runtime_owner: RuntimeOwner::Serve,
                pid: std::process::id(),
                window_id: None,
                created_at,
                last_heartbeat_at: second_heartbeat,
            }
        );

        remove_owner_lease(&codex_home, &thread_id).await;
        assert!(!runtime_owner_lease_path(&codex_home, &thread_id).exists());
        let _ = std::fs::remove_dir_all(codex_home);
    }
}
