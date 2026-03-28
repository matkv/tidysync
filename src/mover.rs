use anyhow::Context;

pub async fn move_file(src: &std::path::Path, dst: &std::path::PathBuf) -> anyhow::Result<()> {
    println!("Moving file from {} to {}", src.display(), dst.display());

    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)
            .context("failed to create parent directories for target file")?;
    }

    std::fs::rename(src, dst).context("failed to move file")?;
    Ok(())
}
