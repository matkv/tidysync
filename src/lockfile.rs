use std::fs::{File, TryLockError};
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use tracing::debug;

/// An exclusive lock that allows only one watcher to run at a time.
///
/// Two watchers pointed at the same source folder would race on every event and
/// fight over the same files, so a CLI `watch` and a tray instance must not both
/// be moving things. The lock is held for as long as this value is alive, and
/// the kernel releases it when the process exits — including on a crash, so a
/// stale lock file can never wedge the next run.
#[derive(Debug)]
pub struct WatchLock {
    /// Never read: the lock lives exactly as long as this handle is open.
    _file: File,
    path: PathBuf,
}

impl WatchLock {
    pub fn acquire() -> Result<Self> {
        Self::acquire_at(&lock_path()?)
    }

    fn acquire_at(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }

        // Deliberately not truncating on open: a contending process needs to be
        // able to read the holder's pid out of the file it does not own.
        let mut file = File::options()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(path)
            .with_context(|| format!("failed to open lock file {}", path.display()))?;

        match file.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => match read_pid(&mut file) {
                Some(pid) => bail!(
                    "another tidysync is already watching (pid {pid}); \
                     stop it first or let it keep running"
                ),
                None => bail!(
                    "another tidysync is already watching (lock held on {})",
                    path.display()
                ),
            },
            Err(TryLockError::Error(err)) => {
                return Err(err)
                    .with_context(|| format!("failed to lock {}", path.display()));
            }
        }

        record_pid(&mut file)
            .with_context(|| format!("failed to write pid to {}", path.display()))?;

        debug!("Acquired watch lock at {}", path.display());

        Ok(Self {
            _file: file,
            path: path.to_path_buf(),
        })
    }
}

impl Drop for WatchLock {
    fn drop(&mut self) {
        // Closing the handle is what actually releases the lock; this is only
        // here to make the lifetime visible in debug logs.
        debug!("Released watch lock at {}", self.path.display());
    }
}

fn lock_path() -> Result<PathBuf> {
    let dir = dirs::state_dir()
        .or_else(dirs::cache_dir)
        .context("could not determine a directory for the lock file")?;

    Ok(dir.join("tidysync").join("tidysync.lock"))
}

fn record_pid(file: &mut File) -> std::io::Result<()> {
    file.set_len(0)?;
    file.rewind()?;
    write!(file, "{}", std::process::id())?;
    file.flush()
}

fn read_pid(file: &mut File) -> Option<u32> {
    let mut contents = String::new();
    file.rewind().ok()?;
    file.read_to_string(&mut contents).ok()?;
    contents.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_a_second_lock_while_the_first_is_held() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tidysync.lock");

        let first = WatchLock::acquire_at(&path).unwrap();

        let err = WatchLock::acquire_at(&path).unwrap_err();
        assert!(
            err.to_string().contains("already watching"),
            "unexpected message: {err}"
        );

        drop(first);
        WatchLock::acquire_at(&path).expect("lock should be free once released");
    }

    #[test]
    fn records_the_holding_pid() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tidysync.lock");

        let _lock = WatchLock::acquire_at(&path).unwrap();

        let contents = std::fs::read_to_string(&path).unwrap();
        assert_eq!(contents.trim(), std::process::id().to_string());
    }

    #[test]
    fn creates_the_parent_directory() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/state/tidysync.lock");

        let _lock = WatchLock::acquire_at(&path).unwrap();

        assert!(path.exists());
    }
}
