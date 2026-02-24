use std::fs::OpenOptions;
use std::io;
use std::path::Path;

pub(super) struct FileLockGuard {
    _file: std::fs::File,
}

#[cfg(unix)]
fn lock_file_exclusive_blocking(path: &Path) -> Result<FileLockGuard, io::Error> {
    use std::os::unix::io::AsRawFd;

    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)?;
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }

    Ok(FileLockGuard { _file: file })
}

#[cfg(not(unix))]
fn lock_file_exclusive_blocking(path: &Path) -> Result<FileLockGuard, io::Error> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)?;

    Ok(FileLockGuard { _file: file })
}

pub(super) async fn lock_file_exclusive(path: &Path) -> Result<FileLockGuard, io::Error> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || lock_file_exclusive_blocking(&path))
        .await
        .map_err(|err| io::Error::other(err.to_string()))?
}
