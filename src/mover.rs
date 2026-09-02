use anyhow::Context;
use std::path::Path;
use tracing::{debug, info, warn};

fn should_skip_file(path: &Path) -> bool {
    let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

    // Skip Syncthing internal files
    if file_name == ".stfolder" {
        return true;
    }

    // Skip temporary files
    if file_name.ends_with(".tmp") {
        return true;
    }

    // Skip .nomedia marker files
    if file_name == ".nomedia" {
        return true;
    }

    false
}

pub async fn move_existing_files(source_root: &Path, target_dir: &Path) -> anyhow::Result<()> {
    for entry in walkdir::WalkDir::new(source_root)
        .min_depth(1)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let path = entry.path();

        if path.is_dir() {
            continue;
        }

        if should_skip_file(path) {
            continue;
        }

        let relative = path
            .strip_prefix(source_root)
            .context("failed to get relative path for existing file")?;

        let destination = target_dir.join(relative);

        // One unreadable file shouldn't abort the whole pre-scan — log it and
        // carry on, otherwise a single permission error blocks startup entirely.
        if let Err(err) = move_file(path, &destination).await {
            warn!("Failed to move existing file {}: {err:#}", path.display());
        }
    }
    Ok(())
}

pub async fn move_file(src: &Path, dst: &Path) -> anyhow::Result<()> {
    if should_skip_file(src) {
        debug!("Skipping file: {}", src.display());
        return Ok(());
    }

    info!("Moving file from {} to {}", src.display(), dst.display());

    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)
            .context("failed to create parent directories for target file")?;
    }

    match std::fs::rename(src, dst) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::CrossesDevices => {
            copy_across_devices(src, dst).await
        }
        Err(err) => Err(err).context("failed to move file"),
    }
}

/// `rename` only works within one filesystem. When the source folder and the
/// target directory live on different mounts it fails with `EXDEV`, so fall back
/// to copying the bytes and deleting the original.
///
/// The copy goes to a `.tidysync-part` sibling first and is renamed into place
/// afterwards. That rename is within a single directory and therefore atomic, so
/// an interrupted copy can never leave a truncated file that looks complete.
async fn copy_across_devices(src: &Path, dst: &Path) -> anyhow::Result<()> {
    debug!("Target is on another filesystem, copying instead of renaming");

    let mut partial = dst.to_path_buf().into_os_string();
    partial.push(".tidysync-part");
    let partial = std::path::PathBuf::from(partial);

    // tokio::fs moves these onto the blocking pool — a multi-gigabyte copy would
    // otherwise stall the event loop for its whole duration.
    if let Err(err) = tokio::fs::copy(src, &partial).await {
        let _ = tokio::fs::remove_file(&partial).await;
        return Err(err).context("failed to copy file to target filesystem");
    }

    tokio::fs::rename(&partial, dst)
        .await
        .context("failed to rename copied file into place")?;

    tokio::fs::remove_file(src)
        .await
        .context("copied file to target but failed to remove the original")?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, contents).unwrap();
    }

    #[tokio::test]
    async fn moves_a_file_within_one_filesystem() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("report.pdf");
        let dst = dir.path().join("target/report.pdf");
        write(&src, "hello");

        move_file(&src, &dst).await.unwrap();

        assert!(!src.exists(), "source should be gone after a move");
        assert_eq!(std::fs::read_to_string(&dst).unwrap(), "hello");
    }

    /// Exercises the `EXDEV` fallback by moving between two different mounts.
    /// `std::env::temp_dir()` is often a tmpfs while the build directory sits on
    /// the real disk; if they happen to share a device there is nothing to test.
    #[tokio::test]
    async fn moves_a_file_across_filesystems() {
        use std::os::unix::fs::MetadataExt;

        let on_tmp = tempfile::tempdir().unwrap();
        let build_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target");
        std::fs::create_dir_all(&build_dir).unwrap();
        let on_disk = tempfile::tempdir_in(&build_dir).unwrap();

        let tmp_dev = on_tmp.path().metadata().unwrap().dev();
        let disk_dev = on_disk.path().metadata().unwrap().dev();
        if tmp_dev == disk_dev {
            eprintln!("skipping: temp dir and build dir are on the same filesystem");
            return;
        }

        let src = on_tmp.path().join("movie.mkv");
        let dst = on_disk.path().join("nested/movie.mkv");
        write(&src, "some bytes");

        move_file(&src, &dst).await.unwrap();

        assert!(!src.exists(), "source should be gone after a cross-device move");
        assert_eq!(std::fs::read_to_string(&dst).unwrap(), "some bytes");
        assert!(
            !dst.with_file_name("movie.mkv.tidysync-part").exists(),
            "the partial copy should not be left behind"
        );
    }
}
