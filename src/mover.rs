use anyhow::Context;
use std::path::Path;
use tracing::{debug, info, warn};

/// Directories Syncthing owns and we must never touch: the folder marker
/// (`.stfolder`, a directory into which Syncthing writes a randomly-named file
/// on some platforms) and the versioning store (`.stversions`).
const SYNCTHING_DIRS: [&str; 2] = [".stfolder", ".stversions"];

fn should_skip_file(path: &Path) -> bool {
    use std::path::Component;

    // Match on *any* path component, not just the basename: the offending file
    // is usually one Syncthing itself dropped *inside* `.stfolder`, whose own
    // name gives nothing away.
    for component in path.components() {
        if let Component::Normal(name) = component
            && SYNCTHING_DIRS.iter().any(|d| name == *d)
        {
            return true;
        }
    }

    let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

    // Skip Syncthing internal files (marker file variant, ignore list)
    if file_name == ".stfolder" || file_name == ".stignore" {
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

/// Sweep everything already sitting in the source folder, returning how many
/// files were moved.
///
/// `should_continue` is polled between files so a long backlog can be abandoned
/// promptly when the watcher is switched off. It is checked between files rather
/// than during one, so a move is never interrupted half-way.
pub async fn move_existing_files(
    source_root: &Path,
    target_dir: &Path,
    should_continue: impl Fn() -> bool,
) -> anyhow::Result<u64> {
    let mut moved = 0;

    for entry in walkdir::WalkDir::new(source_root)
        .min_depth(1)
        .into_iter()
        .filter_entry(|e| !SYNCTHING_DIRS.contains(&e.file_name().to_string_lossy().as_ref()))
        .filter_map(|e| e.ok())
    {
        if !should_continue() {
            debug!("Pre-scan cancelled after {moved} file(s)");
            break;
        }

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
        match move_file(path, &destination).await {
            Ok(true) => moved += 1,
            Ok(false) => {}
            Err(err) => warn!("Failed to move existing file {}: {err:#}", path.display()),
        }
    }

    Ok(moved)
}

/// Move one file, returning whether it was actually moved (as opposed to
/// deliberately skipped).
pub async fn move_file(src: &Path, dst: &Path) -> anyhow::Result<bool> {
    if should_skip_file(src) {
        debug!("Skipping file: {}", src.display());
        return Ok(false);
    }

    debug!("Moving {} to {}", src.display(), dst.display());

    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)
            .context("failed to create parent directories for target file")?;
    }

    match std::fs::rename(src, dst) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::CrossesDevices => {
            copy_across_devices(src, dst).await?
        }
        Err(err) => return Err(err).context("failed to move file"),
    }

    // Logged after the fact, and by name rather than by full path: this line is
    // what shows up in the tray menu, where a pair of absolute paths would be
    // truncated into uselessness. Full paths are at debug above.
    info!("Moved {}", display_name(src));

    Ok(true)
}

fn display_name(path: &Path) -> String {
    path.file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned()
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

        assert!(move_file(&src, &dst).await.unwrap(), "should report a move");

        assert!(!src.exists(), "source should be gone after a move");
        assert_eq!(std::fs::read_to_string(&dst).unwrap(), "hello");
    }

    #[tokio::test]
    async fn reports_skipped_files_as_not_moved() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join(".nomedia");
        let dst = dir.path().join("target/.nomedia");
        write(&src, "");

        assert!(!move_file(&src, &dst).await.unwrap(), "should report a skip");
        assert!(src.exists(), "a skipped file stays where it is");
        assert!(!dst.exists());
    }

    #[tokio::test]
    async fn pre_scan_counts_moves_and_honours_cancellation() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        let target = dir.path().join("target");
        for name in ["a.txt", "b.txt", "c.txt"] {
            write(&source.join(name), "x");
        }
        // Skipped files must not be counted.
        write(&source.join(".nomedia"), "");

        let moved = move_existing_files(&source, &target, || true).await.unwrap();
        assert_eq!(moved, 3);

        // With the watcher switched off nothing should be swept at all.
        for name in ["d.txt", "e.txt"] {
            write(&source.join(name), "x");
        }
        let moved = move_existing_files(&source, &target, || false)
            .await
            .unwrap();
        assert_eq!(moved, 0);
        assert!(source.join("d.txt").exists());
    }

    #[tokio::test]
    async fn skips_files_inside_syncthing_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join(".stfolder/~syncthing~marker");
        let dst = dir.path().join("target/.stfolder/~syncthing~marker");
        write(&src, "");

        assert!(
            !move_file(&src, &dst).await.unwrap(),
            "a file inside .stfolder must be skipped, not moved"
        );
        assert!(src.exists(), "the skipped file stays put");
        assert!(!dst.exists());
    }

    #[tokio::test]
    async fn pre_scan_never_descends_into_syncthing_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        let target = dir.path().join("target");
        write(&source.join("keep.txt"), "x");
        write(&source.join(".stfolder/~syncthing~marker"), "");
        write(&source.join(".stversions/old.txt"), "x");

        let moved = move_existing_files(&source, &target, || true).await.unwrap();
        assert_eq!(moved, 1, "only the real file should move");
        assert!(source.join(".stfolder/~syncthing~marker").exists());
        assert!(source.join(".stversions/old.txt").exists());
    }

    /// Exercises the `EXDEV` fallback by moving between two different mounts.
    /// `std::env::temp_dir()` is often a tmpfs while the build directory sits on
    /// the real disk; if they happen to share a device there is nothing to test.
    /// Unix-only: it relies on `st_dev` to tell the two mounts apart.
    #[cfg(unix)]
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

        assert!(move_file(&src, &dst).await.unwrap(), "should report a move");

        assert!(!src.exists(), "source should be gone after a cross-device move");
        assert_eq!(std::fs::read_to_string(&dst).unwrap(), "some bytes");
        assert!(
            !dst.with_file_name("movie.mkv.tidysync-part").exists(),
            "the partial copy should not be left behind"
        );
    }
}
