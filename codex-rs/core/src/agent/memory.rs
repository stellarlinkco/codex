use std::path::Path;
use std::path::PathBuf;

pub(crate) fn agent_memory_dir(codex_home: &Path, role_name: &str) -> PathBuf {
    codex_home
        .join("agent-memory")
        .join(sanitize_role(role_name))
}

pub(crate) fn agent_memory_path(codex_home: &Path, role_name: &str) -> PathBuf {
    agent_memory_dir(codex_home, role_name).join("MEMORY.md")
}

pub(crate) async fn read_agent_memory(codex_home: &Path, role_name: &str) -> Option<String> {
    let path = agent_memory_path(codex_home, role_name);
    tokio::fs::read_to_string(&path).await.ok()
}

fn sanitize_role(role_name: &str) -> String {
    let sanitized: String = role_name
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    if sanitized.is_empty() {
        "default".to_string()
    } else {
        sanitized
    }
}
