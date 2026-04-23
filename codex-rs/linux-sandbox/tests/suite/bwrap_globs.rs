#![cfg(target_os = "linux")]
#![allow(clippy::unwrap_used)]

use std::process::Command;

use codex_protocol::permissions::FileSystemAccessMode;
use codex_protocol::permissions::FileSystemPath;
use codex_protocol::permissions::FileSystemSandboxEntry;
use codex_protocol::permissions::FileSystemSandboxPolicy;
use codex_protocol::permissions::FileSystemSpecialPath;
use codex_protocol::permissions::NetworkSandboxPolicy;
use codex_protocol::protocol::ReadOnlyAccess;
use codex_protocol::protocol::SandboxPolicy;
use pretty_assertions::assert_eq;

use super::landlock::LONG_TIMEOUT_MS;
use super::landlock::expect_denied;
use super::landlock::run_cmd_result_with_policies;
use super::landlock::should_skip_bwrap_tests;

fn has_ripgrep() -> bool {
    Command::new("rg").arg("--version").output().is_ok()
}

#[tokio::test]
async fn sandbox_blocks_glob_deny_read_carveouts_under_bwrap() {
    if should_skip_bwrap_tests().await {
        eprintln!("skipping bwrap test: bwrap sandbox prerequisites are unavailable");
        return;
    }
    if !has_ripgrep() {
        eprintln!("skipping bwrap test: rg is unavailable for unreadable glob expansion");
        return;
    }

    let tmpdir = tempfile::tempdir().expect("tempdir");
    let blocked_target = tmpdir.path().join("secret.env");
    std::fs::write(&blocked_target, "secret").expect("seed blocked file");

    let sandbox_policy = SandboxPolicy::ReadOnly {
        access: ReadOnlyAccess::FullAccess,
        network_access: true,
    };
    let file_system_sandbox_policy = FileSystemSandboxPolicy::restricted(vec![
        FileSystemSandboxEntry {
            path: FileSystemPath::Special {
                value: FileSystemSpecialPath::Root,
            },
            access: FileSystemAccessMode::Read,
        },
        FileSystemSandboxEntry {
            path: FileSystemPath::GlobPattern {
                pattern: format!("{}/*.env", tmpdir.path().display()),
            },
            access: FileSystemAccessMode::None,
        },
    ]);
    let output = expect_denied(
        run_cmd_result_with_policies(
            &[
                "bash",
                "-lc",
                &format!("cat {}", blocked_target.to_string_lossy()),
            ],
            sandbox_policy,
            file_system_sandbox_policy,
            NetworkSandboxPolicy::Enabled,
            LONG_TIMEOUT_MS,
            true,
        )
        .await,
        "glob deny-read carveout should be denied under bubblewrap",
    );

    assert_ne!(output.exit_code, 0);
}

#[tokio::test]
async fn sandbox_caps_glob_expansion_depth_under_bwrap() {
    if should_skip_bwrap_tests().await {
        eprintln!("skipping bwrap test: bwrap sandbox prerequisites are unavailable");
        return;
    }
    if !has_ripgrep() {
        eprintln!("skipping bwrap test: rg is unavailable for unreadable glob expansion");
        return;
    }

    let tmpdir = tempfile::tempdir().expect("tempdir");
    let shallow_target = tmpdir.path().join("shallow.env");
    let deep_target = tmpdir.path().join("nested").join("deep.env");
    std::fs::create_dir_all(deep_target.parent().expect("deep target parent"))
        .expect("create nested dir");
    std::fs::write(&shallow_target, "shallow").expect("seed shallow file");
    std::fs::write(&deep_target, "deep").expect("seed deep file");

    let sandbox_policy = SandboxPolicy::ReadOnly {
        access: ReadOnlyAccess::FullAccess,
        network_access: true,
    };
    let mut file_system_sandbox_policy = FileSystemSandboxPolicy::restricted(vec![
        FileSystemSandboxEntry {
            path: FileSystemPath::Special {
                value: FileSystemSpecialPath::Root,
            },
            access: FileSystemAccessMode::Read,
        },
        FileSystemSandboxEntry {
            path: FileSystemPath::GlobPattern {
                pattern: format!("{}/**/*.env", tmpdir.path().display()),
            },
            access: FileSystemAccessMode::None,
        },
    ]);
    file_system_sandbox_policy.glob_scan_max_depth = Some(1);

    let shallow_output = expect_denied(
        run_cmd_result_with_policies(
            &[
                "bash",
                "-lc",
                &format!("cat {}", shallow_target.to_string_lossy()),
            ],
            sandbox_policy.clone(),
            file_system_sandbox_policy.clone(),
            NetworkSandboxPolicy::Enabled,
            LONG_TIMEOUT_MS,
            true,
        )
        .await,
        "shallow glob match should be denied under bubblewrap",
    );
    assert_ne!(shallow_output.exit_code, 0);

    let deep_output = run_cmd_result_with_policies(
        &[
            "bash",
            "-lc",
            &format!("cat {}", deep_target.to_string_lossy()),
        ],
        sandbox_policy,
        file_system_sandbox_policy,
        NetworkSandboxPolicy::Enabled,
        LONG_TIMEOUT_MS,
        true,
    )
    .await
    .expect("deep file should remain readable when glob expansion depth is capped");

    assert_eq!(deep_output.exit_code, 0);
    assert_eq!(deep_output.stdout.text.trim(), "deep");
}
