#[cfg(any(unix, test))]
use std::collections::HashSet;

#[cfg(any(unix, test))]
use codex_protocol::models::MacOsSeatbeltProfileExtensions;
#[cfg(any(unix, test))]
use codex_protocol::models::PermissionProfile;
#[cfg(any(unix, test))]
use codex_protocol::permissions::FileSystemSandboxPolicy;
#[cfg(any(unix, test))]
use codex_protocol::permissions::NetworkSandboxPolicy;
#[cfg(any(unix, test))]
use codex_utils_absolute_path::AbsolutePathBuf;
#[cfg(any(unix, test))]
use dunce::canonicalize as canonicalize_path;
#[cfg(any(unix, test))]
use tracing::warn;

#[cfg(any(unix, test))]
use crate::config::Constrained;
#[cfg(any(unix, test))]
use crate::config::Permissions;
#[cfg(any(unix, test))]
use crate::config::types::ShellEnvironmentPolicy;
#[cfg(any(unix, test))]
use crate::protocol::AskForApproval;
#[cfg(any(unix, test))]
use crate::protocol::ReadOnlyAccess;
#[cfg(any(unix, test))]
use crate::protocol::SandboxPolicy;

/// Compiles a skill `PermissionProfile` for the Unix shell escalation path.
///
/// Normal Windows builds do not currently call this helper, so it is only
/// compiled on Unix and in tests.
#[cfg(any(unix, test))]
pub(crate) fn compile_permission_profile(
    permissions: Option<PermissionProfile>,
) -> Option<Permissions> {
    let PermissionProfile {
        network,
        file_system,
        macos,
    } = permissions?;
    let file_system = file_system.unwrap_or_default();
    let network_access = network
        .as_ref()
        .and_then(|value| value.enabled)
        .unwrap_or(false);
    let fs_read = normalize_permission_paths(
        file_system.read.as_deref().unwrap_or_default(),
        "permissions.file_system.read",
    );
    let fs_write = normalize_permission_paths(
        file_system.write.as_deref().unwrap_or_default(),
        "permissions.file_system.write",
    );
    let sandbox_policy = if !fs_write.is_empty() {
        SandboxPolicy::WorkspaceWrite {
            writable_roots: fs_write,
            read_only_access: if fs_read.is_empty() {
                ReadOnlyAccess::FullAccess
            } else {
                ReadOnlyAccess::Restricted {
                    include_platform_defaults: true,
                    readable_roots: fs_read,
                }
            },
            network_access,
            exclude_tmpdir_env_var: false,
            exclude_slash_tmp: false,
        }
    } else if !fs_read.is_empty() {
        SandboxPolicy::ReadOnly {
            access: ReadOnlyAccess::Restricted {
                include_platform_defaults: true,
                readable_roots: fs_read,
            },
            network_access,
        }
    } else {
        SandboxPolicy::ReadOnly {
            access: ReadOnlyAccess::FullAccess,
            network_access,
        }
    };
    let macos_permissions = macos.unwrap_or_default();
    let macos_seatbelt_profile_extensions =
        build_macos_seatbelt_profile_extensions(&macos_permissions);
    let file_system_sandbox_policy = FileSystemSandboxPolicy::from(&sandbox_policy);
    let network_sandbox_policy = NetworkSandboxPolicy::from(&sandbox_policy);

    Some(Permissions {
        approval_policy: Constrained::allow_any(AskForApproval::Never),
        sandbox_policy: Constrained::allow_any(sandbox_policy),
        file_system_sandbox_policy,
        network_sandbox_policy,
        network: None,
        allow_login_shell: true,
        shell_environment_policy: ShellEnvironmentPolicy::default(),
        windows_sandbox_mode: None,
        windows_sandbox_private_desktop: false,
        macos_seatbelt_profile_extensions,
    })
}

#[cfg(any(unix, test))]
fn normalize_permission_paths(values: &[AbsolutePathBuf], field: &str) -> Vec<AbsolutePathBuf> {
    let mut paths = Vec::new();
    let mut seen = HashSet::new();

    for value in values {
        let Some(path) = normalize_permission_path(value, field) else {
            continue;
        };
        if seen.insert(path.clone()) {
            paths.push(path);
        }
    }

    paths
}

#[cfg(any(unix, test))]
fn normalize_permission_path(value: &AbsolutePathBuf, field: &str) -> Option<AbsolutePathBuf> {
    let canonicalized = canonicalize_path(value.as_path()).unwrap_or_else(|_| value.to_path_buf());
    match AbsolutePathBuf::from_absolute_path(&canonicalized) {
        Ok(path) => Some(path),
        Err(error) => {
            warn!("ignoring {field}: expected absolute path, got {canonicalized:?}: {error}");
            None
        }
    }
}

#[cfg(target_os = "macos")]
fn build_macos_seatbelt_profile_extensions(
    permissions: &MacOsSeatbeltProfileExtensions,
) -> Option<MacOsSeatbeltProfileExtensions> {
    Some(permissions.clone())
}

#[cfg(all(not(target_os = "macos"), any(unix, test)))]
fn build_macos_seatbelt_profile_extensions(
    _: &MacOsSeatbeltProfileExtensions,
) -> Option<MacOsSeatbeltProfileExtensions> {
    None
}

#[cfg(test)]
mod tests {
    use super::compile_permission_profile;
    use crate::config::Constrained;
    use crate::config::Permissions;
    use crate::config::types::ShellEnvironmentPolicy;
    use crate::protocol::AskForApproval;
    use crate::protocol::ReadOnlyAccess;
    use crate::protocol::SandboxPolicy;
    use codex_protocol::models::FileSystemPermissions;
    #[cfg(target_os = "macos")]
    use codex_protocol::models::MacOsAutomationPermission;
    #[cfg(target_os = "macos")]
    use codex_protocol::models::MacOsPreferencesPermission;
    #[cfg(target_os = "macos")]
    use codex_protocol::models::MacOsSeatbeltProfileExtensions;
    use codex_protocol::models::NetworkPermissions;
    use codex_protocol::models::PermissionProfile;
    use codex_protocol::permissions::FileSystemSandboxPolicy;
    use codex_protocol::permissions::NetworkSandboxPolicy;
    use codex_utils_absolute_path::AbsolutePathBuf;
    use pretty_assertions::assert_eq;
    use std::fs;
    use std::path::Path;

    fn absolute_path(path: &Path) -> AbsolutePathBuf {
        AbsolutePathBuf::try_from(path).expect("absolute path")
    }

    #[test]
    fn compile_permission_profile_normalizes_paths() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let skill_dir = tempdir.path().join("skill");
        fs::create_dir_all(skill_dir.join("scripts")).expect("skill dir");
        let read_dir = skill_dir.join("data");
        fs::create_dir_all(&read_dir).expect("read dir");
        let expected_sandbox_policy = SandboxPolicy::WorkspaceWrite {
            writable_roots: vec![
                AbsolutePathBuf::try_from(skill_dir.join("output")).expect("absolute output path"),
            ],
            read_only_access: ReadOnlyAccess::Restricted {
                include_platform_defaults: true,
                readable_roots: vec![
                    AbsolutePathBuf::try_from(dunce::canonicalize(&read_dir).unwrap_or(read_dir))
                        .expect("absolute read path"),
                ],
            },
            network_access: true,
            exclude_tmpdir_env_var: false,
            exclude_slash_tmp: false,
        };

        let profile = compile_permission_profile(Some(PermissionProfile {
            network: Some(NetworkPermissions {
                enabled: Some(true),
            }),
            file_system: Some(FileSystemPermissions {
                read: Some(vec![
                    absolute_path(&skill_dir.join("data")),
                    absolute_path(&skill_dir.join("data")),
                    absolute_path(&skill_dir.join("scripts/../data")),
                ]),
                write: Some(vec![absolute_path(&skill_dir.join("output"))]),
            }),
            ..Default::default()
        }))
        .expect("profile");

        assert_eq!(
            profile,
            Permissions {
                approval_policy: Constrained::allow_any(AskForApproval::Never),
                sandbox_policy: Constrained::allow_any(expected_sandbox_policy.clone()),
                file_system_sandbox_policy: FileSystemSandboxPolicy::from(&expected_sandbox_policy),
                network_sandbox_policy: NetworkSandboxPolicy::from(&expected_sandbox_policy),
                network: None,
                allow_login_shell: true,
                shell_environment_policy: ShellEnvironmentPolicy::default(),
                windows_sandbox_mode: None,
                #[cfg(target_os = "macos")]
                macos_seatbelt_profile_extensions: Some(
                    crate::seatbelt_permissions::MacOsSeatbeltProfileExtensions::default(),
                ),
                #[cfg(not(target_os = "macos"))]
                macos_seatbelt_profile_extensions: None,
            }
        );
    }

    #[test]
    fn compile_permission_profile_without_permissions_has_empty_profile() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let skill_dir = tempdir.path().join("skill");
        fs::create_dir_all(&skill_dir).expect("skill dir");

        let profile = compile_permission_profile(None);

        assert_eq!(profile, None);
    }

    #[test]
    fn compile_permission_profile_with_network_only_uses_read_only_policy() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let skill_dir = tempdir.path().join("skill");
        fs::create_dir_all(&skill_dir).expect("skill dir");
        let expected_sandbox_policy = SandboxPolicy::ReadOnly {
            access: ReadOnlyAccess::FullAccess,
            network_access: true,
        };

        let profile = compile_permission_profile(Some(PermissionProfile {
            network: Some(NetworkPermissions {
                enabled: Some(true),
            }),
            ..Default::default()
        }))
        .expect("profile");

        assert_eq!(
            profile,
            Permissions {
                approval_policy: Constrained::allow_any(AskForApproval::Never),
                sandbox_policy: Constrained::allow_any(expected_sandbox_policy.clone()),
                file_system_sandbox_policy: FileSystemSandboxPolicy::from(&expected_sandbox_policy),
                network_sandbox_policy: NetworkSandboxPolicy::from(&expected_sandbox_policy),
                network: None,
                allow_login_shell: true,
                shell_environment_policy: ShellEnvironmentPolicy::default(),
                windows_sandbox_mode: None,
                #[cfg(target_os = "macos")]
                macos_seatbelt_profile_extensions: Some(
                    crate::seatbelt_permissions::MacOsSeatbeltProfileExtensions::default(),
                ),
                #[cfg(not(target_os = "macos"))]
                macos_seatbelt_profile_extensions: None,
            }
        );
    }

    #[test]
    fn compile_permission_profile_with_network_and_read_only_paths_uses_read_only_policy() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let skill_dir = tempdir.path().join("skill");
        let read_dir = skill_dir.join("data");
        fs::create_dir_all(&read_dir).expect("read dir");
        let expected_sandbox_policy = SandboxPolicy::ReadOnly {
            access: ReadOnlyAccess::Restricted {
                include_platform_defaults: true,
                readable_roots: vec![
                    AbsolutePathBuf::try_from(dunce::canonicalize(&read_dir).unwrap_or(read_dir))
                        .expect("absolute read path"),
                ],
            },
            network_access: true,
        };

        let profile = compile_permission_profile(Some(PermissionProfile {
            network: Some(NetworkPermissions {
                enabled: Some(true),
            }),
            file_system: Some(FileSystemPermissions {
                read: Some(vec![absolute_path(&skill_dir.join("data"))]),
                write: Some(Vec::new()),
            }),
            ..Default::default()
        }))
        .expect("profile");

        assert_eq!(
            profile,
            Permissions {
                approval_policy: Constrained::allow_any(AskForApproval::Never),
                sandbox_policy: Constrained::allow_any(expected_sandbox_policy.clone()),
                file_system_sandbox_policy: FileSystemSandboxPolicy::from(&expected_sandbox_policy),
                network_sandbox_policy: NetworkSandboxPolicy::from(&expected_sandbox_policy),
                network: None,
                allow_login_shell: true,
                shell_environment_policy: ShellEnvironmentPolicy::default(),
                windows_sandbox_mode: None,
                #[cfg(target_os = "macos")]
                macos_seatbelt_profile_extensions: Some(
                    crate::seatbelt_permissions::MacOsSeatbeltProfileExtensions::default(),
                ),
                #[cfg(not(target_os = "macos"))]
                macos_seatbelt_profile_extensions: None,
            }
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn compile_permission_profile_builds_macos_permission_file() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let skill_dir = tempdir.path().join("skill");
        fs::create_dir_all(&skill_dir).expect("skill dir");

        let profile = compile_permission_profile(Some(PermissionProfile {
            macos: Some(MacOsSeatbeltProfileExtensions {
                macos_preferences: MacOsPreferencesPermission::ReadWrite,
                macos_automation: MacOsAutomationPermission::BundleIds(vec![
                    "com.apple.Notes".to_string(),
                ]),
                macos_accessibility: true,
                macos_calendar: true,
            }),
            ..Default::default()
        }))
        .expect("profile");

        assert_eq!(
            profile.macos_seatbelt_profile_extensions,
            Some(
                crate::seatbelt_permissions::MacOsSeatbeltProfileExtensions {
                    macos_preferences:
                        crate::seatbelt_permissions::MacOsPreferencesPermission::ReadWrite,
                    macos_automation:
                        crate::seatbelt_permissions::MacOsAutomationPermission::BundleIds(vec![
                            "com.apple.Notes".to_string()
                        ],),
                    macos_accessibility: true,
                    macos_calendar: true,
                }
            )
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn compile_permission_profile_uses_macos_defaults_when_values_missing() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let skill_dir = tempdir.path().join("skill");
        fs::create_dir_all(&skill_dir).expect("skill dir");

        let profile =
            compile_permission_profile(Some(PermissionProfile::default())).expect("profile");

        assert_eq!(
            profile.macos_seatbelt_profile_extensions,
            Some(crate::seatbelt_permissions::MacOsSeatbeltProfileExtensions::default())
        );
    }
}
