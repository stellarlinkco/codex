use codex_protocol::ThreadId;
use serde::Deserialize;
use serde::Serialize;
use std::path::Path;
use std::path::PathBuf;

pub const RUNTIME_DIR: &str = "runtime";
pub const LIVE_DIR: &str = "live";
pub const LEASES_DIR: &str = "leases";
pub const OWNER_LEASE_TIMEOUT_SECS: i64 = 15;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeOwner {
    Tui,
    Cli,
    Serve,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeLiveRegistration {
    pub window_id: String,
    pub thread_id: ThreadId,
    pub pid: u32,
    pub cwd: PathBuf,
    pub socket_path: PathBuf,
    pub last_heartbeat_at: i64,
    pub runtime_owner: RuntimeOwner,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ThreadOwnerLease {
    pub thread_id: ThreadId,
    pub runtime_owner: RuntimeOwner,
    pub pid: u32,
    pub window_id: Option<String>,
    pub created_at: i64,
    pub last_heartbeat_at: i64,
}

impl ThreadOwnerLease {
    pub fn is_stale(&self, now: i64) -> bool {
        now.saturating_sub(self.last_heartbeat_at) > OWNER_LEASE_TIMEOUT_SECS
            && !pid_is_running(self.pid)
    }
}

pub fn runtime_live_dir(codex_home: &Path) -> PathBuf {
    codex_home.join(RUNTIME_DIR).join(LIVE_DIR)
}

pub fn runtime_live_registration_path(codex_home: &Path, window_id: &str) -> PathBuf {
    runtime_live_dir(codex_home).join(format!("{window_id}.json"))
}

pub fn runtime_live_socket_path(codex_home: &Path, window_id: &str) -> PathBuf {
    runtime_live_dir(codex_home).join(format!("{window_id}.sock"))
}

pub fn runtime_leases_dir(codex_home: &Path) -> PathBuf {
    codex_home.join(RUNTIME_DIR).join(LEASES_DIR)
}

pub fn runtime_owner_lease_path(codex_home: &Path, thread_id: &ThreadId) -> PathBuf {
    runtime_leases_dir(codex_home).join(format!("{thread_id}.lock"))
}

pub fn unix_timestamp_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default()
}

#[cfg(unix)]
pub fn pid_is_running(pid: u32) -> bool {
    let result = unsafe { libc::kill(pid as i32, 0) };
    if result == 0 {
        return true;
    }

    match std::io::Error::last_os_error().raw_os_error() {
        Some(libc::EPERM) => true,
        Some(libc::ESRCH) => false,
        Some(_) | None => false,
    }
}

#[cfg(not(unix))]
pub fn pid_is_running(_pid: u32) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn lease_staleness_requires_expired_heartbeat_and_dead_pid() {
        let lease = ThreadOwnerLease {
            thread_id: ThreadId::new(),
            runtime_owner: RuntimeOwner::Serve,
            pid: i32::MAX as u32,
            window_id: None,
            created_at: 100,
            last_heartbeat_at: 100,
        };

        assert_eq!(lease.is_stale(100 + OWNER_LEASE_TIMEOUT_SECS), false);
        assert_eq!(lease.is_stale(100 + OWNER_LEASE_TIMEOUT_SECS + 1), true);
    }
}
