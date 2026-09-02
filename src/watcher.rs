use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::watch;
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

use crate::{client::SyncThingClient, config::Config};

/// How long to wait before restarting a watch session that failed outright
/// (Syncthing unreachable at startup, the folder gone from its config, ...).
const RESTART_DELAY: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchState {
    /// Switched off; nothing is polling.
    Paused,
    /// Sweeping files that already exist in the source folder.
    Scanning,
    /// Long-polling Syncthing for new events.
    Watching,
    /// The session failed and will be retried.
    Failed,
}

#[derive(Debug, Clone)]
pub struct Status {
    pub state: WatchState,
    pub moved: u64,
    pub last_error: Option<String>,
    /// Bumped on every change, so a UI can skip redrawing when nothing moved.
    pub generation: u64,
}

/// Shared, cheaply-clonable view of what the watcher is doing.
///
/// Uses a std mutex rather than a tokio one: every critical section here is a
/// couple of field writes, and the lock is deliberately never held across an
/// `.await`. That also lets the tray thread read it without a runtime.
#[derive(Clone)]
pub struct StatusHandle(Arc<Mutex<Status>>);

impl StatusHandle {
    fn new() -> Self {
        Self(Arc::new(Mutex::new(Status {
            state: WatchState::Paused,
            moved: 0,
            last_error: None,
            generation: 0,
        })))
    }

    pub fn get(&self) -> Status {
        self.0.lock().expect("status mutex poisoned").clone()
    }

    fn update(&self, apply: impl FnOnce(&mut Status)) {
        let mut status = self.0.lock().expect("status mutex poisoned");
        apply(&mut status);
        status.generation += 1;
    }

    pub(crate) fn set_state(&self, state: WatchState) {
        self.update(|status| {
            status.state = state;
            if state != WatchState::Failed {
                status.last_error = None;
            }
        });
    }

    fn set_failed(&self, error: String) {
        self.update(|status| {
            status.state = WatchState::Failed;
            status.last_error = Some(error);
        });
    }

    pub(crate) fn record_moves(&self, count: u64) {
        if count > 0 {
            self.update(|status| status.moved += count);
        }
    }
}

/// Owns the background watch task and the switch that turns it on and off.
pub struct WatcherHandle {
    enabled: watch::Sender<bool>,
    status: StatusHandle,
    task: JoinHandle<()>,
}

impl WatcherHandle {
    /// Start the supervisor. `enabled` decides whether it begins watching
    /// immediately or sits paused.
    pub fn spawn(client: Arc<SyncThingClient>, config: Arc<Config>, enabled: bool) -> Self {
        let (tx, rx) = watch::channel(enabled);
        let status = StatusHandle::new();
        let task = tokio::spawn(supervise(client, config, rx, status.clone()));

        Self {
            enabled: tx,
            status,
            task,
        }
    }

    pub fn status(&self) -> Status {
        self.status.get()
    }

    // The toggle this whole supervisor exists to make possible. Nothing drives
    // it until tray mode lands; the CLI always runs enabled.
    #[allow(dead_code)]
    pub fn is_enabled(&self) -> bool {
        *self.enabled.borrow()
    }

    #[allow(dead_code)]
    pub fn set_enabled(&self, enabled: bool) {
        // Send failure only means the supervisor is gone, which the caller is
        // about to notice anyway.
        let _ = self.enabled.send(enabled);
    }

    /// Switch off and wait for the supervisor to notice, so we do not exit while
    /// a file is mid-move. Gives up after a few seconds rather than hanging.
    pub async fn shutdown(self) {
        drop(self.enabled);

        if tokio::time::timeout(Duration::from_secs(5), self.task)
            .await
            .is_err()
        {
            warn!("Watcher did not stop within 5s, exiting anyway");
        }
    }
}

/// Restarts watch sessions for as long as the watcher is switched on.
///
/// Toggling off cancels the running session; toggling on starts a fresh one,
/// which re-runs the pre-scan and so sweeps up everything Syncthing delivered
/// while we were paused.
async fn supervise(
    client: Arc<SyncThingClient>,
    config: Arc<Config>,
    mut enabled: watch::Receiver<bool>,
    status: StatusHandle,
) {
    loop {
        if !*enabled.borrow() {
            status.set_state(WatchState::Paused);
            debug!("Watcher paused");

            // Err means every sender is gone, i.e. we are shutting down.
            if enabled.wait_for(|on| *on).await.is_err() {
                return;
            }

            info!("Watching resumed");
        }

        let session = client
            .watch_events(
                &config.source_folder_id,
                &config.target_directory,
                &mut enabled,
                &status,
            )
            .await;

        match session {
            // Returned because the watcher was switched off or dropped.
            Ok(()) => {
                if enabled.has_changed().is_err() {
                    return;
                }
            }
            Err(err) => {
                warn!("Watch session failed: {err:#}");
                status.set_failed(format!("{err:#}"));

                // Back off before retrying, but wake immediately if toggled.
                tokio::select! {
                    _ = tokio::time::sleep(RESTART_DELAY) => {}
                    result = enabled.changed() => {
                        if result.is_err() {
                            return;
                        }
                    }
                }
            }
        }
    }
}

/// Resolves when the watcher is switched off, or when its handle is dropped.
///
/// Used to cancel the long poll; callers must only await this at a point where
/// abandoning the work in progress is safe.
pub(crate) async fn stopped(enabled: &mut watch::Receiver<bool>) {
    let _ = enabled.wait_for(|on| !*on).await;
}
