use anyhow::Context;
use chrono::Local;
use std::path::Path;

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
        move_file(path, &destination).await.with_context(|| {
            format!(
                "failed to move existing file {} to {}",
                path.display(),
                destination.display()
            )
        })?;
    }
    Ok(())
}

pub async fn move_file(src: &Path, dst: &std::path::PathBuf) -> anyhow::Result<()> {
    if should_skip_file(src) {
        println!("[{}] Skipping file: {}", Local::now().format("%H:%M:%S"), src.display());
        return Ok(());
    }

    println!("[{}] Moving file from {} to {}", Local::now().format("%H:%M:%S"), src.display(), dst.display());

    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)
            .context("failed to create parent directories for target file")?;
    }

    std::fs::rename(src, dst).context("failed to move file")?;
    Ok(())
}
