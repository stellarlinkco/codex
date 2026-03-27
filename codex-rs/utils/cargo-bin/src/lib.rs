use std::ffi::OsString;
use std::io;
use std::panic::AssertUnwindSafe;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

pub use runfiles;

/// Bazel sets this when runfiles directories are disabled, which we do on all platforms for consistency.
const RUNFILES_MANIFEST_ONLY_ENV: &str = "RUNFILES_MANIFEST_ONLY";

#[derive(Debug, thiserror::Error)]
pub enum CargoBinError {
    #[error("failed to read current exe")]
    CurrentExe {
        #[source]
        source: std::io::Error,
    },
    #[error("failed to read current directory")]
    CurrentDir {
        #[source]
        source: std::io::Error,
    },
    #[error("CARGO_BIN_EXE env var {key} resolved to {path:?}, but it does not exist")]
    ResolvedPathDoesNotExist { key: String, path: PathBuf },
    #[error("could not locate binary {name:?}; tried env vars {env_keys:?}; {fallback}")]
    NotFound {
        name: String,
        env_keys: Vec<String>,
        fallback: String,
    },
}

/// Returns an absolute path to a binary target built for the current test run.
///
/// In `cargo test`, `CARGO_BIN_EXE_*` env vars are absolute.
/// In `bazel test`, `CARGO_BIN_EXE_*` env vars are rlocationpaths, intended to be consumed by `rlocation`.
/// This helper allows callers to transparently support both.
#[allow(deprecated)]
pub fn cargo_bin(name: &str) -> Result<PathBuf, CargoBinError> {
    let env_keys = cargo_bin_env_keys(name);
    for key in &env_keys {
        if let Some(value) = std::env::var_os(key) {
            return resolve_bin_from_env(key, value);
        }
    }

    let mut cargo_build_failure = None;
    if !runfiles_available() {
        match repo_root() {
            Ok(repo_root) => {
                let workspace_root = repo_root.join("codex-rs");
                let target_dir = match std::env::var_os("CARGO_TARGET_DIR") {
                    Some(path) => {
                        let path = PathBuf::from(path);
                        if path.is_absolute() {
                            path
                        } else {
                            workspace_root.join(path)
                        }
                    }
                    None => workspace_root.join("target"),
                };
                let file_name = format!("{name}{}", std::env::consts::EXE_SUFFIX);

                // Under `cargo test` / `cargo nextest`, the test binary lives under
                // `<target-dir>/<...>/<profile>/deps/<test-binary>`. Prefer resolving binaries
                // relative to that directory so custom profiles and `--target` layouts work.
                if let Ok(exe) = std::env::current_exe()
                    && let Some(profile_dir) = exe.parent().and_then(|parent| parent.parent())
                {
                    let path = profile_dir.join(&file_name);
                    if path.exists() {
                        return Ok(path);
                    }
                }

                let profile_dir = if cfg!(debug_assertions) {
                    "debug"
                } else {
                    "release"
                };
                let path = target_dir.join(profile_dir).join(&file_name);
                if path.exists() {
                    return Ok(path);
                }

                let mut cmd = Command::new("cargo");
                cmd.arg("build")
                    .arg("--quiet")
                    .arg("--bin")
                    .arg(name)
                    .current_dir(&workspace_root);
                if !cfg!(debug_assertions) {
                    cmd.arg("--release");
                }

                match cmd.output() {
                    Ok(output) => {
                        if output.status.success() && path.exists() {
                            return Ok(path);
                        }
                        let stderr = String::from_utf8_lossy(&output.stderr);
                        let stdout = String::from_utf8_lossy(&output.stdout);
                        let status = output.status;
                        if status.success() {
                            let path_display = path.display();
                            cargo_build_failure = Some(format!(
                                "cargo build --bin {name} succeeded ({status}), but binary not found at: {path_display}\n{stderr}{stdout}"
                            ));
                        } else {
                            cargo_build_failure = Some(format!(
                                "cargo build --bin {name} failed ({status}):\n{stderr}{stdout}"
                            ));
                        }
                    }
                    Err(err) => {
                        cargo_build_failure =
                            Some(format!("cargo build --bin {name} failed: {err}"));
                    }
                }
            }
            Err(err) => {
                cargo_build_failure = Some(format!("repo_root() failed: {err}"));
            }
        }
    }

    let assert_cmd_result =
        std::panic::catch_unwind(AssertUnwindSafe(|| assert_cmd::Command::cargo_bin(name)));
    match assert_cmd_result {
        Ok(Ok(cmd)) => {
            let mut path = PathBuf::from(cmd.get_program());
            if !path.is_absolute() {
                path = std::env::current_dir()
                    .map_err(|source| CargoBinError::CurrentDir { source })?
                    .join(path);
            }
            if path.exists() {
                Ok(path)
            } else {
                Err(CargoBinError::ResolvedPathDoesNotExist {
                    key: "assert_cmd::Command::cargo_bin".to_owned(),
                    path,
                })
            }
        }
        Ok(Err(err)) => Err(CargoBinError::NotFound {
            name: name.to_owned(),
            env_keys,
            fallback: match cargo_build_failure {
                Some(cargo_build_failure) => {
                    format!("{cargo_build_failure}\nassert_cmd fallback failed: {err}")
                }
                None => format!("assert_cmd fallback failed: {err}"),
            },
        }),
        Err(panic) => Err(CargoBinError::NotFound {
            name: name.to_owned(),
            env_keys,
            fallback: match cargo_build_failure {
                Some(cargo_build_failure) => format!(
                    "{cargo_build_failure}\nassert_cmd fallback panicked: {}",
                    panic_payload_message(panic)
                ),
                None => format!(
                    "assert_cmd fallback panicked: {}",
                    panic_payload_message(panic)
                ),
            },
        }),
    }
}

fn panic_payload_message(panic: Box<dyn std::any::Any + Send>) -> String {
    match panic.downcast::<String>() {
        Ok(message) => *message,
        Err(panic) => match panic.downcast::<&'static str>() {
            Ok(message) => (*message).to_string(),
            Err(_) => "non-string panic payload".to_string(),
        },
    }
}

fn cargo_bin_env_keys(name: &str) -> Vec<String> {
    let mut keys = Vec::with_capacity(2);
    keys.push(format!("CARGO_BIN_EXE_{name}"));

    // Cargo replaces dashes in target names when exporting env vars.
    let underscore_name = name.replace('-', "_");
    if underscore_name != name {
        keys.push(format!("CARGO_BIN_EXE_{underscore_name}"));
    }

    keys
}

pub fn runfiles_available() -> bool {
    std::env::var_os(RUNFILES_MANIFEST_ONLY_ENV).is_some()
}

fn resolve_bin_from_env(key: &str, value: OsString) -> Result<PathBuf, CargoBinError> {
    let raw = PathBuf::from(&value);
    if runfiles_available() {
        let runfiles = runfiles::Runfiles::create().map_err(|err| CargoBinError::CurrentExe {
            source: std::io::Error::other(err),
        })?;
        if let Some(resolved) = runfiles::rlocation!(runfiles, &raw)
            && resolved.exists()
        {
            return Ok(resolved);
        }
    } else if raw.is_absolute() && raw.exists() {
        return Ok(raw);
    }

    Err(CargoBinError::ResolvedPathDoesNotExist {
        key: key.to_owned(),
        path: raw,
    })
}

/// Macro that derives the path to a test resource at runtime, the value of
/// which depends on whether Cargo or Bazel is being used to build and run a
/// test. Note the return value may be a relative or absolute path.
/// (Incidentally, this is a macro rather than a function because it reads
/// compile-time environment variables that need to be captured at the call
/// site.)
///
/// This is expected to be used exclusively in test code because Codex CLI is a
/// standalone binary with no packaged resources.
#[macro_export]
macro_rules! find_resource {
    ($resource:expr) => {{
        let resource = std::path::Path::new(&$resource);
        if $crate::runfiles_available() {
            // When this code is built and run with Bazel:
            // - we inject `BAZEL_PACKAGE` as a compile-time environment variable
            //   that points to native.package_name()
            // - at runtime, Bazel will set runfiles-related env vars
            $crate::resolve_bazel_runfile(option_env!("BAZEL_PACKAGE"), resource)
        } else {
            let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
            Ok(manifest_dir.join(resource))
        }
    }};
}

pub fn resolve_bazel_runfile(
    bazel_package: Option<&str>,
    resource: &Path,
) -> std::io::Result<PathBuf> {
    let runfiles = runfiles::Runfiles::create()
        .map_err(|err| std::io::Error::other(format!("failed to create runfiles: {err}")))?;
    let runfile_path = match bazel_package {
        Some(bazel_package) => PathBuf::from("_main").join(bazel_package).join(resource),
        None => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "BAZEL_PACKAGE was not set at compile time",
            ));
        }
    };
    let runfile_path = normalize_runfile_path(&runfile_path);
    if let Some(resolved) = runfiles::rlocation!(runfiles, &runfile_path)
        && resolved.exists()
    {
        return Ok(resolved);
    }
    let runfile_path_display = runfile_path.display();
    Err(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        format!("runfile does not exist at: {runfile_path_display}"),
    ))
}

pub fn resolve_cargo_runfile(resource: &Path) -> std::io::Result<PathBuf> {
    let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    Ok(manifest_dir.join(resource))
}

pub fn repo_root() -> io::Result<PathBuf> {
    let marker = if runfiles_available() {
        let runfiles = runfiles::Runfiles::create()
            .map_err(|err| io::Error::other(format!("failed to create runfiles: {err}")))?;
        let marker_path = option_env!("CODEX_REPO_ROOT_MARKER")
            .map(PathBuf::from)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    "CODEX_REPO_ROOT_MARKER was not set at compile time",
                )
            })?;
        runfiles::rlocation!(runfiles, &marker_path).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "repo_root.marker not available in runfiles",
            )
        })?
    } else {
        resolve_cargo_runfile(Path::new("repo_root.marker"))?
    };
    let mut root = marker;
    for _ in 0..4 {
        root = root
            .parent()
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    "repo_root.marker did not have expected parent depth",
                )
            })?
            .to_path_buf();
    }
    Ok(root)
}

fn normalize_runfile_path(path: &Path) -> PathBuf {
    let mut components = Vec::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if matches!(components.last(), Some(std::path::Component::Normal(_))) {
                    components.pop();
                } else {
                    components.push(component);
                }
            }
            _ => components.push(component),
        }
    }

    components
        .into_iter()
        .fold(PathBuf::new(), |mut acc, component| {
            acc.push(component.as_os_str());
            acc
        })
}
