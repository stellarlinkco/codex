use codex_protocol::permissions::FileSystemAccessMode;
use codex_protocol::permissions::FileSystemPath;
use codex_protocol::permissions::FileSystemSandboxEntry;
use codex_protocol::permissions::FileSystemSandboxPolicy;
use codex_protocol::permissions::FileSystemSpecialPath;
use codex_utils_absolute_path::AbsolutePathBuf;
use pretty_assertions::assert_eq;
use tempfile::TempDir;

#[test]
fn restricted_policy_preserves_glob_scan_max_depth() {
    let mut policy = FileSystemSandboxPolicy::restricted(vec![FileSystemSandboxEntry {
        path: FileSystemPath::Special {
            value: FileSystemSpecialPath::CurrentWorkingDirectory,
        },
        access: FileSystemAccessMode::Read,
    }]);
    policy.glob_scan_max_depth = Some(4);

    assert_eq!(policy.glob_scan_max_depth, Some(4));
}

#[test]
fn glob_pattern_deny_entries_are_not_resolved_as_exact_unreadable_roots() {
    let cwd = TempDir::new().expect("create cwd");
    let cwd_absolute =
        AbsolutePathBuf::from_absolute_path(cwd.path()).expect("cwd should be absolute");
    let mut policy = FileSystemSandboxPolicy::restricted(vec![FileSystemSandboxEntry {
        path: FileSystemPath::GlobPattern {
            pattern: format!("{}/**/*.env", cwd.path().display()),
        },
        access: FileSystemAccessMode::None,
    }]);
    policy.glob_scan_max_depth = Some(3);

    assert_eq!(policy.get_unreadable_roots_with_cwd(cwd.path()), Vec::new());
    assert_eq!(
        policy.get_unreadable_globs_with_cwd(cwd.path()),
        vec![format!("{}/**/*.env", cwd.path().display())]
    );
    assert_eq!(policy.get_readable_roots_with_cwd(cwd.path()), Vec::new());
    assert_eq!(cwd_absolute.as_path(), cwd.path());
}
