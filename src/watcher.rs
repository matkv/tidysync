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

impl WatchState {
    pub fn label(self) -> &'static str {
        match self {
            WatchState::Paused => "Paused",
            WatchState::Scanning => "Scanning",
            WatchState::Watching => "Watching",
            WatchState::Failed => "Error",
        }
    }
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

/// What the supervisor should be doing.
///
/// Shutdown is a distinct signal rather than "everyone dropped the sender", so
/// stopping works no matter how many controls the tray is still holding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Signal {
    Running,
    Paused,
    Shutdown,
}

/// A clonable remote control for the watcher.
///
/// Every method is synchronous, so the tray thread can drive it directly
/// without a tokio runtime of its own.
#[derive(Clone)]
pub struct WatcherControl {
    signal: watch::Sender<Signal>,
    status: StatusHandle,
}

impl WatcherControl {
    pub fn status(&self) -> Status {
        self.status.get()
    }

    pub fn is_enabled(&self) -> bool {
        *self.signal.borrow() == Signal::Running
    }

    pub fn set_enabled(&self, enabled: bool) {
        // Send failure only means the supervisor is gone, which the caller is
        // about to notice anyway.
        let _ = self.signal.send(if enabled {
            Signal::Running
        } else {
            Signal::Paused
        });
    }
}

/// Owns the background watch task and the switch that turns it on and off.
pub struct WatcherHandle {
    control: WatcherControl,
    task: JoinHandle<()>,
}

impl WatcherHandle {
    /// Start the supervisor. `enabled` decides whether it begins watching
    /// immediately or sits paused.
    pub fn spawn(client: Arc<SyncThingClient>, config: Arc<Config>, enabled: bool) -> Self {
        let start = if enabled {
            Signal::Running
        } else {
            Signal::Paused
        };

        let (tx, rx) = watch::channel(start);
        let status = StatusHandle::new();
        let task = tokio::spawn(supervise(client, config, rx, status.clone()));

        Self {
            control: WatcherControl { signal: tx, status },
            task,
        }
    }

    pub fn control(&self) -> WatcherControl {
        self.control.clone()
    }

    pub fn status(&self) -> Status {
        self.control.status()
    }

    /// Stop and wait for the supervisor to finish the batch it is on, so we
    /// never exit mid-move. Gives up after a few seconds rather than hanging.
    pub async fn shutdown(self) {
        let _ = self.control.signal.send(Signal::Shutdown);

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
    mut signal: watch::Receiver<Signal>,
    status: StatusHandle,
) {
    loop {
        // Bound before matching: holding the watch guard across the arms would
        // conflict with the mutable borrow the session below needs.
        let current = *signal.borrow();

        match current {
            Signal::Shutdown => return,
            Signal::Running => {}
            Signal::Paused => {
                status.set_state(WatchState::Paused);
                debug!("Watcher paused");

                // Err means the handle is gone, i.e. we are shutting down.
                if signal.wait_for(|s| *s != Signal::Paused).await.is_err() {
                    return;
                }

                if *signal.borrow() == Signal::Shutdown {
                    return;
                }

                info!("Watching resumed");
            }
        }

        let session = client
            .watch_events(
                &config.source_folder_id,
                &config.target_directory,
                &mut signal,
                &status,
            )
            .await;

        if let Err(err) = session {
            warn!("Watch session failed: {err:#}");
            status.set_failed(format!("{err:#}"));

            // Back off before retrying, but wake immediately if toggled.
            tokio::select! {
                _ = tokio::time::sleep(RESTART_DELAY) => {}
                result = signal.changed() => {
                    if result.is_err() {
                        return;
                    }
                }
            }
        }
        // On Ok the session was cancelled; loop back round and re-read the
        // signal, which decides whether we park or restart.
    }
}

/// Resolves when the watcher is switched off or shut down.
///
/// Used to cancel the long poll; callers must only await this at a point where
/// abandoning the work in progress is safe.
pub(crate) async fn stopped(signal: &mut watch::Receiver<Signal>) {
    let _ = signal.wait_for(|s| *s != Signal::Running).await;
}
