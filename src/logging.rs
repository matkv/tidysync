use std::collections::VecDeque;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::fmt::time::ChronoLocal;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, fmt};

/// How many log lines the tray menu keeps.
const RECENT_CAPACITY: usize = 10;

const CLOCK: &str = "%H:%M:%S";

/// Initialise logging for CLI subcommands.
///
/// Defaults to `info`; `RUST_LOG=debug tidysync watch` turns on per-event detail.
pub fn init() {
    tracing_subscriber::registry()
        .with(filter())
        .with(stdout_layer())
        .init();
}

/// Initialise logging for tray mode: the same stdout output, plus a rolling
/// in-memory buffer for the menu and a log file on disk.
///
/// The returned value must be kept alive for the life of the process — dropping
/// it stops the background writer and log lines are silently lost.
pub fn init_tray() -> Result<TrayLogging> {
    let recent = RecentLog::default();

    let dir = state_dir()?;
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("failed to create {}", dir.display()))?;
    let log_path = dir.join("tidysync.log");

    let (file, guard) = tracing_appender::non_blocking(tracing_appender::rolling::never(
        &dir,
        "tidysync.log",
    ));

    tracing_subscriber::registry()
        .with(filter())
        .with(stdout_layer())
        .with(
            // Compact and un-coloured: these lines go straight into menu labels.
            fmt::layer()
                .with_writer(recent.clone())
                .with_timer(ChronoLocal::new(CLOCK.to_string()))
                .with_ansi(false)
                .with_target(false)
                .with_level(false),
        )
        .with(
            fmt::layer()
                .with_writer(file)
                .with_timer(ChronoLocal::new("%Y-%m-%d %H:%M:%S".to_string()))
                .with_ansi(false)
                .with_target(false),
        )
        .init();

    Ok(TrayLogging {
        recent,
        log_path,
        _guard: guard,
    })
}

pub struct TrayLogging {
    pub recent: RecentLog,
    pub log_path: PathBuf,
    _guard: WorkerGuard,
}

/// Empty the log file, and the menu's view of it.
///
/// Truncates rather than deletes. The appender holds this file open, so
/// unlinking it would leave the writer appending to an inode nothing can read
/// any more, and every later line would vanish silently until restart. The
/// appender opens with `O_APPEND`, so after truncation writes simply resume at
/// the start of the file.
pub fn clear_log(path: &Path, recent: &RecentLog) -> Result<()> {
    std::fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .create(true)
        .open(path)
        .with_context(|| format!("failed to clear {}", path.display()))?;

    recent.clear();

    Ok(())
}

fn stdout_layer<S>() -> fmt::Layer<S, fmt::format::DefaultFields, fmt::format::Format<fmt::format::Full, ChronoLocal>>
where
    S: tracing::Subscriber,
{
    fmt::layer()
        .with_timer(ChronoLocal::new(CLOCK.to_string()))
        .with_target(false)
}

fn filter() -> EnvFilter {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    // The HTTP stack logs every connection pool operation at debug, which buries
    // our own output under `RUST_LOG=debug`. These directives replace any RUST_LOG
    // set for the same targets, so the HTTP stack is pinned at info regardless.
    ["hyper=info", "hyper_util=info", "reqwest=info", "rustls=info"]
        .iter()
        .fold(filter, |filter, directive| {
            filter.add_directive(directive.parse().expect("static directive is valid"))
        })
}

fn state_dir() -> Result<PathBuf> {
    let dir = dirs::state_dir()
        .or_else(dirs::cache_dir)
        .context("could not determine a directory for the log file")?;

    Ok(dir.join("tidysync"))
}

/// The last few log lines, for display in the tray menu.
///
/// Implemented as a `MakeWriter` rather than a bespoke `Layer` so that tracing's
/// own formatter does the work; we only have to catch the finished lines.
#[derive(Clone, Default)]
pub struct RecentLog(Arc<Mutex<Buffer>>);

#[derive(Default)]
struct Buffer {
    lines: VecDeque<String>,
    /// Bytes written since the last newline.
    pending: String,
}

impl RecentLog {
    pub fn clear(&self) {
        let mut buffer = self.0.lock().expect("recent log mutex poisoned");
        buffer.lines.clear();
        buffer.pending.clear();
    }

    /// Oldest first.
    pub fn lines(&self) -> Vec<String> {
        self.0
            .lock()
            .expect("recent log mutex poisoned")
            .lines
            .iter()
            .cloned()
            .collect()
    }
}

impl io::Write for RecentLog {
    /// Callers are not obliged to hand us one whole line per call — `write!`
    /// splits a format string into a call per fragment — so text is accumulated
    /// and only split off once a newline actually arrives.
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut buffer = self.0.lock().expect("recent log mutex poisoned");

        buffer.pending.push_str(&String::from_utf8_lossy(buf));

        while let Some(newline) = buffer.pending.find('\n') {
            let line: String = buffer.pending.drain(..=newline).collect();
            let line = line.trim_end();

            if !line.is_empty() {
                if buffer.lines.len() == RECENT_CAPACITY {
                    buffer.lines.pop_front();
                }

                let line = line.to_owned();
                buffer.lines.push_back(line);
            }
        }

        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for RecentLog {
    type Writer = Self;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn keeps_only_the_most_recent_lines() {
        let mut log = RecentLog::default();

        for i in 0..RECENT_CAPACITY + 5 {
            writeln!(log, "line {i}").unwrap();
        }

        let lines = log.lines();
        assert_eq!(lines.len(), RECENT_CAPACITY);
        assert_eq!(lines.first().unwrap(), "line 5");
        assert_eq!(lines.last().unwrap(), "line 14");
    }

    #[test]
    fn splits_multi_line_writes_and_drops_blanks() {
        let mut log = RecentLog::default();

        log.write_all(b"first\n\nsecond\n").unwrap();

        assert_eq!(log.lines(), vec!["first", "second"]);
    }

    /// `write!` hands the writer one call per format fragment, so a line has to
    /// survive being delivered in pieces.
    #[test]
    fn reassembles_a_line_split_across_writes() {
        let mut log = RecentLog::default();

        log.write_all(b"Moved ").unwrap();
        log.write_all(b"report.pdf").unwrap();
        assert!(log.lines().is_empty(), "nothing until the newline arrives");

        log.write_all(b"\n").unwrap();
        assert_eq!(log.lines(), vec!["Moved report.pdf"]);
    }

    #[test]
    fn clear_log_empties_both_the_file_and_the_buffer() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tidysync.log");
        std::fs::write(&path, "an old line\n").unwrap();

        let mut recent = RecentLog::default();
        writeln!(recent, "an old line").unwrap();
        assert_eq!(recent.lines().len(), 1);

        clear_log(&path, &recent).unwrap();

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "");
        assert!(recent.lines().is_empty());
    }

    #[test]
    fn clear_log_succeeds_when_the_file_is_missing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gone.log");

        clear_log(&path, &RecentLog::default()).unwrap();

        assert!(path.exists(), "clearing recreates the file");
    }

    #[test]
    fn clones_share_one_buffer() {
        let mut log = RecentLog::default();
        let mut clone = log.clone();

        writeln!(log, "a").unwrap();
        writeln!(clone, "b").unwrap();

        assert_eq!(log.lines(), vec!["a", "b"]);
    }
}
